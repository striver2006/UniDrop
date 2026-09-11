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

	found, ok := reg.Get("dev1")
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

	foundNew, _ := reg.Get("dev1")
	if foundNew.Hostname != "MacBook-Pro" {
		t.Fatalf("expected new session hostname MacBook-Pro, got %s", foundNew.Hostname)
	}

	reg.Unregister("dev1")
	if _, ok := reg.Get("dev1"); ok {
		t.Fatal("expected dev1 unregistered")
	}
}

func TestDeviceRegistrySweepInactive(t *testing.T) {
	reg := NewDeviceRegistry()
	now := time.Now()

	s1 := NewDeviceSession("user1", "dev1", "PC1", "windows", "1.0.0", "127.0.0.1", nil)
	s1.LastPingAt = now.Add(-50 * time.Second) // timed out (>45s)

	s2 := NewDeviceSession("user1", "dev2", "PC2", "linux", "1.0.0", "127.0.0.1", nil)
	s2.LastPingAt = now.Add(-10 * time.Second) // still alive

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
			reg.Get(devID)
			reg.ListByAccount("user1")
		}(i)
	}

	wg.Wait()
}
