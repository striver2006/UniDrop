package controller

import (
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"time"

	"github.com/coder/websocket"
	"github.com/google/uuid"
	"github.com/unidrop/unidrop-server/internal/auth"
	"github.com/unidrop/unidrop-server/internal/limits"
	"github.com/unidrop/unidrop-server/internal/protocol"
	"github.com/unidrop/unidrop-server/internal/registry"
	"github.com/unidrop/unidrop-server/internal/relay"
)

// ControlWSHandler handles incoming WebSocket connections on /ws/control.
type ControlWSHandler struct {
	verifier     *auth.Verifier
	registry     *registry.DeviceRegistry
	relayManager *relay.RelayManager
	limits       limits.Limits
}

// NewControlWSHandler creates a new ControlWSHandler.
func NewControlWSHandler(v *auth.Verifier, reg *registry.DeviceRegistry, rm *relay.RelayManager, lim limits.Limits) *ControlWSHandler {
	return &ControlWSHandler{
		verifier:     v,
		registry:     reg,
		relayManager: rm,
		limits:       lim,
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
	challengePayloadObj := protocol.AuthChallengePayload{
		NonceSalt:  uuid.NewString(),
		ServerTime: time.Now().UnixMilli(),
	}
	challengePayload, _ := json.Marshal(challengePayloadObj)
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
		_ = ws.Close(websocket.StatusPolicyViolation, "auth handshake timed out")
		return
	}

	var authEnv protocol.ControlEnvelope
	if err := json.Unmarshal(authBytes, &authEnv); err != nil || authEnv.Action != protocol.ActionAuthRequest {
		slog.Warn("invalid auth request envelope")
		_ = ws.Close(websocket.StatusPolicyViolation, "invalid auth envelope")
		return
	}

	var authReq protocol.AuthRequestPayload
	if err := json.Unmarshal(authEnv.Payload, &authReq); err != nil {
		slog.Warn("invalid auth payload")
		_ = ws.Close(websocket.StatusPolicyViolation, "invalid auth payload format")
		return
	}

	// Step 3: Verify HMAC Signature with challenge NonceSalt (P2-1)
	now := time.Now()
	if err := h.verifier.VerifyWithSalt(&authReq, now, challengePayloadObj.NonceSalt); err != nil {
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
		_ = ws.Close(websocket.StatusPolicyViolation, "authentication failed")
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
		// P0-5 CAS compare-and-delete: only unregister and broadcast if active session is this exact instance
		if h.registry.UnregisterSession(session) {
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
		} else {
			slog.Info("old session disconnected, superseded by newer session", "device", session.DeviceID)
		}
	}()

	slog.Info("device authenticated", "device", session.DeviceID, "account", session.AccountID, "os", session.OSType)

	// Send AUTH_RESPONSE success, carrying the transfer limits so the client can
	// refuse an oversized selection locally instead of learning about it a
	// round trip later.
	effectiveLimits := h.limits
	authSuccessPayload, _ := json.Marshal(protocol.AuthResponsePayload{
		Success:    true,
		AssignedID: session.DeviceID,
		Limits:     &effectiveLimits,
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

		case protocol.ActionTransferOffer:
			if env.ToDevice == "" {
				continue
			}
			var offer protocol.TransferOfferPayload
			if err := json.Unmarshal(env.Payload, &offer); err != nil {
				// A malformed payload is refused rather than forwarded. The
				// previous `if err == nil` guard only ran the limit check when
				// the payload parsed, so anything unparsable sailed straight
				// past it to the peer.
				slog.Warn("malformed transfer offer refused", "device", session.DeviceID, "error", err)
				h.rejectToSelf(session, salvageSessionID(env.Payload), &limits.Violation{
					Code:    limits.CodeMalformedOffer,
					Message: "传输请求格式无法解析，已拒绝",
				})
				continue
			}
			if v := limits.CheckOffer(h.limits, &offer); v != nil {
				slog.Info("transfer offer refused by limits",
					"device", session.DeviceID, "session", offer.SessionID,
					"code", v.Code, "items", len(offer.Items), "total_size", offer.TotalSize)
				h.rejectToSelf(session, offer.SessionID, v)
				continue
			}
			h.routeToPeer(session, env)

		case protocol.ActionTransferAnswer:
			if env.ToDevice == "" {
				continue
			}
			// P0-1, P2-7: Authorize session in RelayManager when receiver accepts offer
			var answer protocol.TransferAnswerPayload
			if err := json.Unmarshal(env.Payload, &answer); err == nil && answer.Accepted && h.relayManager != nil {
				// Per-account concurrency is enforced while the token is minted,
				// not before it: counting first and authorizing afterwards would
				// let two connections of the same account both see a free slot
				// and both be granted one.
				//
				// On refusal the original answer is NOT forwarded. Forwarding a
				// token-less answer is what the old code did on authorization
				// failure, and the client skips such an answer silently
				// (`if let Some(token)`), leaving the sender waiting forever —
				// the same silent hang this round exists to remove, re-entered
				// through a different door. Both peers are told instead.
				//
				// Note the direction is the opposite of the OFFER rejection
				// above. For an OFFER the sender is the current session; for an
				// ANSWER the current session is the *receiver* and ToDevice is
				// the sender. Reusing one helper for both would send "too many
				// transfers" to the wrong peer. TestConcurrencyRejection... in
				// limits_ws_test.go pins both directions.
				token, inFlight, authErr := h.relayManager.AuthorizeSessionIfUnderLimit(
					answer.SessionID,
					env.ToDevice,     // Sender is ToDevice
					session.DeviceID, // Receiver is current session
					session.AccountID,
					5*time.Minute,
					h.limits.MaxConcurrentTransfers,
				)
				if errors.Is(authErr, relay.ErrConcurrencyLimit) {
					v := limits.ConcurrencyViolation(h.limits, inFlight)
					slog.Info("transfer answer refused by concurrency limit",
						"account", session.AccountID, "session", answer.SessionID,
						"in_flight", inFlight, "code", v.Code)
					h.rejectToSelf(session, answer.SessionID, v)               // 接收方 = 当前连接
					h.rejectToPeer(session, env.ToDevice, answer.SessionID, v) // 发送方 = ToDevice
					continue
				}
				if authErr == nil {
					slog.Info("transfer session authorized", "session", answer.SessionID, "sender", env.ToDevice, "receiver", session.DeviceID)
					answer.Token = token
					env.Payload, _ = json.Marshal(answer)
					// Echo authorized token back to the receiver as well
					echoEnv := env
					echoEnv.ToDevice = session.DeviceID
					echoBytes, _ := json.Marshal(echoEnv)
					session.Send(echoBytes)
				} else {
					slog.Warn("failed to authorize transfer session", "session", answer.SessionID, "error", authErr)
				}
			}
			h.routeToPeer(session, env)

		case protocol.ActionTransferComplete,
			protocol.ActionTransferFailure,
			protocol.ActionTransferCancel:
			// P2-12: Proactively clean up relay pipe and auth session
			if h.relayManager != nil {
				var termPayload struct {
					SessionID string `json:"session_id"`
				}
				if err := json.Unmarshal(env.Payload, &termPayload); err == nil && termPayload.SessionID != "" {
					slog.Info("relay pipe removed", "action", string(env.Action), "session", termPayload.SessionID, "from", session.DeviceID)
					h.relayManager.RemovePipe(termPayload.SessionID)
				}
			}
			h.routeToPeer(session, env)

		case protocol.ActionClipboardInjected:
			h.routeToPeer(session, env)
		}
	}
}

// buildFailure wraps a limit violation as a TRANSFER_FAILURE envelope.
func buildFailure(fromDevice, toDevice, sessionID string, v *limits.Violation) ([]byte, bool) {
	// The client matches a failure to a card by session_id and does nothing at
	// all when it is absent, so sending one without an id would be a rejection
	// that reaches the wire and lands nowhere.
	if sessionID == "" {
		return nil, false
	}
	payload, err := json.Marshal(protocol.TransferFailurePayload{
		SessionID:    sessionID,
		ErrorCode:    v.Code,
		ErrorMessage: v.Message,
	})
	if err != nil {
		return nil, false
	}
	envBytes, err := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    uuid.NewString(),
		Action:     protocol.ActionTransferFailure,
		FromDevice: fromDevice,
		ToDevice:   toDevice,
		Timestamp:  time.Now().UnixMilli(),
		Payload:    payload,
	})
	if err != nil {
		return nil, false
	}
	return envBytes, true
}

// rejectToSelf sends a TRANSFER_FAILURE back over the connection the refused
// message arrived on.
//
// This deliberately does not go through routeToPeer: that helper delivers to
// env.ToDevice, which for an OFFER is the *receiver*. Routing a rejection
// there would tell the wrong device that its file is too large while the
// sender kept waiting — leaving the very hang being fixed in place, now with a
// misleading message attached.
//
// Delivery is best-effort: session.Send enqueues onto the connection's
// 256-slot SendChan and drops the message if that queue is full, which is why
// the failure to enqueue is logged below. Writing to the socket directly would
// not be an improvement — the write pump goroutine owns this connection and
// coder/websocket forbids concurrent writes, so bypassing the queue would
// trade a dropped message for a data race. If a rejection ever must not be
// dropped, the answer is a policy for a full queue (closing the connection,
// say), not a second writer.
func (h *ControlWSHandler) rejectToSelf(session *registry.DeviceSession, sessionID string, v *limits.Violation) {
	envBytes, ok := buildFailure("server", session.DeviceID, sessionID, v)
	if !ok {
		// Reachable when the payload was too broken to yield a session_id. The
		// sender's card stays pending in that case; it is a known, accepted
		// edge (only an out-of-spec client can produce it) and is logged so it
		// is not mistaken for a missing feature.
		slog.Warn("cannot deliver limit rejection: no session_id in payload",
			"device", session.DeviceID, "code", v.Code)
		return
	}
	if !session.Send(envBytes) {
		slog.Warn("failed to deliver limit rejection to sender (queue full)",
			"device", session.DeviceID, "code", v.Code)
	}
}

// rejectToPeer sends a TRANSFER_FAILURE to the other party of a session.
//
// Used for the ANSWER path, where refusing the transfer must also unstick the
// peer: by the time an answer is sent the receiver has already recorded its own
// in-progress row, and the sender is still waiting on a reply that will now
// never carry a token.
func (h *ControlWSHandler) rejectToPeer(session *registry.DeviceSession, peerDevice, sessionID string, v *limits.Violation) {
	if peerDevice == "" {
		return
	}
	target, ok := h.registry.Get(peerDevice)
	if !ok {
		slog.Debug("peer offline for limit rejection", "to_device", peerDevice)
		return
	}
	if target.AccountID != session.AccountID {
		slog.Warn("cross-account limit rejection blocked", "from", session.AccountID, "to_device", peerDevice)
		return
	}
	envBytes, ok := buildFailure("server", peerDevice, sessionID, v)
	if !ok {
		return
	}
	if !target.Send(envBytes) {
		slog.Warn("failed to deliver limit rejection to peer (queue full)",
			"to_device", peerDevice, "code", v.Code)
	}
}

// salvageSessionID makes one lenient attempt to read session_id out of a
// payload that failed strict decoding.
//
// The common malformed case is structurally valid JSON whose fields have the
// wrong shape; session_id usually survives that, and recovering it is the
// difference between a rejection the user sees and one that vanishes. When
// even this fails the caller logs and gives up.
func salvageSessionID(payload []byte) string {
	var minimal struct {
		SessionID string `json:"session_id"`
	}
	if err := json.Unmarshal(payload, &minimal); err != nil {
		return ""
	}
	return minimal.SessionID
}

// routeToPeer routes a control message to target peer and logs drops (P1-1).
func (h *ControlWSHandler) routeToPeer(session *registry.DeviceSession, env protocol.ControlEnvelope) {
	if env.ToDevice == "" {
		return
	}
	if targetSession, ok := h.registry.Get(env.ToDevice); ok {
		if targetSession.AccountID == session.AccountID {
			repacked, _ := json.Marshal(env)
			if !targetSession.Send(repacked) {
				slog.Warn("failed to deliver control message to target peer (queue full)", "action", env.Action, "to_device", env.ToDevice)
			}
		} else {
			slog.Warn("cross-account transfer attempt blocked", "from", session.AccountID, "to_device", env.ToDevice)
		}
	} else {
		slog.Debug("target device offline for transfer message", "action", env.Action, "to_device", env.ToDevice)
	}
}
