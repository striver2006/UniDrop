package registry

import (
	"sync"
	"time"
)

// sessionKey identifies a live connection by the pair that actually has to be
// unique, rather than by DeviceID alone.
//
// The registry used to be keyed on the bare DeviceID, and Register would close
// whatever it found under that key before storing the new session. Two accounts
// that happened to pick the same device_id — two machines both called "MacBook",
// or one config directory copied to a colleague — would therefore kick each
// other offline, and a client could do it on purpose by claiming a device_id it
// had seen. Putting AccountID in the key makes "taking over a device slot" an
// account-internal affair, which is what it always should have been.
//
// The alternative considered was keeping the flat key and comparing AccountID at
// each call site. That was rejected: the map would go on claiming that DeviceID
// is globally unique when it is not, and every future method added to this type
// would have to remember to compare again. This is the same reasoning
// countAuthorizedForAccountLocked uses to refuse a hand-maintained counter —
// more ways to be silently wrong than it is worth.
//
// This structure assumes AccountID is non-empty and already normalised. That is
// guaranteed by auth.ValidateAccountID at the handshake; remove the validation
// and every client that forgot to set an account lands back in one shared
// bucket, which is exactly the defect the key change was made to close.
type sessionKey struct {
	AccountID string
	DeviceID  string
}

func keyOf(s *DeviceSession) sessionKey {
	return sessionKey{AccountID: s.AccountID, DeviceID: s.DeviceID}
}

// DeviceRegistry provides a thread-safe in-memory store for active device sessions.
type DeviceRegistry struct {
	mu       sync.RWMutex
	sessions map[sessionKey]*DeviceSession
}

// NewDeviceRegistry creates a new DeviceRegistry.
func NewDeviceRegistry() *DeviceRegistry {
	return &DeviceRegistry{
		sessions: make(map[sessionKey]*DeviceSession),
	}
}

// Register adds or updates a device session. If a previous session exists for the
// same (account, device) pair, it is gracefully closed and replaced.
//
// Replacing on reconnect is intentional and unchanged: a device that drops and
// comes back should supersede its own stale connection. What changed is that the
// match now requires the account to agree, so the takeover can no longer reach
// across accounts.
func (r *DeviceRegistry) Register(s *DeviceSession) {
	r.mu.Lock()
	defer r.mu.Unlock()

	k := keyOf(s)
	if old, exists := r.sessions[k]; exists {
		old.Close()
	}
	r.sessions[k] = s
}

// Unregister removes a device session by (accountID, deviceID) and closes it.
func (r *DeviceRegistry) Unregister(accountID, deviceID string) {
	r.mu.Lock()
	defer r.mu.Unlock()

	k := sessionKey{AccountID: accountID, DeviceID: deviceID}
	if s, exists := r.sessions[k]; exists {
		s.Close()
		delete(r.sessions, k)
	}
}

// UnregisterSession performs a compare-and-delete (P0-5): it only unregisters and closes
// the session if the session currently registered under s's key is precisely s.
// Returns true if the session was matched and unregistered.
func (r *DeviceRegistry) UnregisterSession(s *DeviceSession) bool {
	if s == nil {
		return false
	}
	r.mu.Lock()
	defer r.mu.Unlock()

	k := keyOf(s)
	if current, exists := r.sessions[k]; exists && current == s {
		s.Close()
		delete(r.sessions, k)
		return true
	}
	return false
}

// Get retrieves a session by (accountID, deviceID).
//
// Callers no longer need to compare AccountID afterwards — a lookup scoped to
// the wrong account simply misses. One consequence is deliberate and worth
// stating: "this device belongs to another account" and "this device is not
// connected" are now the same answer. Telling them apart would leak the
// existence of another account's device into the caller (and into its logs),
// which is not information this account is entitled to.
func (r *DeviceRegistry) Get(accountID, deviceID string) (*DeviceSession, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()

	s, exists := r.sessions[sessionKey{AccountID: accountID, DeviceID: deviceID}]
	return s, exists
}

// ListByAccount returns all active sessions for a given account.
//
// The linear scan is deliberate. Session count is bounded by the heartbeat
// sweep and sits in the tens-to-hundreds range, so a secondary index would buy
// nothing and add an invariant to keep correct on every insert and delete. If
// that ever changes, the index can be added without touching this signature.
func (r *DeviceRegistry) ListByAccount(accountID string) []*DeviceSession {
	r.mu.RLock()
	defer r.mu.RUnlock()

	var result []*DeviceSession
	for k, s := range r.sessions {
		if k.AccountID == accountID {
			result = append(result, s)
		}
	}
	return result
}

// BroadcastToAccount sends a message to all active devices of an account, optionally excluding one device.
func (r *DeviceRegistry) BroadcastToAccount(accountID, excludeDeviceID string, data []byte) {
	r.mu.RLock()
	defer r.mu.RUnlock()

	for k, s := range r.sessions {
		if k.AccountID == accountID && k.DeviceID != excludeDeviceID {
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

// CountAccounts returns the number of distinct accounts with at least one
// connected device.
func (r *DeviceRegistry) CountAccounts() int {
	r.mu.RLock()
	defer r.mu.RUnlock()

	seen := make(map[string]struct{}, len(r.sessions))
	for k := range r.sessions {
		seen[k.AccountID] = struct{}{}
	}
	return len(seen)
}

// MaxDevicesPerAccount returns the device count of the single busiest account.
//
// This exists so that /metrics can answer "is one account eating the box?" with
// one number instead of one time series per account — see the comment on
// MetricsHandler for why a per-account label is not an option.
func (r *DeviceRegistry) MaxDevicesPerAccount() int {
	r.mu.RLock()
	defer r.mu.RUnlock()

	perAccount := make(map[string]int, len(r.sessions))
	max := 0
	for k := range r.sessions {
		perAccount[k.AccountID]++
		if n := perAccount[k.AccountID]; n > max {
			max = n
		}
	}
	return max
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

	for k, s := range r.sessions {
		if now.Sub(s.GetLastPing()) > timeout {
			timedOut = append(timedOut, struct {
				AccountID string
				DeviceID  string
			}{
				AccountID: k.AccountID,
				DeviceID:  k.DeviceID,
			})
			s.Close()
			delete(r.sessions, k)
		}
	}

	return timedOut
}
