package relay

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
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
	ErrServerBusy     = errors.New("server relay capacity reached, please retry later")
	ErrPipeNotFound   = errors.New("relay pipe expired or not found")
	ErrUnauthorized   = errors.New("unauthorized data session")
	ErrDeviceMismatch = errors.New("device does not match authorized session participant")

	// ErrConcurrencyLimit is returned when an account already has the maximum
	// number of authorized-but-unfinished transfers.
	ErrConcurrencyLimit = errors.New("account transfer concurrency limit reached")
)

// SessionAuth represents authorization for an upcoming data plane session.
type SessionAuth struct {
	SessionID      string
	SenderDevice   string
	ReceiverDevice string
	AccountID      string
	Token          string
	ExpiresAt      time.Time
}

// RelayManager manages active streaming data pipes and the shared buffer pool.
type RelayManager struct {
	mu            sync.RWMutex
	pipes         map[string]*RelayPipe   // key: SessionID
	authSessions  map[string]*SessionAuth // key: SessionID
	bufferPool    *BufferPool
	ackBufferPool *AckBufferPool
}

// NewRelayManager initializes a new RelayManager.
func NewRelayManager() *RelayManager {
	return &RelayManager{
		pipes:         make(map[string]*RelayPipe),
		authSessions:  make(map[string]*SessionAuth),
		bufferPool:    NewBufferPool(),
		ackBufferPool: NewAckBufferPool(),
	}
}

// BufferPool returns the shared buffer pool.
func (m *RelayManager) BufferPool() *BufferPool {
	return m.bufferPool
}

// AckBufferPool returns the small buffer pool for ACK/NACK frames.
func (m *RelayManager) AckBufferPool() *AckBufferPool {
	return m.ackBufferPool
}

// AuthorizeSession generates a secure token for an agreed transfer session (P0-1, P2-7),
// without applying any per-account concurrency limit.
func (m *RelayManager) AuthorizeSession(sessionID, senderDevice, receiverDevice, accountID string, ttl time.Duration) (string, error) {
	token, _, err := m.AuthorizeSessionIfUnderLimit(sessionID, senderDevice, receiverDevice, accountID, ttl, 0)
	return token, err
}

// AuthorizeSessionIfUnderLimit counts the account's in-flight transfers,
// refuses when that count has reached maxConcurrent, and otherwise mints a
// token — all three inside a single write lock.
//
// The atomicity is the point. Counting under RLock and then authorizing under
// a separate Lock leaves a window where two connections of the same account
// both observe "one slot left", both pass, and both get authorized. Each
// control connection runs its own read loop goroutine, so a single client
// driving several devices — precisely what this limit exists to contain —
// hits that window rather than merely being able to. The global pipe cap is a
// distant second line, not a substitute.
//
// maxConcurrent <= 0 disables the check.
//
// Returns the minted token and the in-flight count observed. On refusal it
// returns ErrConcurrencyLimit along with that count, so the caller can tell
// the user how many transfers are already running.
func (m *RelayManager) AuthorizeSessionIfUnderLimit(
	sessionID, senderDevice, receiverDevice, accountID string,
	ttl time.Duration,
	maxConcurrent int,
) (string, int, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	inFlight := m.countAuthorizedForAccountLocked(accountID, sessionID, time.Now())
	if maxConcurrent > 0 && inFlight >= maxConcurrent {
		return "", inFlight, ErrConcurrencyLimit
	}

	tokenBytes := make([]byte, 24)
	if _, err := rand.Read(tokenBytes); err != nil {
		return "", inFlight, err
	}
	token := hex.EncodeToString(tokenBytes)

	m.authSessions[sessionID] = &SessionAuth{
		SessionID:      sessionID,
		SenderDevice:   senderDevice,
		ReceiverDevice: receiverDevice,
		AccountID:      accountID,
		Token:          token,
		ExpiresAt:      time.Now().Add(ttl),
	}

	return token, inFlight, nil
}

// countAuthorizedForAccountLocked counts an account's transfers that are
// authorized and not yet finished. The caller must hold m.mu.
//
// authSessions is the right set to count, not pipes: a pipe is only created
// once both peers have connected to /ws/data, so counting pipes would miss
// every transfer that has been granted a token but has not started moving
// bytes — exactly the window a client would sit in while opening many
// transfers at once. Entries leave this map through RemovePipe (on
// COMPLETE/FAILURE/CANCEL), through SweepIdlePipes, or by expiry.
//
// excludeSessionID keeps a re-authorization of an already-tracked session from
// counting itself and being refused its own retry.
//
// The linear scan is deliberate: the map is bounded by the 5 minute TTL and
// swept every 10 seconds, so it holds tens of entries and a scan costs
// microseconds. A per-account counter would be O(1) but adds an invariant that
// has to be decremented correctly on all three removal paths — more ways to be
// silently wrong than this is worth.
func (m *RelayManager) countAuthorizedForAccountLocked(accountID, excludeSessionID string, now time.Time) int {
	count := 0
	for id, auth := range m.authSessions {
		if id == excludeSessionID || auth.AccountID != accountID {
			continue
		}
		if now.Before(auth.ExpiresAt) {
			count++
		}
	}
	return count
}

// HasAuthSessions returns whether any authorization sessions are tracked.
func (m *RelayManager) HasAuthSessions() bool {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return len(m.authSessions) > 0
}

// CheckAuthorization verifies token and roles before WebSocket upgrade (P0-1).
func (m *RelayManager) CheckAuthorization(sessionID, role, deviceID, targetDeviceID, token string) error {
	m.mu.RLock()
	defer m.mu.RUnlock()

	auth, exists := m.authSessions[sessionID]
	if !exists || time.Now().After(auth.ExpiresAt) {
		return ErrUnauthorized
	}

	if subtle.ConstantTimeCompare([]byte(auth.Token), []byte(token)) != 1 {
		return ErrUnauthorized
	}

	if role == "sender" {
		if deviceID != auth.SenderDevice {
			return ErrDeviceMismatch
		}
		if targetDeviceID != "" && targetDeviceID != auth.ReceiverDevice {
			return ErrDeviceMismatch
		}
	} else if role == "receiver" {
		if deviceID != auth.ReceiverDevice {
			return ErrDeviceMismatch
		}
		if targetDeviceID != "" && targetDeviceID != auth.SenderDevice {
			return ErrDeviceMismatch
		}
	} else {
		return ErrUnauthorized
	}

	return nil
}

// ValidateAndGetOrCreatePipe authenticates the data plane connection and acquires/creates the pipe (P0-1, P2-7).
func (m *RelayManager) ValidateAndGetOrCreatePipe(sessionID, role, deviceID, targetDeviceID, token string) (*RelayPipe, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	auth, exists := m.authSessions[sessionID]
	if !exists || time.Now().After(auth.ExpiresAt) {
		return nil, ErrUnauthorized
	}

	if subtle.ConstantTimeCompare([]byte(auth.Token), []byte(token)) != 1 {
		return nil, ErrUnauthorized
	}

	// Validate role and device mapping
	if role == "sender" {
		if deviceID != auth.SenderDevice {
			return nil, ErrDeviceMismatch
		}
		if targetDeviceID != "" && targetDeviceID != auth.ReceiverDevice {
			return nil, ErrDeviceMismatch
		}
	} else if role == "receiver" {
		if deviceID != auth.ReceiverDevice {
			return nil, ErrDeviceMismatch
		}
		if targetDeviceID != "" && targetDeviceID != auth.SenderDevice {
			return nil, ErrDeviceMismatch
		}
	} else {
		return nil, ErrUnauthorized
	}

	// Pipe lookup or creation
	if p, pipeExists := m.pipes[sessionID]; pipeExists {
		// Verify pipe ownership matches authorization
		if p.FromDevice != auth.SenderDevice || p.ToDevice != auth.ReceiverDevice {
			return nil, ErrUnauthorized
		}
		p.Touch()
		return p, nil
	}

	if len(m.pipes) >= MaxConcurrentPipes {
		return nil, ErrServerBusy
	}

	p := NewRelayPipe(sessionID, auth.SenderDevice, auth.ReceiverDevice)
	m.pipes[sessionID] = p
	return p, nil
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

// RemovePipe removes a pipe by sessionID and closes it (P2-12).
func (m *RelayManager) RemovePipe(sessionID string) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if p, exists := m.pipes[sessionID]; exists {
		p.Close()
		delete(m.pipes, sessionID)
	}
	delete(m.authSessions, sessionID)
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
			delete(m.authSessions, id)
			swept = append(swept, id)
		}
	}

	// Also purge expired auth sessions
	for id, auth := range m.authSessions {
		if now.After(auth.ExpiresAt) {
			delete(m.authSessions, id)
		}
	}

	return swept
}
