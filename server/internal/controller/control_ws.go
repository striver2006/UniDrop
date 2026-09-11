package controller

import (
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"time"

	"github.com/coder/websocket"
	"github.com/google/uuid"
	"github.com/unidrop/unidrop-server/internal/auth"
	"github.com/unidrop/unidrop-server/internal/protocol"
	"github.com/unidrop/unidrop-server/internal/registry"
)

// ControlWSHandler handles incoming WebSocket connections on /ws/control.
type ControlWSHandler struct {
	verifier *auth.Verifier
	registry *registry.DeviceRegistry
}

// NewControlWSHandler creates a new ControlWSHandler.
func NewControlWSHandler(v *auth.Verifier, reg *registry.DeviceRegistry) *ControlWSHandler {
	return &ControlWSHandler{
		verifier: v,
		registry: reg,
	}
}

func (h *ControlWSHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	ws, err := websocket.Accept(w, r, &websocket.AcceptOptions{
		InsecureSkipVerify: true, // Allow cross-origin for local development/clients
	})
	if err != nil {
		slog.Error("websocket accept failed", "error", err)
		return
	}
	defer ws.Close(websocket.StatusInternalError, "handler exiting")

	// Set read limit to 512KB for control signaling (P1-11)
	ws.SetReadLimit(512 * 1024)

	// Step 1: Send AUTH_CHALLENGE
	challengePayload, _ := json.Marshal(protocol.AuthChallengePayload{
		NonceSalt:  uuid.NewString(),
		ServerTime: time.Now().UnixMilli(),
	})
	challengeEnv := protocol.ControlEnvelope{
		Version:   1,
		TraceID:   uuid.NewString(),
		Action:    protocol.ActionAuthChallenge,
		Timestamp: time.Now().UnixMilli(),
		Payload:   challengePayload,
	}
	challengeBytes, _ := json.Marshal(challengeEnv)
	if err := ws.Write(ctx, websocket.MessageText, challengeBytes); err != nil {
		return
	}

	// Step 2: Read AUTH_REQUEST with 5s timeout
	authCtx, authCancel := context.WithTimeout(ctx, 5*time.Second)
	msgType, authBytes, err := ws.Read(authCtx)
	authCancel()
	if err != nil || msgType != websocket.MessageText {
		slog.Warn("auth handshake read failed or timed out", "error", err)
		return
	}

	var authEnv protocol.ControlEnvelope
	if err := json.Unmarshal(authBytes, &authEnv); err != nil || authEnv.Action != protocol.ActionAuthRequest {
		slog.Warn("invalid auth request envelope")
		return
	}

	var authReq protocol.AuthRequestPayload
	if err := json.Unmarshal(authEnv.Payload, &authReq); err != nil {
		slog.Warn("invalid auth payload")
		return
	}

	// Step 3: Verify HMAC Signature and Nonce
	now := time.Now()
	if err := h.verifier.Verify(&authReq, now); err != nil {
		slog.Warn("auth verification failed", "device", authReq.DeviceID, "error", err)
		respPayload, _ := json.Marshal(protocol.AuthResponsePayload{
			Success:      false,
			ErrorCode:    "UNAUTHORIZED",
			ErrorMessage: err.Error(),
		})
		respEnv, _ := json.Marshal(protocol.ControlEnvelope{
			Version:   1,
			TraceID:   authEnv.TraceID,
			Action:    protocol.ActionAuthResponse,
			Timestamp: time.Now().UnixMilli(),
			Payload:   respPayload,
		})
		_ = ws.Write(ctx, websocket.MessageText, respEnv)
		return
	}

	// Authentication Succeeded
	session := registry.NewDeviceSession(
		authReq.AccountID,
		authReq.DeviceID,
		authReq.Hostname,
		authReq.OSType,
		authReq.AppVersion,
		r.RemoteAddr,
		ws,
	)
	h.registry.Register(session)
	defer func() {
		h.registry.Unregister(session.DeviceID)
		// Broadcast offline event
		offlinePayload, _ := json.Marshal(protocol.DeviceOfflinePayload{
			DeviceID: session.DeviceID,
			Reason:   "connection disconnected",
		})
		offlineEnv, _ := json.Marshal(protocol.ControlEnvelope{
			Version:    1,
			TraceID:    uuid.NewString(),
			Action:     protocol.ActionDeviceOffline,
			FromDevice: session.DeviceID,
			Timestamp:  time.Now().UnixMilli(),
			Payload:    offlinePayload,
		})
		h.registry.BroadcastToAccount(session.AccountID, session.DeviceID, offlineEnv)
		slog.Info("device disconnected", "device", session.DeviceID)
	}()

	slog.Info("device authenticated", "device", session.DeviceID, "account", session.AccountID, "os", session.OSType)

	// Send AUTH_RESPONSE success
	authSuccessPayload, _ := json.Marshal(protocol.AuthResponsePayload{
		Success:    true,
		AssignedID: session.DeviceID,
	})
	authSuccessEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:   1,
		TraceID:   authEnv.TraceID,
		Action:    protocol.ActionAuthResponse,
		Timestamp: time.Now().UnixMilli(),
		Payload:   authSuccessPayload,
	})
	if err := ws.Write(ctx, websocket.MessageText, authSuccessEnv); err != nil {
		return
	}

	// Step 4: Send DEVICE_LIST_SYNC to this device
	allSessions := h.registry.ListByAccount(session.AccountID)
	var onlineDevices []protocol.OnlineDevice
	for _, s := range allSessions {
		if s.DeviceID != session.DeviceID {
			onlineDevices = append(onlineDevices, s.ToOnlineDevice())
		}
	}
	syncPayload, _ := json.Marshal(protocol.DeviceListSyncPayload{Devices: onlineDevices})
	syncEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:   1,
		TraceID:   uuid.NewString(),
		Action:    protocol.ActionDeviceListSync,
		Timestamp: time.Now().UnixMilli(),
		Payload:   syncPayload,
	})
	_ = ws.Write(ctx, websocket.MessageText, syncEnv)

	// Step 5: Broadcast DEVICE_ONLINE to peers
	onlinePayload, _ := json.Marshal(protocol.DeviceOnlinePayload{Device: session.ToOnlineDevice()})
	onlineEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    uuid.NewString(),
		Action:     protocol.ActionDeviceOnline,
		FromDevice: session.DeviceID,
		Timestamp:  time.Now().UnixMilli(),
		Payload:    onlinePayload,
	})
	h.registry.BroadcastToAccount(session.AccountID, session.DeviceID, onlineEnv)

	// Step 6: Start WritePump goroutine
	writeCtx, writeCancel := context.WithCancel(ctx)
	defer writeCancel()

	go func() {
		for {
			select {
			case <-writeCtx.Done():
				return
			case <-session.Closed:
				return
			case msg, ok := <-session.SendChan:
				if !ok {
					return
				}
				writeTimeoutCtx, cancel := context.WithTimeout(writeCtx, 5*time.Second)
				err := ws.Write(writeTimeoutCtx, websocket.MessageText, msg)
				cancel()
				if err != nil {
					slog.Warn("write to client failed", "device", session.DeviceID, "error", err)
					session.Close()
					return
				}
			}
		}
	}()

	// Step 7: ReadPump Loop
	for {
		msgType, msgBytes, err := ws.Read(ctx)
		if err != nil {
			break
		}
		if msgType != websocket.MessageText {
			continue
		}

		var env protocol.ControlEnvelope
		if err := json.Unmarshal(msgBytes, &env); err != nil {
			slog.Warn("malformed envelope from client", "device", session.DeviceID)
			continue
		}

		env.FromDevice = session.DeviceID // Force correct sender ID (P1-3 prevent spoofing)

		switch env.Action {
		case protocol.ActionHeartbeatPing:
			session.TouchPing(time.Now())
			pongEnv, _ := json.Marshal(protocol.ControlEnvelope{
				Version:   1,
				TraceID:   env.TraceID,
				Action:    protocol.ActionHeartbeatPong,
				Timestamp: time.Now().UnixMilli(),
			})
			session.Send(pongEnv)

		case protocol.ActionTransferOffer,
			protocol.ActionTransferAnswer,
			protocol.ActionTransferCancel,
			protocol.ActionTransferFailure,
			protocol.ActionTransferComplete,
			protocol.ActionClipboardInjected:

			if env.ToDevice == "" {
				continue
			}

			// Route to target peer
			if targetSession, ok := h.registry.Get(env.ToDevice); ok {
				// Verify target belongs to same account
				if targetSession.AccountID == session.AccountID {
					repacked, _ := json.Marshal(env)
					targetSession.Send(repacked)
				}
			} else {
				slog.Debug("target device offline for transfer message", "to_device", env.ToDevice)
			}
		}
	}
}
