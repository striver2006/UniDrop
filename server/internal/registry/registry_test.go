package registry

import (
	"sync"
	"testing"
	"time"
)

func TestDeviceRegistryCRUD(t *testing.T) {
	reg := NewDeviceRegistry()

	s1 := NewDeviceSession("user1", "dev1", "MacBook", "macos", "1.0.0", "127.0.0.1", nil)
	s2 := NewDeviceSession("user1", "dev2", "Windows11", "windows", "1.0.0", "127.0.0.1", nil)

	reg.Register(s1)
	reg.Register(s2)

	if reg.Count() != 2 {
		t.Fatalf("expected count 2, got %d", reg.Count())
	}

	found, ok := reg.Get("user1", "dev1")
	if !ok || found.Hostname != "MacBook" {
		t.Fatalf("expected dev1 found with MacBook hostname")
	}

	user1Devices := reg.ListByAccount("user1")
	if len(user1Devices) != 2 {
		t.Fatalf("expected 2 devices for user1, got %d", len(user1Devices))
	}

	// Replacement on re-register
	s1New := NewDeviceSession("user1", "dev1", "MacBook-Pro", "macos", "1.0.1", "127.0.0.1", nil)
	reg.Register(s1New)

	// Old s1 should be closed
	select {
	case <-s1.Closed:
		// success
	default:
		t.Fatal("expected old session to be closed upon replacement")
	}

	foundNew, _ := reg.Get("user1", "dev1")
	if foundNew.Hostname != "MacBook-Pro" {
		t.Fatalf("expected new session hostname MacBook-Pro, got %s", foundNew.Hostname)
	}

	reg.Unregister("user1", "dev1")
	if _, ok := reg.Get("user1", "dev1"); ok {
		t.Fatal("expected dev1 unregistered")
	}
}

func TestDeviceRegistrySweepInactive(t *testing.T) {
	reg := NewDeviceRegistry()
	now := time.Now()

	s1 := NewDeviceSession("user1", "dev1", "PC1", "windows", "1.0.0", "127.0.0.1", nil)
	s1.TouchPing(now.Add(-50 * time.Second)) // timed out (>45s)

	s2 := NewDeviceSession("user1", "dev2", "PC2", "linux", "1.0.0", "127.0.0.1", nil)
	s2.TouchPing(now.Add(-10 * time.Second)) // still alive

	reg.Register(s1)
	reg.Register(s2)

	timedOut := reg.SweepInactive(45*time.Second, now)
	if len(timedOut) != 1 || timedOut[0].DeviceID != "dev1" {
		t.Fatalf("expected dev1 timed out, got %v", timedOut)
	}

	if reg.Count() != 1 {
		t.Fatalf("expected count 1 after sweep, got %d", reg.Count())
	}
}

func TestDeviceRegistryUnregisterSessionCAS(t *testing.T) {
	reg := NewDeviceRegistry()

	// Old session
	sOld := NewDeviceSession("user1", "dev1", "PC1", "windows", "1.0.0", "127.0.0.1", nil)
	reg.Register(sOld)

	// New reconnect session for same deviceID
	sNew := NewDeviceSession("user1", "dev1", "PC1", "windows", "1.0.0", "127.0.0.1", nil)
	reg.Register(sNew)

	// Old session exits defer UnregisterSession: should FAIL to delete sNew (P0-5)
	if reg.UnregisterSession(sOld) {
		t.Fatal("expected unregister of stale session to fail CAS")
	}

	// sNew must still be present and alive
	current, exists := reg.Get("user1", "dev1")
	if !exists || current != sNew {
		t.Fatal("expected sNew to remain registered")
	}

	// sNew unregister should succeed
	if !reg.UnregisterSession(sNew) {
		t.Fatal("expected sNew unregister to succeed")
	}
	if _, exists := reg.Get("user1", "dev1"); exists {
		t.Fatal("expected dev1 to be deleted after sNew unregister")
	}
}

func TestDeviceRegistryConcurrentAccess(t *testing.T) {
	reg := NewDeviceRegistry()
	var wg sync.WaitGroup

	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			devID := "dev_" + string(rune('A'+id%26))
			s := NewDeviceSession("user1", devID, "host", "linux", "1.0.0", "127.0.0.1", nil)
			reg.Register(s)
			reg.Get("user1", devID)
			reg.ListByAccount("user1")
		}(i)
	}

	wg.Wait()
}

// 本组核心负例：两个账号用了相同的 device_id。
// 改回扁平键（以 DeviceID 为唯一键）这条立刻红 ——
// 那正是「B 账号连上来就把 A 的设备踢下线」的形态，也是用户最在意的一条。
func TestRegisterSameDeviceIDDifferentAccountsCoexist(t *testing.T) {
	reg := NewDeviceRegistry()

	a := NewDeviceSession("acctA", "dev1", "A-Mac", "macos", "1.0.0", "127.0.0.1", nil)
	b := NewDeviceSession("acctB", "dev1", "B-Mac", "macos", "1.0.0", "127.0.0.1", nil)

	reg.Register(a)
	reg.Register(b)

	select {
	case <-a.Closed:
		t.Fatal("另一个账号注册同名 device_id 时，本账号的会话不得被关闭")
	default:
	}

	if reg.Count() != 2 {
		t.Fatalf("两个账号的同名设备应当共存，期望 2 个会话，实得 %d", reg.Count())
	}
}

// 正例：确认没有把「同账号同设备重连顶掉旧连接」这个正当行为一起修掉。
func TestRegisterSameAccountSameDeviceStillReplaces(t *testing.T) {
	reg := NewDeviceRegistry()

	old := NewDeviceSession("acctA", "dev1", "旧连接", "macos", "1.0.0", "127.0.0.1", nil)
	fresh := NewDeviceSession("acctA", "dev1", "新连接", "macos", "1.0.1", "127.0.0.1", nil)

	reg.Register(old)
	reg.Register(fresh)

	select {
	case <-old.Closed:
	default:
		t.Fatal("同账号同设备重连必须顶掉旧连接")
	}
	if reg.Count() != 1 {
		t.Fatalf("顶号后应只剩 1 个会话，实得 %d", reg.Count())
	}
}

func TestGetIsScopedToAccount(t *testing.T) {
	reg := NewDeviceRegistry()
	reg.Register(NewDeviceSession("acctA", "dev1", "A-Mac", "macos", "1.0.0", "127.0.0.1", nil))

	if _, ok := reg.Get("acctB", "dev1"); ok {
		t.Fatal("查另一个账号下的同名 device_id 必须查不到")
	}
	if s, ok := reg.Get("acctA", "dev1"); !ok || s.Hostname != "A-Mac" {
		t.Fatal("本账号下的设备必须查得到")
	}
}

// CAS 不得误摘他账号的同名条目。
func TestUnregisterSessionCASIgnoresOtherAccountSameDeviceID(t *testing.T) {
	reg := NewDeviceRegistry()

	a := NewDeviceSession("acctA", "dev1", "A-Mac", "macos", "1.0.0", "127.0.0.1", nil)
	b := NewDeviceSession("acctB", "dev1", "B-Mac", "macos", "1.0.0", "127.0.0.1", nil)
	reg.Register(a)
	reg.Register(b)

	if !reg.UnregisterSession(a) {
		t.Fatal("摘除自己的会话应当成功")
	}
	if _, ok := reg.Get("acctB", "dev1"); !ok {
		t.Fatal("摘除 A 账号的会话不得连带摘掉 B 账号的同名设备")
	}
}

// 广播既不能漏发，也不能串发到同名 device_id 的另一个账号。
func TestBroadcastToAccountSkipsOtherAccountSameDeviceID(t *testing.T) {
	reg := NewDeviceRegistry()

	a1 := NewDeviceSession("acctA", "dev1", "A1", "macos", "1.0.0", "127.0.0.1", nil)
	a2 := NewDeviceSession("acctA", "dev2", "A2", "macos", "1.0.0", "127.0.0.1", nil)
	b1 := NewDeviceSession("acctB", "dev1", "B1", "macos", "1.0.0", "127.0.0.1", nil)
	reg.Register(a1)
	reg.Register(a2)
	reg.Register(b1)

	reg.BroadcastToAccount("acctA", "dev1", []byte("hello"))

	if len(a2.SendChan) != 1 {
		t.Fatalf("同账号的其他设备必须收到广播，实得 %d 条", len(a2.SendChan))
	}
	if len(a1.SendChan) != 0 {
		t.Fatal("被排除的设备不得收到广播")
	}
	if len(b1.SendChan) != 0 {
		t.Fatal("另一个账号的同名设备不得收到广播")
	}
}

func TestSweepInactiveRemovesOnlyTimedOutAcrossAccounts(t *testing.T) {
	reg := NewDeviceRegistry()
	now := time.Now()

	stale := NewDeviceSession("acctA", "dev1", "A1", "macos", "1.0.0", "127.0.0.1", nil)
	stale.TouchPing(now.Add(-50 * time.Second))
	alive := NewDeviceSession("acctB", "dev1", "B1", "macos", "1.0.0", "127.0.0.1", nil)
	alive.TouchPing(now.Add(-10 * time.Second))
	reg.Register(stale)
	reg.Register(alive)

	timedOut := reg.SweepInactive(45*time.Second, now)
	if len(timedOut) != 1 {
		t.Fatalf("只应清掉 1 个超时会话，实得 %d", len(timedOut))
	}
	if timedOut[0].AccountID != "acctA" || timedOut[0].DeviceID != "dev1" {
		t.Fatalf("清掉的应当是 acctA/dev1，实得 %+v", timedOut[0])
	}
	if _, ok := reg.Get("acctB", "dev1"); !ok {
		t.Fatal("另一个账号的同名设备不得被连带清掉")
	}
}

// 这两个聚合是 /metrics 能以有界基数回答「是否单账号吃满机器」的依据。
func TestCountAccountsAndMaxDevicesPerAccount(t *testing.T) {
	reg := NewDeviceRegistry()

	if reg.CountAccounts() != 0 || reg.MaxDevicesPerAccount() != 0 {
		t.Fatal("空注册表的两个聚合都应为 0")
	}

	reg.Register(NewDeviceSession("acctA", "dev1", "h", "macos", "1", "127.0.0.1", nil))
	reg.Register(NewDeviceSession("acctA", "dev2", "h", "macos", "1", "127.0.0.1", nil))
	reg.Register(NewDeviceSession("acctA", "dev3", "h", "macos", "1", "127.0.0.1", nil))
	reg.Register(NewDeviceSession("acctB", "dev1", "h", "macos", "1", "127.0.0.1", nil))

	if got := reg.CountAccounts(); got != 2 {
		t.Fatalf("在线账号数应为 2，实得 %d", got)
	}
	if got := reg.MaxDevicesPerAccount(); got != 3 {
		t.Fatalf("单账号设备数峰值应为 3，实得 %d", got)
	}
}
