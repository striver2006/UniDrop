package relay

import (
	"errors"
	"strconv"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestBufferPool(t *testing.T) {
	pool := NewBufferPool()
	buf1 := pool.Get()
	if len(*buf1) != BufferSize {
		t.Fatalf("expected buffer size %d, got %d", BufferSize, len(*buf1))
	}

	// Modify content
	(*buf1)[0] = 0x55
	pool.Put(buf1)

	buf2 := pool.Get()
	if len(*buf2) != BufferSize {
		t.Fatalf("expected buffer size %d after Put, got %d", BufferSize, len(*buf2))
	}
}

func TestRelayPipeForwardBackward(t *testing.T) {
	pipe := NewRelayPipe("sess1", "acctA", "devA", "devB")
	pool := NewBufferPool()

	buf := pool.Get()
	(*buf)[0] = 0xAA

	// Test forward push
	err := pipe.PushForward(buf, 100*time.Millisecond)
	if err != nil {
		t.Fatalf("PushForward failed: %v", err)
	}

	select {
	case received := <-pipe.ForwardChan:
		if (*received)[0] != 0xAA {
			t.Fatalf("unexpected content: 0x%02x", (*received)[0])
		}
	case <-time.After(500 * time.Millisecond):
		t.Fatal("timed out waiting for ForwardChan")
	}

	// Test backward push (ACK)
	ackBuf := pool.Get()
	(*ackBuf)[0] = 0xBB
	err = pipe.PushBackward(ackBuf, 100*time.Millisecond)
	if err != nil {
		t.Fatalf("PushBackward failed: %v", err)
	}

	select {
	case received := <-pipe.BackwardChan:
		if (*received)[0] != 0xBB {
			t.Fatalf("unexpected content: 0x%02x", (*received)[0])
		}
	case <-time.After(500 * time.Millisecond):
		t.Fatal("timed out waiting for BackwardChan")
	}
}

func TestRelayPipeCongestionTimeout(t *testing.T) {
	pipe := NewRelayPipe("sess2", "acctA", "devA", "devB")
	pool := NewBufferPool()

	// Fill the ForwardChan buffer (cap = 4)
	for i := 0; i < 4; i++ {
		_ = pipe.PushForward(pool.Get(), 10*time.Millisecond)
	}

	// Push beyond capacity should timeout
	start := time.Now()
	err := pipe.PushForward(pool.Get(), 50*time.Millisecond)
	elapsed := time.Since(start)

	if err != ErrReceiverCongested {
		t.Fatalf("expected ErrReceiverCongested, got: %v", err)
	}
	if elapsed < 40*time.Millisecond {
		t.Fatalf("expected timeout to wait ~50ms, elapsed %v", elapsed)
	}
}

func TestRelayManagerSweepIdle(t *testing.T) {
	mgr := NewRelayManager(0)
	now := time.Now()

	p1, _ := mgr.GetOrCreatePipe("sess1", "devA", "devB")
	p2, _ := mgr.GetOrCreatePipe("sess2", "devA", "devC")

	p1.LastActiveAt = now.Add(-70 * time.Second).UnixNano() // idle
	p2.LastActiveAt = now.Add(-10 * time.Second).UnixNano() // active

	swept := mgr.SweepIdlePipes(60*time.Second, now)
	if len(swept) != 1 || swept[0] != "sess1" {
		t.Fatalf("expected sess1 swept, got %v", swept)
	}

	if mgr.Count() != 1 {
		t.Fatalf("expected 1 remaining pipe, got %d", mgr.Count())
	}
}

// ---------- 按账号并发上限（需求 3） ----------

// 计数只看本账号。跨账号互不影响，否则一个账号忙起来会拖垮所有人。
func TestAuthorizeSessionIfUnderLimitCountsOnlyOwnAccount(t *testing.T) {
	m := NewRelayManager(0)

	for i := 0; i < 5; i++ {
		if _, _, err := m.AuthorizeSessionIfUnderLimit(
			"other-"+strconv.Itoa(i), "s", "r", "other-account", time.Minute, 0,
		); err != nil {
			t.Fatalf("预置他账号会话失败: %v", err)
		}
	}

	token, inFlight, err := m.AuthorizeSessionIfUnderLimit("mine", "s", "r", "my-account", time.Minute, 1)
	if err != nil {
		t.Fatalf("他账号的 5 个在途不应算到本账号头上: %v", err)
	}
	if inFlight != 0 {
		t.Fatalf("本账号在途应为 0，实际 %d", inFlight)
	}
	if token == "" {
		t.Fatal("应铸出 token")
	}
}

// 重新授权同一个 session 不该把自己算进去——否则它会拒掉自己的重试。
func TestAuthorizeSessionIfUnderLimitExcludesItself(t *testing.T) {
	m := NewRelayManager(0)

	if _, _, err := m.AuthorizeSessionIfUnderLimit("s1", "a", "b", "acct", time.Minute, 1); err != nil {
		t.Fatalf("首次授权失败: %v", err)
	}
	// 上限 1，且已有 s1；重新授权 s1 自己必须成功
	if _, inFlight, err := m.AuthorizeSessionIfUnderLimit("s1", "a", "b", "acct", time.Minute, 1); err != nil {
		t.Fatalf("重新授权同一 session 不应被自己挡住: %v (in_flight=%d)", err, inFlight)
	}
	// 换一个 session 才应该被拒
	if _, _, err := m.AuthorizeSessionIfUnderLimit("s2", "a", "b", "acct", time.Minute, 1); !errors.Is(err, ErrConcurrencyLimit) {
		t.Fatalf("第二个不同 session 应被拒，实际 err=%v", err)
	}
}

// 已过期的授权不计入在途——否则额度会被僵尸会话永久占住。
func TestAuthorizeSessionIfUnderLimitIgnoresExpired(t *testing.T) {
	m := NewRelayManager(0)

	// 负 TTL：落库即过期
	if _, _, err := m.AuthorizeSessionIfUnderLimit("stale", "a", "b", "acct", -time.Minute, 0); err != nil {
		t.Fatalf("预置过期会话失败: %v", err)
	}

	_, inFlight, err := m.AuthorizeSessionIfUnderLimit("fresh", "a", "b", "acct", time.Minute, 1)
	if err != nil {
		t.Fatalf("过期会话不应占用额度: %v", err)
	}
	if inFlight != 0 {
		t.Fatalf("在途应为 0（过期的不算），实际 %d", inFlight)
	}
}

func TestAuthorizeSessionIfUnderLimitZeroDisablesCheck(t *testing.T) {
	m := NewRelayManager(0)

	for i := 0; i < 50; i++ {
		if _, _, err := m.AuthorizeSessionIfUnderLimit(
			"s"+strconv.Itoa(i), "a", "b", "acct", time.Minute, 0,
		); err != nil {
			t.Fatalf("上限为 0 时第 %d 个不应被拒: %v", i, err)
		}
	}
}

// **本组的核心**：同账号并发调用不得越界。
//
// 改造前计数持 RLock、授权另持 Lock，两段之间有窗口：同账号的多个连接可以
// 同时读到「还有名额」而同时获得授权。这条测试用 -race 跑并发调用，断言
// 最终授权数严格等于上限。把两段拆回两把锁，它会红。
func TestAuthorizeSessionIfUnderLimitIsAtomicUnderConcurrency(t *testing.T) {
	const limit = 8
	const goroutines = 64

	m := NewRelayManager(0)

	var wg sync.WaitGroup
	var granted int64
	start := make(chan struct{})

	for i := 0; i < goroutines; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			<-start // 尽量让所有 goroutine 同时冲进去
			_, _, err := m.AuthorizeSessionIfUnderLimit(
				"sess-"+strconv.Itoa(i), "a", "b", "acct", time.Minute, limit,
			)
			if err == nil {
				atomic.AddInt64(&granted, 1)
			} else if !errors.Is(err, ErrConcurrencyLimit) {
				t.Errorf("非预期错误: %v", err)
			}
		}(i)
	}
	close(start)
	wg.Wait()

	if got := atomic.LoadInt64(&granted); got != limit {
		t.Fatalf("并发授权数必须恰好等于上限 %d，实际 %d —— "+
			"大于说明计数与铸 token 不在同一把锁下，小于说明有会话被误拒", limit, got)
	}
}

// ---- 会话归属保护（A3）与每账号管道上限（A4）----

// B 账号拿 A 的 sessionID 去授权：必须被拒，且 **A 的旧 token 仍然有效**。
// 只断言返回了 error 是不够的 —— 那样即使条目已被改写也会「通过」。
func TestAuthorizeSessionRejectsCrossAccountOverwrite(t *testing.T) {
	m := NewRelayManager(0)

	tokenA, _, err := m.AuthorizeSessionIfUnderLimit("sess-x", "devA1", "devA2", "acctA", time.Minute, 0)
	if err != nil {
		t.Fatalf("A 账号首次授权应当成功：%v", err)
	}

	if _, _, err := m.AuthorizeSessionIfUnderLimit("sess-x", "devB1", "devB2", "acctB", time.Minute, 0); !errors.Is(err, ErrSessionIDConflict) {
		t.Fatalf("跨账号复用会话标识必须报 ErrSessionIDConflict，实得 %v", err)
	}

	if err := m.CheckAuthorization("sess-x", "sender", "devA1", "devA2", tokenA); err != nil {
		t.Fatalf("A 账号的原授权不得被覆写，实得 %v", err)
	}
}

// 同账号但设备对变了：合法的重复授权只有「接收方重发 ANSWER」一种，设备对必然不变。
func TestAuthorizeSessionRejectsSameAccountDeviceMismatch(t *testing.T) {
	m := NewRelayManager(0)

	if _, _, err := m.AuthorizeSessionIfUnderLimit("sess-y", "dev1", "dev2", "acctA", time.Minute, 0); err != nil {
		t.Fatalf("首次授权应当成功：%v", err)
	}
	if _, _, err := m.AuthorizeSessionIfUnderLimit("sess-y", "dev1", "dev3", "acctA", time.Minute, 0); !errors.Is(err, ErrSessionIDConflict) {
		t.Fatalf("同账号内设备对变化必须报冲突，实得 %v", err)
	}
}

// 正例：防止把「接收方重发 ANSWER」这条正常路径一并打死。
func TestAuthorizeSessionAllowsSameAccountReauthorization(t *testing.T) {
	m := NewRelayManager(0)

	if _, _, err := m.AuthorizeSessionIfUnderLimit("sess-z", "dev1", "dev2", "acctA", time.Minute, 0); err != nil {
		t.Fatalf("首次授权应当成功：%v", err)
	}
	if _, _, err := m.AuthorizeSessionIfUnderLimit("sess-z", "dev1", "dev2", "acctA", time.Minute, 0); err != nil {
		t.Fatalf("同账号同设备对的重复授权必须放行，实得 %v", err)
	}
}

// 过期条目是死容量，报冲突只会让一个陈旧 ID 白白把键位毒上整个 TTL。
func TestAuthorizeSessionAllowsOverwriteOfExpiredEntry(t *testing.T) {
	m := NewRelayManager(0)

	if _, _, err := m.AuthorizeSessionIfUnderLimit("sess-e", "dev1", "dev2", "acctA", time.Nanosecond, 0); err != nil {
		t.Fatalf("首次授权应当成功：%v", err)
	}
	time.Sleep(2 * time.Millisecond)

	if _, _, err := m.AuthorizeSessionIfUnderLimit("sess-e", "devX", "devY", "acctB", time.Minute, 0); err != nil {
		t.Fatalf("已过期的条目必须允许被覆写，实得 %v", err)
	}
}

// 钉死顺序决策：冲突检查必须排在 countAuthorizedForAccountLocked 之前。
// 顺序颠倒的话，B 复用 A 的 sessionID 会因为「按 ID 排除自己」而白得一个额度豁免。
func TestConflictCheckPrecedesConcurrencyCount(t *testing.T) {
	m := NewRelayManager(0)

	// acctB 已占满 1 个额度
	if _, _, err := m.AuthorizeSessionIfUnderLimit("b-own", "devB1", "devB2", "acctB", time.Minute, 1); err != nil {
		t.Fatalf("B 账号首次授权应当成功：%v", err)
	}
	// acctA 持有 sess-shared
	if _, _, err := m.AuthorizeSessionIfUnderLimit("sess-shared", "devA1", "devA2", "acctA", time.Minute, 0); err != nil {
		t.Fatalf("A 账号授权应当成功：%v", err)
	}

	// B 复用 A 的 sessionID，且自己已达上限 1。
	// 必须报冲突（而不是因为「排除了 sess-shared」导致计数为 0 而放行）。
	_, _, err := m.AuthorizeSessionIfUnderLimit("sess-shared", "devB1", "devB2", "acctB", time.Minute, 1)
	if !errors.Is(err, ErrSessionIDConflict) {
		t.Fatalf("冲突检查必须先于并发计数，期望 ErrSessionIDConflict，实得 %v", err)
	}
}

// 拆除归属：改造前任何已鉴权客户端自报一个 session_id 就能拆掉别人的管道。
func TestRemovePipeForSessionRejectsOtherAccount(t *testing.T) {
	m := NewRelayManager(0)

	token, _, err := m.AuthorizeSessionIfUnderLimit("sess-t", "dev1", "dev2", "acctA", time.Minute, 0)
	if err != nil {
		t.Fatalf("授权应当成功：%v", err)
	}
	if _, err := m.ValidateAndGetOrCreatePipe("sess-t", "sender", "dev1", "dev2", token); err != nil {
		t.Fatalf("建立管道应当成功：%v", err)
	}

	if r, _ := m.RemovePipeForSession("sess-t", "acctB", "dev1"); r == TeardownOK {
		t.Fatal("另一个账号不得拆除本账号的管道")
	}
	if _, ok := m.GetPipe("sess-t"); !ok {
		t.Fatal("被拒的拆除不得真的把管道删掉")
	}
}

// 账号内的非参与设备同样拆不掉：数据面早就钉死在 sender/receiver 两个角色上，
// 元数据面比数据面松是没有理由的。
func TestRemovePipeForSessionRejectsNonParticipantDevice(t *testing.T) {
	m := NewRelayManager(0)

	token, _, _ := m.AuthorizeSessionIfUnderLimit("sess-n", "dev1", "dev2", "acctA", time.Minute, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("sess-n", "sender", "dev1", "dev2", token); err != nil {
		t.Fatalf("建立管道应当成功：%v", err)
	}

	if r, _ := m.RemovePipeForSession("sess-n", "acctA", "dev9"); r == TeardownOK {
		t.Fatal("同账号内的非参与设备不得拆除该管道")
	}
	if _, ok := m.GetPipe("sess-n"); !ok {
		t.Fatal("被拒的拆除不得真的把管道删掉")
	}
}

// 正例：发送方与接收方都必须能合法拆除。
func TestRemovePipeForSessionSucceedsForBothParticipants(t *testing.T) {
	for _, actor := range []string{"dev1", "dev2"} {
		m := NewRelayManager(0)
		token, _, _ := m.AuthorizeSessionIfUnderLimit("sess-p", "dev1", "dev2", "acctA", time.Minute, 0)
		if _, err := m.ValidateAndGetOrCreatePipe("sess-p", "sender", "dev1", "dev2", token); err != nil {
			t.Fatalf("建立管道应当成功：%v", err)
		}

		if r, _ := m.RemovePipeForSession("sess-p", "acctA", actor); r != TeardownOK {
			t.Fatalf("参与方 %s 必须能拆除自己的管道", actor)
		}
		if _, ok := m.GetPipe("sess-p"); ok {
			t.Fatalf("参与方 %s 拆除后管道应当消失", actor)
		}
	}
}

// 迟到/重复的终结信令：既不成功也不该被当成越权。
func TestRemovePipeForSessionOnUnknownSessionIsNotAnError(t *testing.T) {
	m := NewRelayManager(0)

	// 必须是 NotFound 而不是 Denied：调用方据此把它记成 debug 而非
	// 「跨账号拆除」告警，否则诚实的长传输会把真正的越权淹掉。
	r, owner := m.RemovePipeForSession("never-existed", "acctA", "dev1")
	if r != TeardownNotFound {
		t.Fatalf("不存在的会话应判为 TeardownNotFound，实得 %v", r)
	}
	if owner != "" {
		t.Fatalf("不存在的会话不应有归属账号，实得 %q", owner)
	}
}

// 拒绝时必须在**同一次持锁**内带出归属账号。
// 这条守的是那次 TOCTOU：原先调用方拿到 false 后再查一次 OwnerOfSession，
// 两次加锁之间若有新授权占用同一 session_id，一条良性的迟到信令就会被
// 误报成跨账号越权。
func TestTeardownDeniedCarriesOwnerAccount(t *testing.T) {
	m := NewRelayManager(0)

	tok, _, _ := m.AuthorizeSessionIfUnderLimit("owned", "dev1", "dev2", "acctA", time.Minute, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("owned", "sender", "dev1", "dev2", tok); err != nil {
		t.Fatalf("建立管道应当成功：%v", err)
	}

	r, owner := m.RemovePipeForSession("owned", "acctB", "devB1")
	if r != TeardownDenied {
		t.Fatalf("跨账号拆除应判为 TeardownDenied，实得 %v", r)
	}
	if owner != "acctA" {
		t.Fatalf("拒绝结果必须带出真实归属账号以便告警定位，实得 %q", owner)
	}
}

// 单账号打满每账号上限后，**其他账号仍然必须能建管道**。
// 否则一个账号就能饿死全实例，等于没修。
func TestPipeCapIsPerAccountNotGlobal(t *testing.T) {
	m := NewRelayManager(1)

	tokA, _, _ := m.AuthorizeSessionIfUnderLimit("a1", "devA1", "devA2", "acctA", time.Minute, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("a1", "sender", "devA1", "devA2", tokA); err != nil {
		t.Fatalf("A 账号第一条管道应当建立成功：%v", err)
	}

	tokA2, _, _ := m.AuthorizeSessionIfUnderLimit("a2", "devA1", "devA2", "acctA", time.Minute, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("a2", "sender", "devA1", "devA2", tokA2); !errors.Is(err, ErrAccountPipeLimit) {
		t.Fatalf("A 账号超过每账号上限应当被拒，实得 %v", err)
	}

	tokB, _, _ := m.AuthorizeSessionIfUnderLimit("b1", "devB1", "devB2", "acctB", time.Minute, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("b1", "sender", "devB1", "devB2", tokB); err != nil {
		t.Fatalf("A 账号打满不得影响 B 账号建管道，实得 %v", err)
	}
}

// 正例，钉死检查顺序：到达上限后，**已存在管道的再次查找仍须成功**。
// 顺序写反的话，传输中的数据面重连会被自己账号的上限打断，
// 用户看到的是「传到一半莫名中断」。
func TestExistingPipeLookupSucceedsAtCap(t *testing.T) {
	m := NewRelayManager(1)

	tok, _, _ := m.AuthorizeSessionIfUnderLimit("s1", "dev1", "dev2", "acctA", time.Minute, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("s1", "sender", "dev1", "dev2", tok); err != nil {
		t.Fatalf("首次建立管道应当成功：%v", err)
	}

	// 账号已达上限，但这是对**同一条**管道的再次查找（接收方接入 / 断线重连）
	if _, err := m.ValidateAndGetOrCreatePipe("s1", "receiver", "dev2", "dev1", tok); err != nil {
		t.Fatalf("到达上限后已存在管道的查找必须成功，实得 %v", err)
	}
}

// 全局上限作为第二道线没有被架空。
func TestGlobalPipeCapStillApplies(t *testing.T) {
	m := NewRelayManager(0) // 关闭每账号上限，只看全局

	for i := 0; i < MaxConcurrentPipes; i++ {
		sid := "g" + strconv.Itoa(i)
		tok, _, _ := m.AuthorizeSessionIfUnderLimit(sid, "dev1", "dev2", "acct"+strconv.Itoa(i), time.Minute, 0)
		if _, err := m.ValidateAndGetOrCreatePipe(sid, "sender", "dev1", "dev2", tok); err != nil {
			t.Fatalf("第 %d 条管道应当建立成功：%v", i, err)
		}
	}

	tok, _, _ := m.AuthorizeSessionIfUnderLimit("overflow", "dev1", "dev2", "acctZ", time.Minute, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("overflow", "sender", "dev1", "dev2", tok); !errors.Is(err, ErrServerBusy) {
		t.Fatalf("超出全局上限应当报 ErrServerBusy，实得 %v", err)
	}
}

// 生命周期联动：pipe 还活着时，它的 auth 条目不得被清扫掉。
// 这条红了就意味着超过 5 分钟的诚实长传输会在结束时被判成「未授权的拆除」，
// 同时账号在途计数归零、容量拒绝退化到数据面。
func TestSweepDoesNotPurgeAuthOfLivePipe(t *testing.T) {
	m := NewRelayManager(0)

	// TTL 取一个够建完管道、又能在本用例内过期的短值
	tok, _, _ := m.AuthorizeSessionIfUnderLimit("live", "dev1", "dev2", "acctA", 50*time.Millisecond, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("live", "sender", "dev1", "dev2", tok); err != nil {
		t.Fatalf("建立管道应当成功：%v", err)
	}

	// 授权已过期，但管道仍然活跃（远未达空闲阈值）
	time.Sleep(80 * time.Millisecond)
	m.SweepIdlePipes(time.Hour, time.Now())

	// 必须直接断言 authSessions 本身。
	// 不能用 OwnerOfSession 或 RemovePipeForSession 来判断：两者都会在
	// 条目缺失时回落到 pipe 上携带的 AccountID（RemovePipeForSession 的 case 4），
	// 于是即便联动被撤掉它们也照样成功 —— 断言会失去全部鉴别力。
	m.mu.RLock()
	_, authAlive := m.authSessions["live"]
	m.mu.RUnlock()
	if !authAlive {
		t.Fatal("活跃管道的授权条目不得被清扫：一旦被清，账号在途计数会归零，" +
			"容量拒绝就从 TRANSFER_ANSWER 退化到数据面，用户只能看到笼统的连接失败")
	}

	// 附带确认拆除本身可用（此处走的是正常分支，不是 case 4 兜底）
	if r, _ := m.RemovePipeForSession("live", "acctA", "dev1"); r != TeardownOK {
		t.Fatal("长传输结束时的拆除必须被认可，而不是当成未授权")
	}
}

// case 4 兜底本身也要有守卫：auth 条目确实不在时，
// 归属仍须由 pipe 上携带的 AccountID 把住，不能一律放行。
func TestRemovePipeFallsBackToPipeOwnershipWhenAuthGone(t *testing.T) {
	m := NewRelayManager(0)

	tok, _, _ := m.AuthorizeSessionIfUnderLimit("orphan", "dev1", "dev2", "acctA", time.Minute, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("orphan", "sender", "dev1", "dev2", tok); err != nil {
		t.Fatalf("建立管道应当成功：%v", err)
	}

	// 人为制造「条目没了但管道还在」的中间态
	m.mu.Lock()
	delete(m.authSessions, "orphan")
	m.mu.Unlock()

	if r, _ := m.RemovePipeForSession("orphan", "acctB", "dev1"); r == TeardownOK {
		t.Fatal("授权条目缺失时也不得放行跨账号拆除")
	}
	if r, _ := m.RemovePipeForSession("orphan", "acctA", "dev9"); r == TeardownOK {
		t.Fatal("授权条目缺失时也不得放行非参与设备的拆除")
	}
	if r, _ := m.RemovePipeForSession("orphan", "acctA", "dev2"); r != TeardownOK {
		t.Fatal("授权条目缺失时，真正的参与方仍应能拆除")
	}
}

// 授权条目在其管道存活期间必须保持**有效**，而不只是留在 map 里。
//
// 这条守的是一个真实的劫持链：长传输超过 5 分钟后 ExpiresAt 已过，若此时
// 「条目仍在」但各处判定仍只看 ExpiresAt，攻击者就能用受害者的 session_id
// 重新授权（冲突检查因条目「已过期」而放行），再凭新条目拆掉对方正在传输的管道。
//
// 只断言条目还在是不够的 —— 它可以在场却不起任何作用。
func TestExpiredAuthOfLivePipeStaysBinding(t *testing.T) {
	m := NewRelayManager(0)

	tok, _, _ := m.AuthorizeSessionIfUnderLimit("victim", "devA1", "devA2", "acctA", 50*time.Millisecond, 0)
	if _, err := m.ValidateAndGetOrCreatePipe("victim", "sender", "devA1", "devA2", tok); err != nil {
		t.Fatalf("受害者建立管道应当成功：%v", err)
	}
	time.Sleep(80 * time.Millisecond) // 授权已过 TTL，但管道仍活跃

	// ① 不得被其他账号覆盖
	if _, _, err := m.AuthorizeSessionIfUnderLimit("victim", "devB1", "devB2", "acctB", time.Minute, 0); !errors.Is(err, ErrSessionIDConflict) {
		t.Fatalf("管道仍活跃时，过期授权不得被他人覆盖，实得 %v", err)
	}

	// ② 攻击者拆不掉
	if r, _ := m.RemovePipeForSession("victim", "acctB", "devB1"); r == TeardownOK {
		t.Fatal("攻击者不得拆除受害者正在传输的管道")
	}
	if _, ok := m.GetPipe("victim"); !ok {
		t.Fatal("受害者的管道不得消失")
	}

	// ③ 仍计入本账号在途，容量拒绝才不会退化到数据面
	if got := m.countAuthorizedForAccountLockedForTest("acctA", "", time.Now()); got != 1 {
		t.Fatalf("活跃管道的授权必须计入在途，期望 1，实得 %d", got)
	}

	// ④ 原持有者的 token 仍然可用：传输中途的数据面重连不得被打断
	if err := m.CheckAuthorization("victim", "sender", "devA1", "devA2", tok); err != nil {
		t.Fatalf("长传输途中的数据面重连必须仍被授权，实得 %v", err)
	}

	// ⑤ 参与方自己仍能正常拆除
	if r, _ := m.RemovePipeForSession("victim", "acctA", "devA1"); r != TeardownOK {
		t.Fatal("参与方必须能拆除自己的管道")
	}
}

// 供上面的用例检查在途计数，避免把私有方法暴露给生产代码。
func (m *RelayManager) countAuthorizedForAccountLockedForTest(accountID, exclude string, now time.Time) int {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.countAuthorizedForAccountLocked(accountID, exclude, now)
}
