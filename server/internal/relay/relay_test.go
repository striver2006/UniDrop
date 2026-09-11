package relay

import (
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

	// Fill channel (cap = 2)
	_ = pipe.PushForward(pool.Get(), 10*time.Millisecond)
	_ = pipe.PushForward(pool.Get(), 10*time.Millisecond)

	// Third push should timeout
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
