package internal_test

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/google/uuid"
	"github.com/unidrop/unidrop-server/internal/auth"
	"github.com/unidrop/unidrop-server/internal/controller"
	"github.com/unidrop/unidrop-server/internal/protocol"
	"github.com/unidrop/unidrop-server/internal/registry"
	"github.com/unidrop/unidrop-server/internal/relay"
)

func TestEndToEndTransfer(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	pskSecret := "test-super-secret-key-12345"
	verifier, err := auth.NewVerifier(pskSecret)
	if err != nil {
		t.Fatalf("failed to create verifier: %v", err)
	}

	reg := registry.NewDeviceRegistry()
	relayMgr := relay.NewRelayManager()

	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", controller.HealthHandler(reg, relayMgr))
	mux.Handle("GET /ws/control", controller.NewControlWSHandler(verifier, reg, relayMgr))
	mux.Handle("GET /ws/data", controller.NewDataWSHandler(relayMgr))

	server := httptest.NewServer(mux)
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http")

	// 1. Connect Client A (Sender) and Client B (Receiver) to Control Plane
	clientAControl := connectAndAuthClient(t, ctx, wsURL, "user1", "devA", "Host-A", "macos", pskSecret, verifier)
	defer clientAControl.Close(websocket.StatusNormalClosure, "done")

	clientBControl := connectAndAuthClient(t, ctx, wsURL, "user1", "devB", "Host-B", "windows", pskSecret, verifier)
	defer clientBControl.Close(websocket.StatusNormalClosure, "done")

	time.Sleep(100 * time.Millisecond) // Allow registrations to propagate

	// Verify both devices registered
	if reg.Count() != 2 {
		t.Fatalf("expected 2 devices in registry, got %d", reg.Count())
	}

	// 2. Prepare 10MB test data (3 chunks: 4MB, 4MB, 2MB)
	fileSize := 10 * 1024 * 1024 // 10MB
	fileData := make([]byte, fileSize)
	_, _ = rand.Read(fileData)

	fileHasher := sha256.New()
	fileHasher.Write(fileData)
	expectedSHA256 := hex.EncodeToString(fileHasher.Sum(nil))

	sessionUUID := uuid.New()
	chunkSize := int(protocol.MaxPayloadLength) // 4MB
	totalChunks := uint32((fileSize + chunkSize - 1) / chunkSize)

	// 3. Client A sends TRANSFER_OFFER to Client B
	offerPayload, _ := json.Marshal(protocol.TransferOfferPayload{
		SessionID:      sessionUUID.String(),
		DataType:       "FILES",
		TotalSize:      int64(fileSize),
		TotalItems:     1,
		PreviewSummary: "10mb-test-file.bin",
		Items: []protocol.TransferItemPayload{
			{
				ItemIndex:    0,
				RelativePath: "10mb-test-file.bin",
				Size:         int64(fileSize),
				IsDir:        false,
				SHA256:       expectedSHA256,
				TotalChunks:  totalChunks,
			},
		},
	})

	offerEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    uuid.NewString(),
		Action:     protocol.ActionTransferOffer,
		FromDevice: "devA",
		ToDevice:   "devB",
		Timestamp:  time.Now().UnixMilli(),
		Payload:    offerPayload,
	})

	if err := clientAControl.Write(ctx, websocket.MessageText, offerEnv); err != nil {
		t.Fatalf("Client A failed to send TRANSFER_OFFER: %v", err)
	}

	// 4. Client B receives TRANSFER_OFFER and replies with TRANSFER_ANSWER (accepted=true)
	_, offerBytesReceived, err := clientBControl.Read(ctx)
	if err != nil {
		t.Fatalf("Client B failed to read offer: %v", err)
	}

	var receivedOffer protocol.ControlEnvelope
	if err := json.Unmarshal(offerBytesReceived, &receivedOffer); err != nil || receivedOffer.Action != protocol.ActionTransferOffer {
		t.Fatalf("Client B received unexpected message: %v", string(offerBytesReceived))
	}

	answerPayload, _ := json.Marshal(protocol.TransferAnswerPayload{
		SessionID: sessionUUID.String(),
		Accepted:  true,
	})
	answerEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    receivedOffer.TraceID,
		Action:     protocol.ActionTransferAnswer,
		FromDevice: "devB",
		ToDevice:   "devA",
		Timestamp:  time.Now().UnixMilli(),
		Payload:    answerPayload,
	})
	if err := clientBControl.Write(ctx, websocket.MessageText, answerEnv); err != nil {
		t.Fatalf("Client B failed to send answer: %v", err)
	}

	// Client A receives TRANSFER_ANSWER (skipping any async broadcast like DEVICE_ONLINE)
	var receivedAnswer protocol.ControlEnvelope
	for {
		_, answerBytesReceived, err := clientAControl.Read(ctx)
		if err != nil {
			t.Fatalf("Client A failed to read answer: %v", err)
		}
		if err := json.Unmarshal(answerBytesReceived, &receivedAnswer); err != nil {
			t.Fatalf("Client A failed to unmarshal: %v", err)
		}
		if receivedAnswer.Action == protocol.ActionTransferAnswer {
			break
		}
	}

	var answerObj protocol.TransferAnswerPayload
	_ = json.Unmarshal(receivedAnswer.Payload, &answerObj)
	sessionToken := answerObj.Token
	if sessionToken == "" {
		t.Fatalf("expected non-empty data session token in TRANSFER_ANSWER")
	}

	// 5. Connect Client A (Sender) and Client B (Receiver) to Data Plane with secure token
	dataURLSender := fmt.Sprintf("%s/ws/data?session_id=%s&role=sender&device_id=devA&target_device_id=devB&token=%s", wsURL, sessionUUID.String(), sessionToken)
	clientAData, _, err := websocket.Dial(ctx, dataURLSender, nil)
	if err != nil {
		t.Fatalf("Client A data dial failed: %v", err)
	}
	defer clientAData.Close(websocket.StatusNormalClosure, "done")

	dataURLReceiver := fmt.Sprintf("%s/ws/data?session_id=%s&role=receiver&device_id=devB&target_device_id=devA&token=%s", wsURL, sessionUUID.String(), sessionToken)
	clientBData, _, err := websocket.Dial(ctx, dataURLReceiver, nil)
	if err != nil {
		t.Fatalf("Client B data dial failed: %v", err)
	}
	defer clientBData.Close(websocket.StatusNormalClosure, "done")

	// Set read limit to accommodate 4MB chunk
	clientBData.SetReadLimit(int64(protocol.HeaderSize + protocol.MaxPayloadLength + 1024))
	clientAData.SetReadLimit(int64(protocol.HeaderSize + 1024))

	// 6. Receiver Goroutine: Read Chunks, verify CRC32, send back ACK
	receivedFileData := make([]byte, fileSize)
	receiverDone := make(chan struct{})

	go func() {
		defer close(receiverDone)
		for c := uint32(0); c < totalChunks; c++ {
			msgType, chunkData, err := clientBData.Read(ctx)
			if err != nil {
				t.Errorf("Client B read chunk error: %v", err)
				return
			}
			if msgType != websocket.MessageBinary {
				t.Errorf("expected binary message, got %v", msgType)
				return
			}

			hdr, err := protocol.DecodeBinaryHeader(chunkData)
			if err != nil {
				t.Errorf("Client B decode header error: %v", err)
				return
			}

			payload := chunkData[protocol.HeaderSize:]
			if err := hdr.VerifyPayloadCRC32(payload); err != nil {
				t.Errorf("Client B CRC32 failed on chunk %d: %v", c, err)
				return
			}

			// Copy into received buffer
			offset := int(hdr.ChunkIndex) * chunkSize
			copy(receivedFileData[offset:], payload)

			// Send back CHUNK_ACK
			ackHdr := protocol.NewAckHeader(hdr.SessionID, hdr.ItemIndex, hdr.ChunkIndex)
			ackBuf := make([]byte, protocol.HeaderSize)
			_ = ackHdr.Encode(ackBuf)
			if err := clientBData.Write(ctx, websocket.MessageBinary, ackBuf); err != nil {
				t.Errorf("Client B write ACK error: %v", err)
				return
			}
		}
	}()

	// 7. Sender Loop: Send chunks and read ACKs
	senderAckDone := make(chan struct{})
	go func() {
		defer close(senderAckDone)
		for c := uint32(0); c < totalChunks; c++ {
			_, ackData, err := clientAData.Read(ctx)
			if err != nil {
				t.Errorf("Client A read ACK error: %v", err)
				return
			}
			ackHdr, err := protocol.DecodeBinaryHeader(ackData)
			if err != nil || ackHdr.ChunkType != protocol.ChunkTypeAck || ackHdr.ChunkIndex != c {
				t.Errorf("Client A unexpected ACK: idx=%d err=%v", ackHdr.ChunkIndex, err)
				return
			}
		}
	}()

	for c := uint32(0); c < totalChunks; c++ {
		start := int(c) * chunkSize
		end := start + chunkSize
		if end > fileSize {
			end = fileSize
		}
		chunkPayload := fileData[start:end]

		hdr := protocol.NewDataHeader(sessionUUID, 0, c, totalChunks, chunkPayload)
		frame := make([]byte, protocol.HeaderSize+len(chunkPayload))
		_ = hdr.Encode(frame[:protocol.HeaderSize])
		copy(frame[protocol.HeaderSize:], chunkPayload)

		if err := clientAData.Write(ctx, websocket.MessageBinary, frame); err != nil {
			t.Fatalf("Client A send chunk %d error: %v", c, err)
		}
	}

	<-receiverDone
	<-senderAckDone

	// 8. Verify assembled whole-file SHA-256
	receivedHasher := sha256.New()
	receivedHasher.Write(receivedFileData)
	actualSHA256 := hex.EncodeToString(receivedHasher.Sum(nil))

	if actualSHA256 != expectedSHA256 {
		t.Fatalf("Whole file SHA-256 mismatch!\nExpected: %s\nActual:   %s", expectedSHA256, actualSHA256)
	}

	// 9. Feedback: Client B sends CLIPBOARD_INJECTED, Client A receives it
	injectedPayload, _ := json.Marshal(protocol.ClipboardInjectedPayload{
		SessionID:  sessionUUID.String(),
		InjectedAt: time.Now().UnixMilli(),
		ItemCount:  1,
	})
	injectedEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    uuid.NewString(),
		Action:     protocol.ActionClipboardInjected,
		FromDevice: "devB",
		ToDevice:   "devA",
		Timestamp:  time.Now().UnixMilli(),
		Payload:    injectedPayload,
	})
	if err := clientBControl.Write(ctx, websocket.MessageText, injectedEnv); err != nil {
		t.Fatalf("Client B failed to send CLIPBOARD_INJECTED: %v", err)
	}

	_, injectedReceivedBytes, err := clientAControl.Read(ctx)
	if err != nil {
		t.Fatalf("Client A failed to read CLIPBOARD_INJECTED: %v", err)
	}
	var receivedInjected protocol.ControlEnvelope
	if err := json.Unmarshal(injectedReceivedBytes, &receivedInjected); err != nil || receivedInjected.Action != protocol.ActionClipboardInjected {
		t.Fatalf("Client A received unexpected feedback: %v", string(injectedReceivedBytes))
	}

	t.Log("Successfully completed 10MB transfer with zero corruption and feedback acknowledgment!")
}

func connectAndAuthClient(t *testing.T, ctx context.Context, wsURL, accountID, deviceID, hostname, osType, pskSecret string, v *auth.Verifier) *websocket.Conn {
	ws, _, err := websocket.Dial(ctx, wsURL+"/ws/control", nil)
	if err != nil {
		t.Fatalf("dial control failed for %s: %v", deviceID, err)
	}

	// 1. Read challenge
	_, msgBytes, err := ws.Read(ctx)
	if err != nil {
		t.Fatalf("read challenge failed: %v", err)
	}
	var challengeEnv protocol.ControlEnvelope
	if err := json.Unmarshal(msgBytes, &challengeEnv); err != nil || challengeEnv.Action != protocol.ActionAuthChallenge {
		t.Fatalf("unexpected challenge: %v", string(msgBytes))
	}

	// 2. Send auth request with NonceSalt
	var challengePayload protocol.AuthChallengePayload
	_ = json.Unmarshal(challengeEnv.Payload, &challengePayload)

	now := time.Now().UnixMilli()
	nonce := uuid.NewString()
	sig := v.GenerateSignatureWithSalt(accountID, deviceID, nonce, now, challengePayload.NonceSalt)

	reqPayload, _ := json.Marshal(protocol.AuthRequestPayload{
		AccountID:  accountID,
		DeviceID:   deviceID,
		Hostname:   hostname,
		OSType:     osType,
		AppVersion: "1.0.0",
		Signature:  sig,
		Nonce:      nonce,
		Timestamp:  now,
	})
	reqEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:   1,
		TraceID:   uuid.NewString(),
		Action:    protocol.ActionAuthRequest,
		Timestamp: now,
		Payload:   reqPayload,
	})
	if err := ws.Write(ctx, websocket.MessageText, reqEnv); err != nil {
		t.Fatalf("write auth request failed: %v", err)
	}

	// 3. Read auth response
	_, respBytes, err := ws.Read(ctx)
	if err != nil {
		t.Fatalf("read auth response failed: %v", err)
	}
	var respEnv protocol.ControlEnvelope
	if err := json.Unmarshal(respBytes, &respEnv); err != nil || respEnv.Action != protocol.ActionAuthResponse {
		t.Fatalf("unexpected auth response: %v", string(respBytes))
	}
	var authResp protocol.AuthResponsePayload
	_ = json.Unmarshal(respEnv.Payload, &authResp)
	if !authResp.Success {
		t.Fatalf("auth failed: %s", authResp.ErrorMessage)
	}

	// 4. Read DEVICE_LIST_SYNC
	_, syncBytes, err := ws.Read(ctx)
	if err != nil {
		t.Fatalf("read sync failed: %v", err)
	}
	var syncEnv protocol.ControlEnvelope
	if err := json.Unmarshal(syncBytes, &syncEnv); err != nil || syncEnv.Action != protocol.ActionDeviceListSync {
		t.Fatalf("unexpected sync: %v", string(syncBytes))
	}

	return ws
}
