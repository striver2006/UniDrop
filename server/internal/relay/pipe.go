package relay

import (
	"errors"
	"sync"
	"sync/atomic"
	"time"
)

var (
	ErrPipeClosed        = errors.New("relay pipe is closed")
	ErrReceiverCongested = errors.New("downstream receiver is congested (timeout)")
	ErrSenderCongested   = errors.New("upstream sender is congested (timeout)")
)

// RelayPipe provides bidirectional streaming channels between sender and receiver.
type RelayPipe struct {
	SessionID string

	// AccountID of the authorization this pipe was created from. It is carried
	// here so that ownership can still be established when the auth entry is
	// gone, and so that per-account pipe accounting does not need a second
	// lookup.
	AccountID  string
	FromDevice string
	ToDevice   string
	ForwardChan  chan *[]byte // Sender -> Receiver (data frames)
	BackwardChan chan *[]byte // Receiver -> Sender (ACK/NACK frames)
	DoneChan     chan struct{}
	closeOnce    sync.Once
	LastActiveAt int64 // Unix nanoseconds
	CreatedAt    time.Time
}

// NewRelayPipe creates a new streaming relay pipe.
func NewRelayPipe(sessionID, accountID, fromDevice, toDevice string) *RelayPipe {
	now := time.Now()
	return &RelayPipe{
		SessionID:    sessionID,
		AccountID:    accountID,
		FromDevice:   fromDevice,
		ToDevice:     toDevice,
		ForwardChan:  make(chan *[]byte, 4),  // Buffered so a sender that dials before the receiver is not immediately congested
		BackwardChan: make(chan *[]byte, 16), // Light ACK/NACK frames
		DoneChan:     make(chan struct{}),
		LastActiveAt: now.UnixNano(),
		CreatedAt:    now,
	}
}

// Touch updates the last activity timestamp.
func (p *RelayPipe) Touch() {
	atomic.StoreInt64(&p.LastActiveAt, time.Now().UnixNano())
}

// LastActive returns the last active time.
func (p *RelayPipe) LastActive() time.Time {
	return time.Unix(0, atomic.LoadInt64(&p.LastActiveAt))
}

// Close gracefully closes the pipe.
func (p *RelayPipe) Close() {
	p.closeOnce.Do(func() {
		close(p.DoneChan)
	})
}

// PushForward pushes a data buffer to the receiver.
func (p *RelayPipe) PushForward(buf *[]byte, timeout time.Duration) error {
	p.Touch()
	timer := time.NewTimer(timeout)
	defer timer.Stop()

	select {
	case <-p.DoneChan:
		return ErrPipeClosed
	case p.ForwardChan <- buf:
		return nil
	case <-timer.C:
		return ErrReceiverCongested
	}
}

// PushBackward pushes an ACK/NACK buffer back to the sender.
func (p *RelayPipe) PushBackward(buf *[]byte, timeout time.Duration) error {
	p.Touch()
	timer := time.NewTimer(timeout)
	defer timer.Stop()

	select {
	case <-p.DoneChan:
		return ErrPipeClosed
	case p.BackwardChan <- buf:
		return nil
	case <-timer.C:
		return ErrSenderCongested
	}
}
