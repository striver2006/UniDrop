package registry_test

import (
	"sync"
	"testing"
	"time"

	"github.com/unidrop/unidrop-server/internal/registry"
)

func TestSessionLastPingDataRace(t *testing.T) {
	session := registry.NewDeviceSession("user1", "dev1", "Host1", "macos", "1.0.0", "127.0.0.1", nil)

	var wg sync.WaitGroup
	stop := make(chan struct{})

	// 1. Worker simulating ReadPump TouchPing
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			select {
			case <-stop:
				return
			default:
				session.TouchPing(time.Now())
			}
		}
	}()

	// 2. Worker simulating Cleaner SweepInactive GetLastPing
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			select {
			case <-stop:
				return
			default:
				_ = session.GetLastPing()
			}
		}
	}()

	// Run concurrently for 100ms under -race detector
	time.Sleep(100 * time.Millisecond)
	close(stop)
	wg.Wait()
}
