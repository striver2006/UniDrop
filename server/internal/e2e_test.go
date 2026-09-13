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
	"github.com/unidrop/unidrop-server/internal/limits"
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
	relayMgr := relay.NewRelayManager(0)

	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", controller.HealthHandler(reg, relayMgr))
	mux.Handle("GET /ws/control", controller.NewControlWSHandler(verifier, reg, relayMgr, limits.FromEnv()))
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

// TestCrossAccountIsolationEndToEnd 用两个账号、四台设备跑一次隔离验证，
// 其中**跨账号的 device_id 完全相同**（都叫 devA / devB）——
// 那正是改造前 registry 以裸 DeviceID 为键时会互相踢下线的形态。
//
// 断言三件事：A 账号内的完整传输照常走通、B 账号全程收不到任何信令、
// B 账号的两条连接都没有被踢掉。
func TestCrossAccountIsolationEndToEnd(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	pskSecret := "test-super-secret-key-12345"
	verifier, err := auth.NewVerifier(pskSecret)
	if err != nil {
		t.Fatalf("初始化校验器失败: %v", err)
	}

	reg := registry.NewDeviceRegistry()
	relayMgr := relay.NewRelayManager(0)

	mux := http.NewServeMux()
	mux.Handle("GET /ws/control", controller.NewControlWSHandler(verifier, reg, relayMgr, limits.FromEnv()))
	mux.Handle("GET /ws/data", controller.NewDataWSHandler(relayMgr))

	server := httptest.NewServer(mux)
	defer server.Close()
	wsURL := "ws" + strings.TrimPrefix(server.URL, "http")

	// 两个账号，各两台设备，device_id 刻意撞车
	acctA1 := connectAndAuthClient(t, ctx, wsURL, "acctA", "devA", "A-1", "macos", pskSecret, verifier)
	defer acctA1.Close(websocket.StatusNormalClosure, "done")
	acctA2 := connectAndAuthClient(t, ctx, wsURL, "acctA", "devB", "A-2", "windows", pskSecret, verifier)
	defer acctA2.Close(websocket.StatusNormalClosure, "done")
	acctB1 := connectAndAuthClient(t, ctx, wsURL, "acctB", "devA", "B-1", "macos", pskSecret, verifier)
	defer acctB1.Close(websocket.StatusNormalClosure, "done")
	acctB2 := connectAndAuthClient(t, ctx, wsURL, "acctB", "devB", "B-2", "linux", pskSecret, verifier)
	defer acctB2.Close(websocket.StatusNormalClosure, "done")

	time.Sleep(150 * time.Millisecond)

	if reg.Count() != 4 {
		t.Fatalf("四台设备应当全部在线（同名 device_id 不得互踢），实得 %d", reg.Count())
	}
	if reg.CountAccounts() != 2 {
		t.Fatalf("在线账号数应为 2，实得 %d", reg.CountAccounts())
	}
	if reg.MaxDevicesPerAccount() != 2 {
		t.Fatalf("单账号设备峰值应为 2，实得 %d", reg.MaxDevicesPerAccount())
	}

	// ---- A 账号内跑一次完整传输 ----
	payload := make([]byte, 64*1024)
	_, _ = rand.Read(payload)
	hasher := sha256.New()
	hasher.Write(payload)
	expectedSHA := hex.EncodeToString(hasher.Sum(nil))

	sessionUUID := uuid.New()
	offerPayload, _ := json.Marshal(protocol.TransferOfferPayload{
		SessionID:      sessionUUID.String(),
		DataType:       "FILES",
		TotalSize:      int64(len(payload)),
		TotalItems:     1,
		PreviewSummary: "isolation.bin",
		Items: []protocol.TransferItemPayload{{
			ItemIndex:    0,
			RelativePath: "isolation.bin",
			Size:         int64(len(payload)),
			SHA256:       expectedSHA,
			TotalChunks:  1,
		}},
	})
	offerEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version: 1, TraceID: uuid.NewString(), Action: protocol.ActionTransferOffer,
		FromDevice: "devA", ToDevice: "devB", Timestamp: time.Now().UnixMilli(),
		Payload: offerPayload,
	})
	if err := acctA1.Write(ctx, websocket.MessageText, offerEnv); err != nil {
		t.Fatalf("A 账号发送 OFFER 失败: %v", err)
	}

	// A 账号的 devB 收到 OFFER（跳过上下线广播）
	var receivedOffer protocol.ControlEnvelope
	for {
		_, raw, err := acctA2.Read(ctx)
		if err != nil {
			t.Fatalf("A 账号的对端读取 OFFER 失败: %v", err)
		}
		_ = json.Unmarshal(raw, &receivedOffer)
		if receivedOffer.Action == protocol.ActionTransferOffer {
			break
		}
	}

	answerPayload, _ := json.Marshal(protocol.TransferAnswerPayload{
		SessionID: sessionUUID.String(), Accepted: true,
	})
	answerEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version: 1, TraceID: receivedOffer.TraceID, Action: protocol.ActionTransferAnswer,
		FromDevice: "devB", ToDevice: "devA", Timestamp: time.Now().UnixMilli(),
		Payload: answerPayload,
	})
	if err := acctA2.Write(ctx, websocket.MessageText, answerEnv); err != nil {
		t.Fatalf("A 账号发送 ANSWER 失败: %v", err)
	}

	var receivedAnswer protocol.ControlEnvelope
	for {
		_, raw, err := acctA1.Read(ctx)
		if err != nil {
			t.Fatalf("A 账号读取 ANSWER 失败: %v", err)
		}
		_ = json.Unmarshal(raw, &receivedAnswer)
		if receivedAnswer.Action == protocol.ActionTransferAnswer {
			break
		}
	}
	var answerObj protocol.TransferAnswerPayload
	_ = json.Unmarshal(receivedAnswer.Payload, &answerObj)
	if answerObj.Token == "" {
		t.Fatal("A 账号的 ANSWER 应当带回数据面 token")
	}

	// 数据面：A 账号的两台设备对传
	sendURL := fmt.Sprintf("%s/ws/data?session_id=%s&role=sender&device_id=devA&target_device_id=devB&token=%s",
		wsURL, sessionUUID.String(), answerObj.Token)
	senderData, _, err := websocket.Dial(ctx, sendURL, nil)
	if err != nil {
		t.Fatalf("A 账号发送端接入数据面失败: %v", err)
	}
	defer senderData.Close(websocket.StatusNormalClosure, "done")

	recvURL := fmt.Sprintf("%s/ws/data?session_id=%s&role=receiver&device_id=devB&target_device_id=devA&token=%s",
		wsURL, sessionUUID.String(), answerObj.Token)
	receiverData, _, err := websocket.Dial(ctx, recvURL, nil)
	if err != nil {
		t.Fatalf("A 账号接收端接入数据面失败: %v", err)
	}
	defer receiverData.Close(websocket.StatusNormalClosure, "done")
	receiverData.SetReadLimit(int64(protocol.HeaderSize + protocol.MaxPayloadLength + 1024))

	hdr := protocol.NewDataHeader(sessionUUID, 0, 0, 1, payload)
	frame := make([]byte, protocol.HeaderSize+len(payload))
	if err := hdr.Encode(frame); err != nil {
		t.Fatalf("编码帧头失败: %v", err)
	}
	copy(frame[protocol.HeaderSize:], payload)

	if err := senderData.Write(ctx, websocket.MessageBinary, frame); err != nil {
		t.Fatalf("发送数据帧失败: %v", err)
	}

	_, got, err := receiverData.Read(ctx)
	if err != nil {
		t.Fatalf("接收数据帧失败: %v", err)
	}
	gotHdr, err := protocol.DecodeBinaryHeader(got)
	if err != nil {
		t.Fatalf("解码帧头失败: %v", err)
	}
	gotPayload := got[protocol.HeaderSize:]
	if err := gotHdr.VerifyPayloadCRC32(gotPayload); err != nil {
		t.Fatalf("数据帧 CRC32 校验失败: %v", err)
	}
	gotHasher := sha256.New()
	gotHasher.Write(gotPayload)
	if hex.EncodeToString(gotHasher.Sum(nil)) != expectedSHA {
		t.Fatal("A 账号内传输的数据校验和不符")
	}

	// ---- B 账号全程必须静默，且两条连接都还活着 ----
	// 顺序要紧：registry 断言必须排在下面的静默探测**之前**。
	// coder/websocket 在 Read 的 context 超时时会把整条连接关掉，
	// 所以探测完 B 的两条连接后它们就已经离线，再去数 Count 会少两台。
	if reg.Count() != 4 {
		t.Fatalf("传输结束后四台设备仍应在线（B 账号不得被踢），实得 %d", reg.Count())
	}
	if _, ok := reg.Get("acctB", "devA"); !ok {
		t.Fatal("B 账号的 devA 不得被 A 账号的同名设备顶掉")
	}
	if _, ok := reg.Get("acctB", "devB"); !ok {
		t.Fatal("B 账号的 devB 不得被 A 账号的同名设备顶掉")
	}

	// 注意断言的边界：B 账号**内部**的上下线广播是正常的（B-2 上线时 B-1 会收到
	// DEVICE_ONLINE），不算泄漏。这里要证伪的是「A 账号的传输信令漏到了 B」。
	for name, ws := range map[string]*websocket.Conn{"B-1": acctB1, "B-2": acctB2} {
		quietCtx, quietCancel := context.WithTimeout(ctx, 400*time.Millisecond)
		for {
			_, raw, err := ws.Read(quietCtx)
			if err != nil {
				break // 读超时 = 再没有别的东西了，符合预期
			}
			var env protocol.ControlEnvelope
			_ = json.Unmarshal(raw, &env)
			switch env.Action {
			case protocol.ActionDeviceOnline, protocol.ActionDeviceOffline,
				protocol.ActionDeviceListSync:
				continue // B 账号内部的设备拓扑广播，与 A 无关
			}
			quietCancel()
			t.Fatalf("B 账号的 %s 不得收到 A 账号的传输信令，却收到了 %s: %s",
				name, env.Action, raw)
		}
		quietCancel()
	}

}
