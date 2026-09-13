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

	// ErrSessionIDConflict is returned when a session ID is already held by a
	// live authorization belonging to someone else, or to a different device
	// pair within the same account.
	ErrSessionIDConflict = errors.New("session id already in use by another authorization")

	// ErrAccountPipeLimit is returned when an account already holds its maximum
	// number of live relay pipes.
	ErrAccountPipeLimit = errors.New("account relay pipe limit reached")
)

// TeardownResult says why a teardown request was or was not honoured.
//
// This is an enum rather than a bool because the caller logs the three outcomes
// at different levels, and deciding between them afterwards used to require a
// second, separate lock acquisition (OwnerOfSession). Between the two locks a
// new authorization could claim the same session id, which turned a perfectly
// benign late signal into a "cross-account teardown blocked" warning — exactly
// the noise the level split exists to avoid. Resolving the reason while still
// holding the lock removes the race and the second lookup together.
type TeardownResult int

const (
	// TeardownOK: the pipe (and its authorization) were removed.
	TeardownOK TeardownResult = iota
	// TeardownNotFound: nothing to remove. A duplicate or late terminal signal
	// arriving after the idle sweep already reclaimed the pipe — normal traffic.
	TeardownNotFound
	// TeardownDenied: the session exists but does not belong to the requester.
	// This is the abuse the ownership check was added for.
	TeardownDenied
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
//
// Both maps are keyed on the bare SessionID, and that is a deliberate contrast
// with registry.DeviceRegistry, which was changed to a composite
// (account, device) key. The two identifiers are not the same kind of thing.
// A device_id is user-visible and collides across accounts as a matter of
// course — two people really do both own a "MacBook" — so partitioning it is
// right. A session_id is a UUIDv4 minted per transfer; a collision there is not
// normal traffic but either ID reuse or someone probing, and partitioning would
// silently swallow that signal instead of surfacing it. The data plane also has
// no account in its URL (see controller.DataWSHandler), so a composite key here
// would force adding a client-reported account_id whose only validation would be
// the token we already check — strictly worse. What was missing was never the
// key shape but authorization on writes and deletes, and those are handled
// explicitly below.
type RelayManager struct {
	mu            sync.RWMutex
	pipes         map[string]*RelayPipe   // key: SessionID
	authSessions  map[string]*SessionAuth // key: SessionID
	bufferPool    *BufferPool
	ackBufferPool *AckBufferPool

	// maxPipesPerAccount bounds live pipes per account; <= 0 disables it.
	maxPipesPerAccount int
}

// NewRelayManager initializes a new RelayManager.
//
// maxPipesPerAccount is required rather than optional: leaving a zero-argument
// constructor in place would leave a way to build an unbounded manager, and the
// default path has to be the safe one. Callers pass the same value they use for
// transfer concurrency — see ValidateAndGetOrCreatePipe for why one number
// governs both.
func NewRelayManager(maxPipesPerAccount int) *RelayManager {
	return &RelayManager{
		pipes:              make(map[string]*RelayPipe),
		authSessions:       make(map[string]*SessionAuth),
		bufferPool:         NewBufferPool(),
		ackBufferPool:      NewAckBufferPool(),
		maxPipesPerAccount: maxPipesPerAccount,
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

	now := time.Now()

	// Ownership check first, before counting. The order is not cosmetic:
	// countAuthorizedForAccountLocked excludes "this session" by ID alone,
	// without looking at whose it is. Establishing that any entry under this ID
	// belongs to this account is what makes that exclusion sound. Reverse the
	// two and an account that reuses someone else's session ID gets a free pass
	// on its own quota, because the entry it is colliding with is the one being
	// excluded from the count.
	//
	// Expired entries are overwritten freely. They are dead capacity —
	// countAuthorizedForAccountLocked already ignores them — and reporting a
	// conflict against one would let a stale ID poison a key for the rest of
	// its five minute TTL for no benefit.
	//
	// The device pair is compared even within one account because the only
	// legitimate re-authorization is the receiver re-sending its ANSWER, and
	// that keeps both endpoints identical. A changed pair means either session
	// ID reuse or one device inside the account trying to attach itself to
	// another's transfer.
	if existing, ok := m.authSessions[sessionID]; ok && m.authLiveLocked(sessionID, existing, now) {
		if existing.AccountID != accountID ||
			existing.SenderDevice != senderDevice ||
			existing.ReceiverDevice != receiverDevice {
			return "", 0, ErrSessionIDConflict
		}
	}

	inFlight := m.countAuthorizedForAccountLocked(accountID, sessionID, now)
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
		ExpiresAt:      now.Add(ttl),
	}

	return token, inFlight, nil
}

// authLiveLocked reports whether an authorization entry still binds anything.
//
// An entry is live while it has not expired, OR while the pipe it authorized is
// still alive. The second half is the whole point: ExpiresAt answers "how much
// longer may this ticket open a NEW pipe", and a transfer that has been
// streaming for longer than the TTL has an entry that is past that deadline yet
// is still governing an active pipe.
//
// Keeping such an entry in the map (see SweepIdlePipes) is not by itself
// enough — that was the original mistake. Every predicate that asks "is this
// entry meaningful?" has to agree, otherwise the entry sits there inert:
// contributing nothing to the account's in-flight count, and — far worse —
// looking free to overwrite. An attacker could then re-authorize the victim's
// session id under their own account and use the resulting entry to tear down a
// transfer that was still running.
//
// Caller must hold m.mu.
func (m *RelayManager) authLiveLocked(sessionID string, auth *SessionAuth, now time.Time) bool {
	if now.Before(auth.ExpiresAt) {
		return true
	}
	_, pipeAlive := m.pipes[sessionID]
	return pipeAlive
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
		if m.authLiveLocked(id, auth, now) {
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
	if !exists || !m.authLiveLocked(sessionID, auth, time.Now()) {
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

	// authLiveLocked rather than a bare ExpiresAt check: a data-plane reconnect
	// in the middle of a transfer that has outlived the TTL must still be able
	// to reach its own pipe, or a long transfer dies on a blip. The token is
	// still verified below, so this widens nothing for anyone without it.
	auth, exists := m.authSessions[sessionID]
	if !exists || !m.authLiveLocked(sessionID, auth, time.Now()) {
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

	// Pipe lookup or creation.
	//
	// The existing-pipe hit MUST come before the capacity checks below. A pipe
	// that is already streaming has to remain reachable when its account is at
	// the cap, or a data-plane reconnect mid-transfer would be rejected by the
	// account's own limit — which the user experiences as a large transfer
	// dying partway through for no stated reason.
	if p, pipeExists := m.pipes[sessionID]; pipeExists {
		// Verify pipe ownership matches authorization
		if p.FromDevice != auth.SenderDevice || p.ToDevice != auth.ReceiverDevice {
			return nil, ErrUnauthorized
		}
		p.Touch()
		return p, nil
	}

	// Per-account capacity. This is a backstop rather than the main gate: the
	// primary refusal happens in AuthorizeSessionIfUnderLimit, at TRANSFER_ANSWER
	// time, where it can be reported to both peers through TRANSFER_FAILURE with
	// text a user can act on. Refusing here instead means closing the data-plane
	// socket, which the client can only render as a generic connection failure.
	//
	// It is kept anyway because the data plane is separately reachable: nothing
	// forces a client to have gone through control-plane authorization in this
	// process, and capacity must not depend on that assumption.
	//
	// The bound is deliberately the same number as the transfer concurrency
	// limit rather than a new environment variable. One authorized session
	// corresponds to exactly one pipe, so the two govern the same resource in
	// the same units; a second knob would only ever create the question of what
	// happens when they disagree.
	if m.maxPipesPerAccount > 0 && m.countPipesForAccountLocked(auth.AccountID) >= m.maxPipesPerAccount {
		return nil, ErrAccountPipeLimit
	}

	// Global cap, now the second line rather than the first.
	if len(m.pipes) >= MaxConcurrentPipes {
		return nil, ErrServerBusy
	}

	p := NewRelayPipe(sessionID, auth.AccountID, auth.SenderDevice, auth.ReceiverDevice)
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

	p := NewRelayPipe(sessionID, "", fromDevice, toDevice)
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

// RemovePipeForSession tears down a pipe on behalf of a participant, and
// reports whether the teardown was authorized.
//
// This replaced a RemovePipe(sessionID) that performed both deletes with no
// check of any kind. Because the session_id on a TRANSFER_COMPLETE / FAILURE /
// CANCEL is simply whatever the client typed into the payload, any
// authenticated client could tear down any transfer in the deployment — other
// accounts included — without holding a token or any other credential. That was
// strictly worse than the overwrite path guarded in
// AuthorizeSessionIfUnderLimit, which at least required minting a token.
//
// There are four cases and they are not collapsible:
//
//  1. Auth entry present, account and device both match -> tear down. The
//     ordinary path.
//  2. Auth entry present, ownership does not match -> refuse and warn. This is
//     the abuse this function exists to stop. The device is checked as well as
//     the account because the data plane has always pinned itself to exactly
//     these two roles (see ValidateAndGetOrCreatePipe); leaving the metadata
//     plane looser than the data plane has no justification, and both values
//     are already sitting in SessionAuth, so the check is free.
//  3. No auth entry and no pipe -> nothing to do, report false, log at debug.
//     Deliberately NOT a warning: a late or duplicate terminal signal arriving
//     after the idle sweep already reclaimed the pipe is completely normal, and
//     warning on it would bury the case 2 warnings under noise from honest
//     transfers. This is also why the refusal must not be read as "attack
//     detected" by anything downstream.
//  4. No auth entry but the pipe is alive -> fall back to the ownership data
//     carried on the pipe itself. With auth entries now held open for as long
//     as their pipe lives (see SweepIdlePipes) this should be unreachable, but
//     it is kept as a backstop so that ownership enforcement does not depend on
//     that coupling never regressing.
//
// Note what is deliberately not done: treating an expired entry as absent and
// letting the teardown through. That would reopen the very hole this closes,
// just on a five minute delay.
func (m *RelayManager) RemovePipeForSession(sessionID, accountID, deviceID string) (TeardownResult, string) {
	m.mu.Lock()
	defer m.mu.Unlock()

	p, pipeExists := m.pipes[sessionID]

	auth, authExists := m.authSessions[sessionID]
	if !authExists {
		if !pipeExists {
			return TeardownNotFound, "" // case 3
		}
		// case 4
		if p.AccountID != accountID || (deviceID != p.FromDevice && deviceID != p.ToDevice) {
			return TeardownDenied, p.AccountID
		}
		p.Close()
		delete(m.pipes, sessionID)
		return TeardownOK, ""
	}

	// cases 1 and 2
	if auth.AccountID != accountID {
		return TeardownDenied, auth.AccountID
	}
	if deviceID != auth.SenderDevice && deviceID != auth.ReceiverDevice {
		return TeardownDenied, auth.AccountID
	}

	if pipeExists {
		p.Close()
		delete(m.pipes, sessionID)
	}
	delete(m.authSessions, sessionID)
	return TeardownOK, ""
}

// MaxPipesInUsePerAccount returns the live pipe count of the busiest account.
func (m *RelayManager) MaxPipesInUsePerAccount() int {
	m.mu.RLock()
	defer m.mu.RUnlock()

	perAccount := make(map[string]int, len(m.pipes))
	max := 0
	for _, p := range m.pipes {
		perAccount[p.AccountID]++
		if n := perAccount[p.AccountID]; n > max {
			max = n
		}
	}
	return max
}

// countPipesForAccountLocked counts an account's live pipes. Caller holds m.mu.
func (m *RelayManager) countPipesForAccountLocked(accountID string) int {
	count := 0
	for _, p := range m.pipes {
		if p.AccountID == accountID {
			count++
		}
	}
	return count
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

	// Also purge expired auth sessions — but never one whose pipe is still
	// alive.
	//
	// ExpiresAt answers "how much longer may this ticket be used to open a new
	// pipe", not "how much longer may that pipe live". Those were conflated
	// before, and because the auth TTL is five minutes while a pipe survives on
	// sixty seconds of activity, any honest transfer running longer than five
	// minutes would lose its authorization record while still streaming. Two
	// things broke as a result: the terminal signal at the end of that transfer
	// found no entry and was refused as an unauthorized teardown, and
	// countAuthorizedForAccountLocked stopped counting the transfer as in
	// flight, so the account appeared to have free quota it did not have and the
	// refusal moved from TRANSFER_ANSWER down to the data plane where it cannot
	// be explained to the user.
	for id, auth := range m.authSessions {
		if !m.authLiveLocked(id, auth, now) {
			delete(m.authSessions, id)
		}
	}

	return swept
}
