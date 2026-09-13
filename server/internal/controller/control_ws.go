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

	// Step 3a: Validate identifier shape BEFORE verifying the signature.
	//
	// The order matters and is not interchangeable. VerifyWithSalt burns the
	// nonce once the signature checks out (see auth.VerifyWithSalt), so a
	// request whose signature is valid but whose account_id is malformed would,
	// if validated afterwards, come back on retry as "replay detected" instead
	// of "your account id is malformed". A client implemented straight from the
	// canonical-string contract in DESIGN.md may reuse a nonce within the 60s
	// window, and for those the diagnosis would be permanently wrong. (The
	// shipped Tauri client mints a fresh nonce per handshake, so it is the
	// third-party contract — not our own client — that this ordering protects.)
	//
	// The second reason is cheapness: this rejects malformed input without
	// paying for an HMAC, and auth.validIdentifier is allocation-free and
	// length-bounded precisely because it runs here, on unauthenticated input.
	if err := auth.ValidateAccountID(authReq.AccountID); err != nil {
		slog.Warn("auth rejected: malformed account id", "device", authReq.DeviceID)
		Metrics.AuthRejectedInvalidAccount.Add(1)
		writeAuthFailure(ctx, ws, authEnv.TraceID, protocol.AuthErrInvalidAccountID,
			"账号标识不能为空，只能包含字母、数字与 . _ @ -，长度 1-64")
		return
	}
	if err := auth.ValidateDeviceID(authReq.DeviceID); err != nil {
		slog.Warn("auth rejected: malformed device id", "account", authReq.AccountID)
		Metrics.AuthRejectedInvalidDevice.Add(1)
		writeAuthFailure(ctx, ws, authEnv.TraceID, protocol.AuthErrInvalidDeviceID,
			"设备标识不能为空，只能包含字母、数字与 _ -，长度 1-64")
		return
	}

	// Step 3b: Verify HMAC Signature with challenge NonceSalt (P2-1)
	now := time.Now()
	if err := h.verifier.VerifyWithSalt(&authReq, now, challengePayloadObj.NonceSalt); err != nil {
		slog.Warn("auth verification failed", "device", authReq.DeviceID, "error", err)
		Metrics.AuthRejectedUnauthorized.Add(1)
		writeAuthFailure(ctx, ws, authEnv.TraceID, protocol.AuthErrUnauthorized, err.Error())
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
			answerOK := json.Unmarshal(env.Payload, &answer) == nil

			// session_id 必须先过格式校验再进授权：它会成为 authSessions 与
			// pipes 的 map 键，而这个字段只受控制面 512 KiB 读上限约束。
			//
			// 非法时**不转发、也无法回执**：buildFailure 依赖 session_id 匹配
			// 客户端卡片，空或畸形的 id 本来就没有卡片可落——硬发一条只会
			// 变成一次到不了任何地方的推送。这里只能记日志并丢弃。
			if answerOK && h.relayManager != nil {
				if err := auth.ValidateSessionID(answer.SessionID); err != nil {
					slog.Warn("transfer answer dropped: malformed session id",
						"account", session.AccountID, "device", session.DeviceID)
					continue
				}
			}

			if answerOK && answer.Accepted && h.relayManager != nil {
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
				// Every authorization failure — not just the concurrency one —
				// refuses both peers and stops here. The previous code special
				// cased ErrConcurrencyLimit and let everything else fall through
				// to routeToPeer with a token-less answer, which is precisely
				// the silent-hang shape described above; adding new error kinds
				// without widening this branch would walk straight back into it.
				if authErr != nil {
					var v *limits.Violation
					switch {
					case errors.Is(authErr, relay.ErrConcurrencyLimit):
						v = limits.ConcurrencyViolation(h.limits, inFlight)
						slog.Info("transfer answer refused by concurrency limit",
							"account", session.AccountID, "session", answer.SessionID,
							"in_flight", inFlight, "code", v.Code)
					case errors.Is(authErr, relay.ErrSessionIDConflict):
						v = limits.SessionConflictViolation()
						slog.Warn("transfer answer refused: session id conflict",
							"account", session.AccountID, "session", answer.SessionID)
					default:
						v = limits.AuthorizeFailedViolation()
						slog.Warn("failed to authorize transfer session",
							"session", answer.SessionID, "error", authErr)
					}
					// ConcurrencyViolation returns nil when the limit is
					// disabled or unreached. That should not be reachable from
					// an ErrConcurrencyLimit, but buildFailure dereferences the
					// violation, so a nil here would turn a refusal into a panic
					// on the control connection. Fall back rather than trust the
					// invariant.
					if v == nil {
						v = limits.AuthorizeFailedViolation()
					}
					h.rejectToSelf(session, answer.SessionID, v)               // 接收方 = 当前连接
					h.rejectToPeer(session, env.ToDevice, answer.SessionID, v) // 发送方 = ToDevice
					continue
				}

				slog.Info("transfer session authorized", "session", answer.SessionID, "sender", env.ToDevice, "receiver", session.DeviceID)
				answer.Token = token
				env.Payload, _ = json.Marshal(answer)
				// Echo authorized token back to the receiver as well
				echoEnv := env
				echoEnv.ToDevice = session.DeviceID
				echoBytes, _ := json.Marshal(echoEnv)
				session.Send(echoBytes)
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
					// 拒绝原因由 relay 在持锁期间一并定下，调用方不再二次查询：
					// 两次独立加锁之间可以插入一次新授权，会把一条良性的迟到信令
					// 误报成「跨账号拆除」告警。
					switch result, owner := h.relayManager.RemovePipeForSession(
						termPayload.SessionID, session.AccountID, session.DeviceID); result {
					case relay.TeardownOK:
						slog.Info("relay pipe removed", "action", string(env.Action),
							"session", termPayload.SessionID, "from", session.DeviceID)
					case relay.TeardownDenied:
						// 会话存在但不归请求方所有 —— 归属校验正是为这一支而加。
						slog.Warn("cross-account pipe teardown blocked",
							"session", termPayload.SessionID, "from_account", session.AccountID,
							"owner_account", owner, "from_device", session.DeviceID)
					case relay.TeardownNotFound:
						// 无可拆除：空闲回收之后迟到或重复的终结信令，属正常流量。
						// 这里若打 warn，诚实的长传输会把上面那支真正的越权告警淹掉。
						slog.Debug("terminal signal for unknown session",
							"action", string(env.Action), "session", termPayload.SessionID)
					}
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
	target, ok := h.registry.Get(session.AccountID, peerDevice)
	if !ok {
		slog.Debug("peer not found in account for limit rejection", "to_device", peerDevice)
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

// writeAuthFailure sends a failed AUTH_RESPONSE and closes the connection.
//
// Note that the two kinds of message flowing through here have different
// audiences, and that is deliberate rather than an inconsistency waiting to be
// tidied up. The identifier rejections carry Chinese text that states the rule
// itself, because the user is the only one who can fix it and "invalid account
// id" alone is not actionable. The UNAUTHORIZED path forwards err.Error() —
// English, internal, occasionally carrying things like diff=1.2s — because it
// is a diagnostic for whoever is debugging a signature or clock problem, and
// the user-facing wording for it already lives in USER_GUIDE.md.
func writeAuthFailure(ctx context.Context, ws *websocket.Conn, traceID, code, message string) {
	respPayload, _ := json.Marshal(protocol.AuthResponsePayload{
		Success:      false,
		ErrorCode:    code,
		ErrorMessage: message,
	})
	respEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:   1,
		TraceID:   traceID,
		Action:    protocol.ActionAuthResponse,
		Timestamp: time.Now().UnixMilli(),
		Payload:   respPayload,
	})
	_ = ws.Write(ctx, websocket.MessageText, respEnv)
	_ = ws.Close(websocket.StatusPolicyViolation, "authentication failed")
}

// routeToPeer routes a control message to target peer and logs drops (P1-1).
//
// The lookup is scoped to the sender's account, so cross-account delivery is
// structurally impossible rather than prevented by a comparison that a later
// edit could forget. One thing was given up in exchange, on purpose: this used
// to log "cross-account transfer attempt blocked" separately from "target
// device offline", and now both are one miss. Distinguishing them would mean
// confirming to the caller that another account owns a device by that name —
// and writing that into the log leaks it to operations as well.
func (h *ControlWSHandler) routeToPeer(session *registry.DeviceSession, env protocol.ControlEnvelope) {
	if env.ToDevice == "" {
		return
	}
	targetSession, ok := h.registry.Get(session.AccountID, env.ToDevice)
	if !ok {
		slog.Debug("peer not found in account for transfer message", "action", env.Action, "to_device", env.ToDevice)
		return
	}
	repacked, _ := json.Marshal(env)
	if !targetSession.Send(repacked) {
		slog.Warn("failed to deliver control message to target peer (queue full)", "action", env.Action, "to_device", env.ToDevice)
	}
}
