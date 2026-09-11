package controller

import (
	"context"
	"log/slog"
	"net/http"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"
	"github.com/unidrop/unidrop-server/internal/protocol"
	"github.com/unidrop/unidrop-server/internal/relay"
)

// DataWSHandler handles binary streaming connections on /ws/data.
type DataWSHandler struct {
	relayManager *relay.RelayManager
}

// NewDataWSHandler creates a new DataWSHandler.
func NewDataWSHandler(rm *relay.RelayManager) *DataWSHandler {
	return &DataWSHandler{
		relayManager: rm,
	}
}

func (h *DataWSHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	query := r.URL.Query()

	sessionID := query.Get("session_id")
	role := query.Get("role") // "sender" | "receiver"
	deviceID := query.Get("device_id")
	targetDeviceID := query.Get("target_device_id")
	token := query.Get("token")

	if sessionID == "" || (role != "sender" && role != "receiver") {
		http.Error(w, "missing session_id or valid role", http.StatusBadRequest)
		return
	}

	// P0-1: Pre-authenticate data session before WebSocket Upgrade (unconditional, no bypass)
	if token == "" {
		slog.Warn("unauthorized data plane access attempt rejected: missing token", "session", sessionID, "device", deviceID)
		http.Error(w, "missing data token", http.StatusForbidden)
		return
	}

	if err := h.relayManager.CheckAuthorization(sessionID, role, deviceID, targetDeviceID, token); err != nil {
		slog.Warn("unauthorized data plane access attempt rejected", "session", sessionID, "device", deviceID, "error", err)
		http.Error(w, err.Error(), http.StatusForbidden)
		return
	}

	ws, err := websocket.Accept(w, r, &websocket.AcceptOptions{
		InsecureSkipVerify: true,
	})
	if err != nil {
		slog.Error("data websocket accept failed", "error", err)
		return
	}
	defer ws.Close(websocket.StatusInternalError, "data handler exiting")

	// Frame length limit: 64B Header + 4MB Payload + 1KB margin
	ws.SetReadLimit(int64(protocol.HeaderSize + protocol.MaxPayloadLength + 1024))

	pipe, err := h.relayManager.ValidateAndGetOrCreatePipe(sessionID, role, deviceID, targetDeviceID, token)
	if err != nil {
		slog.Warn("failed to authenticate or allocate relay pipe", "session", sessionID, "error", err)
		_ = ws.Close(websocket.StatusPolicyViolation, err.Error())
		return
	}

	pool := h.relayManager.BufferPool()

	if role == "sender" {
		h.handleSender(ctx, ws, pipe, pool)
	} else {
		h.handleReceiver(ctx, ws, pipe, pool)
	}
}

// handleSender reads data frames from sender and delivers ACKs/NACKs back to sender.
func (h *DataWSHandler) handleSender(ctx context.Context, ws *websocket.Conn, pipe *relay.RelayPipe, pool *relay.BufferPool) {
	senderCtx, cancel := context.WithCancel(ctx)
	defer cancel()

	ackPool := h.relayManager.AckBufferPool()

	// Reverse Pump: Forward ACKs/NACKs from BackwardChan back to sender WebSocket
	go func() {
		for {
			select {
			case <-senderCtx.Done():
				return
			case <-pipe.DoneChan:
				return
			case ackBuf, ok := <-pipe.BackwardChan:
				if !ok {
					return
				}
				writeCtx, writeCancel := context.WithTimeout(senderCtx, 5*time.Second)
				err := ws.Write(writeCtx, websocket.MessageBinary, *ackBuf)
				writeCancel()
				ackPool.Put(ackBuf)
				if err != nil {
					return
				}
			}
		}
	}()

	// Forward Pump: Read DATA frames from sender and push into pipe.ForwardChan
	for {
		msgType, data, err := ws.Read(ctx)
		if err != nil {
			break
		}
		if msgType != websocket.MessageBinary || len(data) < protocol.HeaderSize {
			slog.Warn("sender sent invalid non-binary or truncated frame, disconnecting")
			_ = ws.Close(websocket.StatusPolicyViolation, "invalid binary frame")
			break
		}

		// Decode header to validate frame structure (P1-7: disconnect on invalid magic)
		hdr, err := protocol.DecodeBinaryHeader(data)
		if err != nil {
			slog.Warn("invalid binary frame header from sender, disconnecting", "error", err)
			_ = ws.Close(websocket.StatusPolicyViolation, "invalid frame magic/header")
			break
		}

		if hdr.ChunkType != protocol.ChunkTypeData {
			slog.Warn("sender sent non-data chunk on forward channel, disconnecting", "type", hdr.ChunkType)
			_ = ws.Close(websocket.StatusPolicyViolation, "expected DATA chunk")
			break
		}

		// Check payload length matches declared
		if int(hdr.PayloadLen) != len(data)-protocol.HeaderSize {
			slog.Warn("declared payload length does not match actual length, disconnecting")
			_ = ws.Close(websocket.StatusPolicyViolation, "payload length mismatch")
			break
		}

		buf := pool.Get()
		*buf = (*buf)[:len(data)]
		copy(*buf, data)

		atomic.AddUint64(&Metrics.TotalRelayedBytes, uint64(len(data)))

		if err := pipe.PushForward(buf, 5*time.Second); err != nil {
			pool.Put(buf)
			slog.Warn("pipe push forward failed", "error", err)
			break
		}
	}
}

// handleReceiver forwards data frames to receiver and receives ACKs/NACKs back to pipe.
func (h *DataWSHandler) handleReceiver(ctx context.Context, ws *websocket.Conn, pipe *relay.RelayPipe, pool *relay.BufferPool) {
	receiverCtx, cancel := context.WithCancel(ctx)
	defer cancel()

	ackPool := h.relayManager.AckBufferPool()

	// Forward Pump: Read from ForwardChan and write to receiver WebSocket
	go func() {
		for {
			select {
			case <-receiverCtx.Done():
				return
			case <-pipe.DoneChan:
				return
			case dataBuf, ok := <-pipe.ForwardChan:
				if !ok {
					return
				}
				writeCtx, writeCancel := context.WithTimeout(receiverCtx, 5*time.Second)
				err := ws.Write(writeCtx, websocket.MessageBinary, *dataBuf)
				writeCancel()
				pool.Put(dataBuf)
				if err != nil {
					return
				}
			}
		}
	}()

	// Reverse Pump: Read ACK/NACK frames from receiver WebSocket and push to BackwardChan (P1-7, P2-2)
	for {
		msgType, data, err := ws.Read(ctx)
		if err != nil {
			break
		}
		if msgType != websocket.MessageBinary || len(data) < protocol.HeaderSize {
			slog.Warn("receiver sent invalid reverse frame, disconnecting")
			_ = ws.Close(websocket.StatusPolicyViolation, "invalid binary reverse frame")
			break
		}

		// Decode header and enforce ACK/NACK validation (P1-7)
		hdr, err := protocol.DecodeBinaryHeader(data)
		if err != nil {
			slog.Warn("receiver reverse frame header invalid, disconnecting", "error", err)
			_ = ws.Close(websocket.StatusPolicyViolation, "invalid header in ACK frame")
			break
		}

		if hdr.ChunkType != protocol.ChunkTypeAck && hdr.ChunkType != protocol.ChunkTypeNack && hdr.ChunkType != protocol.ChunkTypeProbe {
			slog.Warn("receiver sent non-ACK/NACK frame on backward channel, disconnecting", "type", hdr.ChunkType)
			_ = ws.Close(websocket.StatusPolicyViolation, "expected ACK/NACK chunk on backward channel")
			break
		}

		if hdr.PayloadLen != 0 {
			slog.Warn("ACK/NACK frame must have 0 payload, disconnecting", "payload_len", hdr.PayloadLen)
			_ = ws.Close(websocket.StatusPolicyViolation, "ACK frame must have zero payload")
			break
		}

		// Use small buffer pool for 64B ACK frames (P2-2)
		ackBuf := ackPool.Get()
		*ackBuf = (*ackBuf)[:len(data)]
		copy(*ackBuf, data)

		if err := pipe.PushBackward(ackBuf, 5*time.Second); err != nil {
			ackPool.Put(ackBuf)
			slog.Warn("pipe push backward failed", "error", err)
			break
		}
	}
}
