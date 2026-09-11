package auth

import (
	"sync"
	"time"
)

// NonceCache stores seen nonces within a sliding expiration window to prevent replay attacks.
type NonceCache struct {
	mu     sync.Mutex
	items  map[string]time.Time
	window time.Duration
}

// NewNonceCache creates a new NonceCache with the given TTL window.
func NewNonceCache(window time.Duration) *NonceCache {
	nc := &NonceCache{
		items:  make(map[string]time.Time),
		window: window,
	}
	return nc
}

// CheckAndAdd returns true if the nonce is new and adds it to cache.
// Returns false if the nonce already exists within the active window.
func (nc *NonceCache) CheckAndAdd(nonce string, now time.Time) bool {
	nc.mu.Lock()
	defer nc.mu.Unlock()

	// Lazy cleanup of expired nonces if cache grows
	if len(nc.items) > 1000 {
		nc.cleanup(now)
	}

	if exp, exists := nc.items[nonce]; exists {
		if now.Before(exp) {
			return false // Replay detected!
		}
	}

	nc.items[nonce] = now.Add(nc.window)
	return true
}

func (nc *NonceCache) cleanup(now time.Time) {
	for k, exp := range nc.items {
		if now.After(exp) {
			delete(nc.items, k)
		}
	}
}
