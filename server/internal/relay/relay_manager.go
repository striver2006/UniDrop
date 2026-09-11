package relay

import (
	"errors"
	"sync"
	"time"
)

const (
	// MaxConcurrentPipes is the maximum number of active transfer pipes permitted.
	MaxConcurrentPipes = 200

	// DefaultPipeIdleTimeout is the idle duration before an inactive pipe is pruned.
	DefaultPipeIdleTimeout = 60 * time.Second
)

var (
	ErrServerBusy   = errors.New("server relay capacity reached, please retry later")
	ErrPipeNotFound = errors.New("relay pipe expired or not found")
)

// RelayManager manages active streaming data pipes and the shared buffer pool.
type RelayManager struct {
	mu         sync.RWMutex
	pipes      map[string]*RelayPipe // key: SessionID
	bufferPool *BufferPool
}

// NewRelayManager initializes a new RelayManager.
func NewRelayManager() *RelayManager {
	return &RelayManager{
		pipes:      make(map[string]*RelayPipe),
		bufferPool: NewBufferPool(),
	}
}

// BufferPool returns the shared buffer pool.
func (m *RelayManager) BufferPool() *BufferPool {
	return m.bufferPool
}

// GetOrCreatePipe gets an existing pipe or creates a new one if within capacity.
func (m *RelayManager) GetOrCreatePipe(sessionID, fromDevice, toDevice string) (*RelayPipe, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if p, exists := m.pipes[sessionID]; exists {
		p.Touch()
		return p, nil
	}

	if len(m.pipes) >= MaxConcurrentPipes {
		return nil, ErrServerBusy
	}

	p := NewRelayPipe(sessionID, fromDevice, toDevice)
	m.pipes[sessionID] = p
	return p, nil
}

// GetPipe retrieves an existing pipe by sessionID.
func (m *RelayManager) GetPipe(sessionID string) (*RelayPipe, bool) {
	m.mu.RLock()
	defer m.mu.RUnlock()

	p, exists := m.pipes[sessionID]
	return p, exists
}

// RemovePipe removes a pipe by sessionID and closes it.
func (m *RelayManager) RemovePipe(sessionID string) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if p, exists := m.pipes[sessionID]; exists {
		p.Close()
		delete(m.pipes, sessionID)
	}
}

// Count returns the number of active pipes.
func (m *RelayManager) Count() int {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return len(m.pipes)
}

// SweepIdlePipes closes and purges pipes that have exceeded the idle timeout.
func (m *RelayManager) SweepIdlePipes(timeout time.Duration, now time.Time) []string {
	m.mu.Lock()
	defer m.mu.Unlock()

	var swept []string
	for id, p := range m.pipes {
		if now.Sub(p.LastActive()) > timeout {
			p.Close()
			delete(m.pipes, id)
			swept = append(swept, id)
		}
	}
	return swept
}
