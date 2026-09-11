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

// BuildCanonicalStringWithSalt generates canonical string incorporating challenge NonceSalt.
func BuildCanonicalStringWithSalt(accountID, deviceID, nonce string, timestampMs int64, nonceSalt string) string {
	if nonceSalt == "" {
		return BuildCanonicalString(accountID, deviceID, nonce, timestampMs)
	}
	return fmt.Sprintf("UNIDROP_V1\n%s\n%s\n%s\n%s\n%s", accountID, deviceID, nonce, strconv.FormatInt(timestampMs, 10), nonceSalt)
}

// GenerateSignature generates the expected hex-encoded HMAC-SHA256 signature without salt.
func (v *Verifier) GenerateSignature(accountID, deviceID, nonce string, timestampMs int64) string {
	return v.GenerateSignatureWithSalt(accountID, deviceID, nonce, timestampMs, "")
}

// GenerateSignatureWithSalt generates the expected hex-encoded HMAC-SHA256 signature with challenge salt.
func (v *Verifier) GenerateSignatureWithSalt(accountID, deviceID, nonce string, timestampMs int64, nonceSalt string) string {
	mac := hmac.New(sha256.New, v.pskSecret)
	mac.Write([]byte(BuildCanonicalStringWithSalt(accountID, deviceID, nonce, timestampMs, nonceSalt)))
	return hex.EncodeToString(mac.Sum(nil))
}

// Verify checks the validity of an AuthRequestPayload against the configured PSK.
func (v *Verifier) Verify(req *protocol.AuthRequestPayload, now time.Time) error {
	return v.VerifyWithSalt(req, now, "")
}

// VerifyWithSalt checks the validity of AuthRequestPayload and NonceSalt (P2-1).
// Crucially, it verifies the cryptographic signature FIRST, and only records the nonce if valid.
func (v *Verifier) VerifyWithSalt(req *protocol.AuthRequestPayload, now time.Time, nonceSalt string) error {
	// 1. Validate timestamp within allowed skew
	reqTime := time.UnixMilli(req.Timestamp)
	diff := now.Sub(reqTime)
	if diff < -v.maxSkew || diff > v.maxSkew {
		return fmt.Errorf("%w: diff=%v", ErrTimestampOutOfRange, diff)
	}

	// 2. Verify signature in constant time FIRST (P2-1: never burn nonce on bad signature)
	expectedSigWithSalt := v.GenerateSignatureWithSalt(req.AccountID, req.DeviceID, req.Nonce, req.Timestamp, nonceSalt)
	sigValid := subtle.ConstantTimeCompare([]byte(req.Signature), []byte(expectedSigWithSalt)) == 1

	if !sigValid {
		return ErrInvalidSignature
	}

	// 3. Check and record nonce replay ONLY after signature is confirmed authentic
	if !v.nonceCache.CheckAndAdd(req.Nonce, now) {
		return fmt.Errorf("%w: nonce=%s", ErrReplayDetected, req.Nonce)
	}

	return nil
}
