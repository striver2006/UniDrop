package controller_test

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/unidrop/unidrop-server/internal/auth"
	"github.com/unidrop/unidrop-server/internal/controller"
	"github.com/unidrop/unidrop-server/internal/registry"
	"github.com/unidrop/unidrop-server/internal/relay"
)

func TestDataWSUnauthorizedAccessRejected(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	psk := "test-secret"
	verifier, _ := auth.NewVerifier(psk)
	reg := registry.NewDeviceRegistry()
	relayMgr := relay.NewRelayManager()

	mux := http.NewServeMux()
	mux.Handle("GET /ws/control", controller.NewControlWSHandler(verifier, reg, relayMgr))
	mux.Handle("GET /ws/data", controller.NewDataWSHandler(relayMgr))

	server := httptest.NewServer(mux)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http")

	// 1. Try to connect to /ws/data with random invalid session and token
	dataURL := wsURL + "/ws/data?session_id=fake-session&role=sender&device_id=devX&token=bad-token"
	_, _, err := websocket.Dial(ctx, dataURL, nil)
	if err == nil {
		t.Fatal("expected unauthorized data websocket connection to fail")
	}

	// 2. Try to connect to /ws/data with empty/missing token (R1: must fail unconditionally)
	emptyTokenURL := wsURL + "/ws/data?session_id=fake-session&role=sender&device_id=devX"
	_, resp, err := websocket.Dial(ctx, emptyTokenURL, nil)
	if err == nil {
		t.Fatal("expected empty token connection to be rejected")
	}
	if resp != nil && resp.StatusCode != http.StatusForbidden {
		t.Fatalf("expected 403 Forbidden for missing token, got %d", resp.StatusCode)
	}
}

func TestDataWSInvalidMagicFrameDisconnected(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	relayMgr := relay.NewRelayManager()

	// Pre-authorize a session
	token, _ := relayMgr.AuthorizeSession("valid-session", "devA", "devB", "user1", 1*time.Minute)

	mux := http.NewServeMux()
	mux.Handle("GET /ws/data", controller.NewDataWSHandler(relayMgr))

	server := httptest.NewServer(mux)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http")
	dataURL := wsURL + "/ws/data?session_id=valid-session&role=sender&device_id=devA&token=" + token

	ws, _, err := websocket.Dial(ctx, dataURL, nil)
	if err != nil {
		t.Fatalf("failed to dial: %v", err)
	}
	defer ws.Close(websocket.StatusNormalClosure, "done")

	// Send garbage binary frame (invalid magic)
	garbage := make([]byte, 100)
	copy(garbage, []byte("INVALID_MAGIC_HEADER_GARBAGE"))
	_ = ws.Write(ctx, websocket.MessageBinary, garbage)

	// Server must close connection with StatusPolicyViolation
	_, _, err = ws.Read(ctx)
	if err == nil {
		t.Fatal("expected server to disconnect on invalid magic frame")
	}
}
