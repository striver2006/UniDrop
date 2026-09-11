package auth

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"errors"
	"fmt"
	"strconv"
	"time"

	"github.com/unidrop/unidrop-server/internal/protocol"
)

var (
	ErrEmptySecret         = errors.New("PSK secret is empty")
	ErrTimestampOutOfRange = errors.New("timestamp skewed by more than 60 seconds")
	ErrReplayDetected      = errors.New("nonce replay attack detected")
	ErrInvalidSignature    = errors.New("invalid HMAC signature")
)

// Verifier handles client authentication via HMAC-SHA256 and replay protection.
type Verifier struct {
	pskSecret  []byte
	nonceCache *NonceCache
	maxSkew    time.Duration
}

// NewVerifier creates a new Verifier instance.
func NewVerifier(pskSecret string) (*Verifier, error) {
	if len(pskSecret) == 0 {
		return nil, ErrEmptySecret
	}
	return &Verifier{
		pskSecret:  []byte(pskSecret),
		nonceCache: NewNonceCache(60 * time.Second),
		maxSkew:    60 * time.Second,
	}, nil
}

// BuildCanonicalString generates the exact string to be signed.
func BuildCanonicalString(accountID, deviceID, nonce string, timestampMs int64) string {
	return fmt.Sprintf("UNIDROP_V1\n%s\n%s\n%s\n%s", accountID, deviceID, nonce, strconv.FormatInt(timestampMs, 10))
}

// GenerateSignature generates the expected hex-encoded HMAC-SHA256 signature.
func (v *Verifier) GenerateSignature(accountID, deviceID, nonce string, timestampMs int64) string {
	mac := hmac.New(sha256.New, v.pskSecret)
	mac.Write([]byte(BuildCanonicalString(accountID, deviceID, nonce, timestampMs)))
	return hex.EncodeToString(mac.Sum(nil))
}

// Verify checks the validity of an AuthRequestPayload against the configured PSK.
func (v *Verifier) Verify(req *protocol.AuthRequestPayload, now time.Time) error {
	// 1. Validate timestamp within allowed skew
	reqTime := time.UnixMilli(req.Timestamp)
	diff := now.Sub(reqTime)
	if diff < -v.maxSkew || diff > v.maxSkew {
		return fmt.Errorf("%w: diff=%v", ErrTimestampOutOfRange, diff)
	}

	// 2. Check nonce replay
	if !v.nonceCache.CheckAndAdd(req.Nonce, now) {
		return fmt.Errorf("%w: nonce=%s", ErrReplayDetected, req.Nonce)
	}

	// 3. Verify signature in constant time
	expectedSig := v.GenerateSignature(req.AccountID, req.DeviceID, req.Nonce, req.Timestamp)
	if subtle.ConstantTimeCompare([]byte(req.Signature), []byte(expectedSig)) != 1 {
		return ErrInvalidSignature
	}

	return nil
}
