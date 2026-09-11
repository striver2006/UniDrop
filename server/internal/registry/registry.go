package registry

import (
	"sync"
	"time"
)

// DeviceRegistry provides a thread-safe in-memory store for active device sessions.
type DeviceRegistry struct {
	mu       sync.RWMutex
	sessions map[string]*DeviceSession // key: DeviceID
}

// NewDeviceRegistry creates a new DeviceRegistry.
func NewDeviceRegistry() *DeviceRegistry {
	return &DeviceRegistry{
		sessions: make(map[string]*DeviceSession),
	}
}

// Register adds or updates a device session. If a previous session exists for the same DeviceID,
// it is gracefully closed and replaced.
func (r *DeviceRegistry) Register(s *DeviceSession) {
	r.mu.Lock()
	defer r.mu.Unlock()

	if old, exists := r.sessions[s.DeviceID]; exists {
		old.Close()
	}
	r.sessions[s.DeviceID] = s
}

// Unregister removes a device session by DeviceID and closes it.
func (r *DeviceRegistry) Unregister(deviceID string) {
	r.mu.Lock()
	defer r.mu.Unlock()

	if s, exists := r.sessions[deviceID]; exists {
		s.Close()
		delete(r.sessions, deviceID)
	}
}

// Get retrieves a session by DeviceID.
func (r *DeviceRegistry) Get(deviceID string) (*DeviceSession, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()

	s, exists := r.sessions[deviceID]
	return s, exists
}

// ListByAccount returns all active sessions for a given account.
func (r *DeviceRegistry) ListByAccount(accountID string) []*DeviceSession {
	r.mu.RLock()
	defer r.mu.RUnlock()

	var result []*DeviceSession
	for _, s := range r.sessions {
		if s.AccountID == accountID {
			result = append(result, s)
		}
	}
	return result
}

// BroadcastToAccount sends a message to all active devices of an account, optionally excluding one device.
func (r *DeviceRegistry) BroadcastToAccount(accountID, excludeDeviceID string, data []byte) {
	r.mu.RLock()
	defer r.mu.RUnlock()

	for _, s := range r.sessions {
		if s.AccountID == accountID && s.DeviceID != excludeDeviceID {
			s.Send(data)
		}
	}
}

// Count returns the total number of connected devices.
func (r *DeviceRegistry) Count() int {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return len(r.sessions)
}

// SweepInactive checks for devices that have not sent a heartbeat within the timeout period.
// It unregisters them and returns their IDs along with their AccountIDs.
func (r *DeviceRegistry) SweepInactive(timeout time.Duration, now time.Time) []struct {
	AccountID string
	DeviceID  string
} {
	r.mu.Lock()
	defer r.mu.Unlock()

	var timedOut []struct {
		AccountID string
		DeviceID  string
	}

	for id, s := range r.sessions {
		if now.Sub(s.LastPingAt) > timeout {
			timedOut = append(timedOut, struct {
				AccountID string
				DeviceID  string
			}{
				AccountID: s.AccountID,
				DeviceID:  s.DeviceID,
			})
			s.Close()
			delete(r.sessions, id)
		}
	}

	return timedOut
}
