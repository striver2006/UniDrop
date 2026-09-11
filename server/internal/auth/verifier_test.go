package auth

import (
	"testing"
	"time"

	"github.com/unidrop/unidrop-server/internal/protocol"
)

func TestVerifierSuccess(t *testing.T) {
	secret := "super-secure-psk-secret-key-123456"
	v, err := NewVerifier(secret)
	if err != nil {
		t.Fatalf("failed to create verifier: %v", err)
	}

	now := time.Now()
	timestamp := now.UnixMilli()
	nonce := "random_nonce_12345"
	sig := v.GenerateSignature("user_a", "device_mac", nonce, timestamp)

	req := &protocol.AuthRequestPayload{
		AccountID: "user_a",
		DeviceID:  "device_mac",
		Nonce:     nonce,
		Timestamp: timestamp,
		Signature: sig,
	}

	if err := v.Verify(req, now); err != nil {
		t.Fatalf("expected verification success, got: %v", err)
	}
}

func TestVerifierReplayAttack(t *testing.T) {
	secret := "super-secure-psk-secret-key-123456"
	v, _ := NewVerifier(secret)

	now := time.Now()
	timestamp := now.UnixMilli()
	nonce := "replay_nonce_abc"
	sig := v.GenerateSignature("user_a", "device_mac", nonce, timestamp)

	req := &protocol.AuthRequestPayload{
		AccountID: "user_a",
		DeviceID:  "device_mac",
		Nonce:     nonce,
		Timestamp: timestamp,
		Signature: sig,
	}

	// First attempt succeeds
	if err := v.Verify(req, now); err != nil {
		t.Fatalf("first verification failed: %v", err)
	}

	// Second attempt with same nonce should be rejected
	if err := v.Verify(req, now); err == nil {
		t.Fatal("expected replay attack error on duplicate nonce, got nil")
	}
}

func TestVerifierTimestampSkew(t *testing.T) {
	secret := "super-secure-psk-secret-key-123456"
	v, _ := NewVerifier(secret)

	now := time.Now()
	// Skewed by 2 minutes into past
	timestamp := now.Add(-2 * time.Minute).UnixMilli()
	nonce := "nonce_skewed"
	sig := v.GenerateSignature("user_a", "device_mac", nonce, timestamp)

	req := &protocol.AuthRequestPayload{
		AccountID: "user_a",
		DeviceID:  "device_mac",
		Nonce:     nonce,
		Timestamp: timestamp,
		Signature: sig,
	}

	if err := v.Verify(req, now); err == nil {
		t.Fatal("expected timestamp skew error, got nil")
	}
}

func TestVerifierInvalidSignature(t *testing.T) {
	secret := "super-secure-psk-secret-key-123456"
	v, _ := NewVerifier(secret)

	now := time.Now()
	req := &protocol.AuthRequestPayload{
		AccountID: "user_a",
		DeviceID:  "device_mac",
		Nonce:     "nonce_random",
		Timestamp: now.UnixMilli(),
		Signature: "bad_signature_deadbeef",
	}

	if err := v.Verify(req, now); err == nil {
		t.Fatal("expected invalid signature error, got nil")
	}
}
