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
	pipe := NewRelayPipe("sess1", "devA", "devB")
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
	pipe := NewRelayPipe("sess2", "devA", "devB")
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
	mgr := NewRelayManager()
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
	m := NewRelayManager()

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
	m := NewRelayManager()

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
	m := NewRelayManager()

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
	m := NewRelayManager()

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

	m := NewRelayManager()

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
