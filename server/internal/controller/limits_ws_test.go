package controller_test

import (
	"context"
	"encoding/json"
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

const testPSK = "test-psk-for-limits"

// ---------- 握手脚手架 ----------
//
// 控制面握手原本只存在于 internal/e2e_test.go，跨包无法复用；本文件按需在
// controller 包内重建一份最小实现。计划原稿曾误称 control_ws_test.go 里
// 已有握手可复用——那里其实只有两个数据面测试。

type limitsHarness struct {
	server   *httptest.Server
	verifier *auth.Verifier
	registry *registry.DeviceRegistry
	relay    *relay.RelayManager
}

func newLimitsHarness(t *testing.T, lim limits.Limits) *limitsHarness {
	t.Helper()

	verifier, err := auth.NewVerifier(testPSK)
	if err != nil {
		t.Fatalf("初始化校验器失败: %v", err)
	}
	reg := registry.NewDeviceRegistry()
	relayMgr := relay.NewRelayManager(0)

	mux := http.NewServeMux()
	mux.Handle("GET /ws/control", controller.NewControlWSHandler(verifier, reg, relayMgr, lim))

	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)

	return &limitsHarness{server: srv, verifier: verifier, registry: reg, relay: relayMgr}
}

// connect 完成握手并**断言鉴权成功**，返回已鉴权的连接与 AUTH_RESPONSE。
// 需要检验拒绝路径的用例请用 connectRaw。
func (h *limitsHarness) connect(t *testing.T, ctx context.Context, accountID, deviceID string) (*websocket.Conn, protocol.AuthResponsePayload) {
	t.Helper()

	ws, resp := h.connectRaw(t, ctx, accountID, deviceID)
	if !resp.Success {
		t.Fatalf("鉴权失败: %s", resp.ErrorMessage)
	}

	// 紧随其后的 DEVICE_LIST_SYNC 先读掉，免得污染后续断言
	_, _, _ = ws.Read(ctx)

	return ws, resp
}

// connectRaw 跑完握手就返回，**不判断 Success、也不消费后续报文**。
//
// 从 connect 里拆出来是因为原来那一份在鉴权失败时直接 t.Fatalf，
// 于是拒绝路径根本无从断言 —— 而本轮新增的两类身份拒绝正需要它。
func (h *limitsHarness) connectRaw(t *testing.T, ctx context.Context, accountID, deviceID string) (*websocket.Conn, protocol.AuthResponsePayload) {
	t.Helper()

	wsURL := "ws" + strings.TrimPrefix(h.server.URL, "http")
	ws, _, err := websocket.Dial(ctx, wsURL+"/ws/control", nil)
	if err != nil {
		t.Fatalf("连接控制面失败: %v", err)
	}
	t.Cleanup(func() { _ = ws.CloseNow() })

	_, challengeBytes, err := ws.Read(ctx)
	if err != nil {
		t.Fatalf("读取 challenge 失败: %v", err)
	}
	var challengeEnv protocol.ControlEnvelope
	if err := json.Unmarshal(challengeBytes, &challengeEnv); err != nil ||
		challengeEnv.Action != protocol.ActionAuthChallenge {
		t.Fatalf("期望 AUTH_CHALLENGE，实际: %s", challengeBytes)
	}
	var challenge protocol.AuthChallengePayload
	_ = json.Unmarshal(challengeEnv.Payload, &challenge)

	now := time.Now().UnixMilli()
	nonce := uuid.NewString()
	sig := h.verifier.GenerateSignatureWithSalt(accountID, deviceID, nonce, now, challenge.NonceSalt)

	reqPayload, _ := json.Marshal(protocol.AuthRequestPayload{
		AccountID:  accountID,
		DeviceID:   deviceID,
		Hostname:   deviceID,
		OSType:     "macos",
		AppVersion: "test",
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
		t.Fatalf("发送 AUTH_REQUEST 失败: %v", err)
	}

	_, respBytes, err := ws.Read(ctx)
	if err != nil {
		t.Fatalf("读取 AUTH_RESPONSE 失败: %v", err)
	}
	var respEnv protocol.ControlEnvelope
	_ = json.Unmarshal(respBytes, &respEnv)
	var resp protocol.AuthResponsePayload
	_ = json.Unmarshal(respEnv.Payload, &resp)

	return ws, resp
}

// connectRawWithNonce 与 connectRaw 相同，但由调用方指定 nonce，
// 用于验证「身份格式被拒时不烧 nonce」。
func (h *limitsHarness) connectRawWithNonce(t *testing.T, ctx context.Context, accountID, deviceID, nonce string) protocol.AuthResponsePayload {
	t.Helper()

	wsURL := "ws" + strings.TrimPrefix(h.server.URL, "http")
	ws, _, err := websocket.Dial(ctx, wsURL+"/ws/control", nil)
	if err != nil {
		t.Fatalf("连接控制面失败: %v", err)
	}
	t.Cleanup(func() { _ = ws.CloseNow() })

	_, challengeBytes, err := ws.Read(ctx)
	if err != nil {
		t.Fatalf("读取 challenge 失败: %v", err)
	}
	var challengeEnv protocol.ControlEnvelope
	_ = json.Unmarshal(challengeBytes, &challengeEnv)
	var challenge protocol.AuthChallengePayload
	_ = json.Unmarshal(challengeEnv.Payload, &challenge)

	now := time.Now().UnixMilli()
	sig := h.verifier.GenerateSignatureWithSalt(accountID, deviceID, nonce, now, challenge.NonceSalt)

	reqPayload, _ := json.Marshal(protocol.AuthRequestPayload{
		AccountID:  accountID,
		DeviceID:   deviceID,
		Hostname:   deviceID,
		OSType:     "macos",
		AppVersion: "test",
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
		t.Fatalf("发送 AUTH_REQUEST 失败: %v", err)
	}

	_, respBytes, err := ws.Read(ctx)
	if err != nil {
		t.Fatalf("读取 AUTH_RESPONSE 失败: %v", err)
	}
	var respEnv2 protocol.ControlEnvelope
	_ = json.Unmarshal(respBytes, &respEnv2)
	var resp2 protocol.AuthResponsePayload
	_ = json.Unmarshal(respEnv2.Payload, &resp2)
	return resp2
}

func sendOffer(t *testing.T, ctx context.Context, ws *websocket.Conn, from, to string, offer protocol.TransferOfferPayload) {
	t.Helper()
	payload, _ := json.Marshal(offer)
	envBytes, _ := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    uuid.NewString(),
		Action:     protocol.ActionTransferOffer,
		FromDevice: from,
		ToDevice:   to,
		Timestamp:  time.Now().UnixMilli(),
		Payload:    payload,
	})
	if err := ws.Write(ctx, websocket.MessageText, envBytes); err != nil {
		t.Fatalf("发送 offer 失败: %v", err)
	}
}

// readAction 读到指定 action 为止，跳过途中的拓扑广播。
func readAction(t *testing.T, ctx context.Context, ws *websocket.Conn, want protocol.ActionType) protocol.ControlEnvelope {
	t.Helper()
	for i := 0; i < 8; i++ {
		_, msgBytes, err := ws.Read(ctx)
		if err != nil {
			t.Fatalf("等待 %s 时读取失败: %v", want, err)
		}
		var env protocol.ControlEnvelope
		if err := json.Unmarshal(msgBytes, &env); err != nil {
			continue
		}
		if env.Action == want {
			return env
		}
	}
	t.Fatalf("未等到 %s", want)
	return protocol.ControlEnvelope{}
}

func oversizedOffer() protocol.TransferOfferPayload {
	return protocol.TransferOfferPayload{
		SessionID:      uuid.NewString(),
		DataType:       "FILES",
		TotalSize:      limits.DefaultMaxSingleFileBytes + 1,
		TotalItems:     1,
		PreviewSummary: "huge.bin",
		Items: []protocol.TransferItemPayload{
			{ItemIndex: 0, RelativePath: "huge.bin", Size: limits.DefaultMaxSingleFileBytes + 1},
		},
	}
}

// ---------- AUTH_RESPONSE 下发限额 ----------

func TestAuthResponseCarriesLimits(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	h := newLimitsHarness(t, limits.FromEnv())
	_, resp := h.connect(t, ctx, "acct", "dev-a")

	if resp.Limits == nil {
		t.Fatal("AUTH_RESPONSE 必须带上 limits，否则客户端无法做本地预检")
	}
	if resp.Limits.MaxSingleFileBytes != limits.DefaultMaxSingleFileBytes {
		t.Fatalf("下发的单文件上限应为 %d，实际 %d",
			limits.DefaultMaxSingleFileBytes, resp.Limits.MaxSingleFileBytes)
	}
	if resp.Limits.MaxItemsPerOffer != limits.DefaultMaxItemsPerOffer {
		t.Fatalf("下发的条目数上限应为 %d，实际 %d",
			limits.DefaultMaxItemsPerOffer, resp.Limits.MaxItemsPerOffer)
	}
}

// ---------- 超限 offer：拒绝信必须到达发送方 ----------

// 这条同时钉死两件事：拒绝信到达**发送方**（而不是被 routeToPeer 发给接收方），
// 以及超限 offer **不被转发**给接收方。
func TestOversizedOfferRejectedToSenderAndNotForwarded(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	h := newLimitsHarness(t, limits.FromEnv())
	sender, _ := h.connect(t, ctx, "acct", "dev-sender")
	receiver, _ := h.connect(t, ctx, "acct", "dev-receiver")

	offer := oversizedOffer()
	sendOffer(t, ctx, sender, "dev-sender", "dev-receiver", offer)

	// 发送方收到 TRANSFER_FAILURE
	env := readAction(t, ctx, sender, protocol.ActionTransferFailure)
	var failure protocol.TransferFailurePayload
	if err := json.Unmarshal(env.Payload, &failure); err != nil {
		t.Fatalf("解析 TRANSFER_FAILURE 失败: %v", err)
	}
	if failure.SessionID != offer.SessionID {
		t.Fatalf("失败信必须带回原 session_id，否则客户端匹配不到卡片；期望 %s，实际 %q",
			offer.SessionID, failure.SessionID)
	}
	if failure.ErrorCode != limits.CodeFileTooLarge {
		t.Fatalf("错误码应为 %s，实际 %s", limits.CodeFileTooLarge, failure.ErrorCode)
	}
	if !strings.Contains(failure.ErrorMessage, "128.0 MB") {
		t.Fatalf("文案必须写明上限，实际: %q", failure.ErrorMessage)
	}

	// 接收方不应收到这条 offer
	readCtx, readCancel := context.WithTimeout(ctx, 600*time.Millisecond)
	defer readCancel()
	for {
		_, msgBytes, err := receiver.Read(readCtx)
		if err != nil {
			break // 超时即符合预期：没有 offer 被转发过来
		}
		var env protocol.ControlEnvelope
		if json.Unmarshal(msgBytes, &env) == nil && env.Action == protocol.ActionTransferOffer {
			t.Fatal("超限 offer 不得被转发给接收方")
		}
	}
}

// 畸形 payload 必须被拒绝而不是放行。改造前的 `if err == nil` 只在解析成功时
// 才做限额检查，解不开的 payload 反而被直接转发。
func TestMalformedOfferRejectedNotForwarded(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	h := newLimitsHarness(t, limits.FromEnv())
	sender, _ := h.connect(t, ctx, "acct", "dev-sender")
	receiver, _ := h.connect(t, ctx, "acct", "dev-receiver")

	sessionID := uuid.NewString()
	// JSON 结构合法但 items 的类型不对：严格解析失败，而 session_id 仍可打捞
	badPayload := []byte(`{"session_id":"` + sessionID + `","data_type":"FILES","items":"not-an-array"}`)
	envBytes, _ := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    uuid.NewString(),
		Action:     protocol.ActionTransferOffer,
		FromDevice: "dev-sender",
		ToDevice:   "dev-receiver",
		Timestamp:  time.Now().UnixMilli(),
		Payload:    badPayload,
	})
	if err := sender.Write(ctx, websocket.MessageText, envBytes); err != nil {
		t.Fatalf("发送畸形 offer 失败: %v", err)
	}

	env := readAction(t, ctx, sender, protocol.ActionTransferFailure)
	var failure protocol.TransferFailurePayload
	_ = json.Unmarshal(env.Payload, &failure)
	if failure.ErrorCode != limits.CodeMalformedOffer {
		t.Fatalf("错误码应为 %s，实际 %s", limits.CodeMalformedOffer, failure.ErrorCode)
	}
	// 打捞出的 session_id 让这条拒绝真的能落到卡片上
	if failure.SessionID != sessionID {
		t.Fatalf("应从畸形 payload 中打捞出 session_id %s，实际 %q", sessionID, failure.SessionID)
	}

	readCtx, readCancel := context.WithTimeout(ctx, 600*time.Millisecond)
	defer readCancel()
	for {
		_, msgBytes, err := receiver.Read(readCtx)
		if err != nil {
			break
		}
		var got protocol.ControlEnvelope
		if json.Unmarshal(msgBytes, &got) == nil && got.Action == protocol.ActionTransferOffer {
			t.Fatal("畸形 offer 不得被转发给接收方")
		}
	}
}

// 限额内的 offer 必须照常转发——否则上面两条测试用「全都拒绝」也能通过。
func TestWithinLimitOfferStillForwarded(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	h := newLimitsHarness(t, limits.FromEnv())
	sender, _ := h.connect(t, ctx, "acct", "dev-sender")
	receiver, _ := h.connect(t, ctx, "acct", "dev-receiver")

	offer := protocol.TransferOfferPayload{
		SessionID:      uuid.NewString(),
		DataType:       "FILES",
		TotalSize:      1024,
		TotalItems:     1,
		PreviewSummary: "small.txt",
		Items: []protocol.TransferItemPayload{
			{ItemIndex: 0, RelativePath: "small.txt", Size: 1024},
		},
	}
	sendOffer(t, ctx, sender, "dev-sender", "dev-receiver", offer)

	env := readAction(t, ctx, receiver, protocol.ActionTransferOffer)
	var got protocol.TransferOfferPayload
	_ = json.Unmarshal(env.Payload, &got)
	if got.SessionID != offer.SessionID {
		t.Fatalf("限额内的 offer 应原样转发，期望 %s，实际 %s", offer.SessionID, got.SessionID)
	}
}

// 上限设为 0（不限制）时，超大 offer 照常放行。
func TestZeroLimitDisablesEnforcement(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	lim := limits.FromEnv()
	lim.MaxSingleFileBytes = 0
	lim.MaxTotalTransferBytes = 0

	h := newLimitsHarness(t, lim)
	sender, _ := h.connect(t, ctx, "acct", "dev-sender")
	receiver, _ := h.connect(t, ctx, "acct", "dev-receiver")

	offer := oversizedOffer()
	sendOffer(t, ctx, sender, "dev-sender", "dev-receiver", offer)

	env := readAction(t, ctx, receiver, protocol.ActionTransferOffer)
	var got protocol.TransferOfferPayload
	_ = json.Unmarshal(env.Payload, &got)
	if got.SessionID != offer.SessionID {
		t.Fatalf("上限为 0 时应放行，期望 %s，实际 %s", offer.SessionID, got.SessionID)
	}
}

// ---------- ANSWER 并发拒绝：双向回送且方向不得接反 ----------

func sendAnswer(t *testing.T, ctx context.Context, ws *websocket.Conn, from, to, sessionID string) {
	t.Helper()
	payload, _ := json.Marshal(protocol.TransferAnswerPayload{
		SessionID: sessionID,
		Accepted:  true,
	})
	envBytes, _ := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    uuid.NewString(),
		Action:     protocol.ActionTransferAnswer,
		FromDevice: from,
		ToDevice:   to,
		Timestamp:  time.Now().UnixMilli(),
		Payload:    payload,
	})
	if err := ws.Write(ctx, websocket.MessageText, envBytes); err != nil {
		t.Fatalf("发送 answer 失败: %v", err)
	}
}

// 并发超限时，**发送方与接收方都要收到** TRANSFER_FAILURE。
//
// 这条守护的是 control_ws.go 里那段注释所警告的机关：ANSWER 的回送方向与
// OFFER **相反**——OFFER 的发送方是当前连接，ANSWER 的当前连接却是接收方、
// ToDevice 才是发送方。把 rejectToSelf / rejectToPeer 两行对调，或删掉其中
// 任意一行，这条测试都会红。
//
// 在补上它之前，本轮最容易写反的一行没有任何自动守护：方向接反时 CI 依旧
// 全绿，而用户复现的恰是本轮要消灭的静默挂起。
func TestConcurrencyRejectionReachesBothPeers(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	lim := limits.FromEnv()
	lim.MaxConcurrentTransfers = 1 // 第二个会话必被拒

	h := newLimitsHarness(t, lim)
	sender, _ := h.connect(t, ctx, "acct", "dev-sender")
	receiver, _ := h.connect(t, ctx, "acct", "dev-receiver")

	// 第一个会话占满唯一的名额
	first := uuid.NewString()
	sendAnswer(t, ctx, receiver, "dev-receiver", "dev-sender", first)
	// 接收方会收到 token 回显，发送方会收到带 token 的 answer；读掉以免污染后续
	_ = readAction(t, ctx, sender, protocol.ActionTransferAnswer)

	// 第二个会话：应被拒，且双方都收到失败
	second := uuid.NewString()
	sendAnswer(t, ctx, receiver, "dev-receiver", "dev-sender", second)

	// 接收方（当前连接）——走 rejectToSelf
	recvEnv := readAction(t, ctx, receiver, protocol.ActionTransferFailure)
	var recvFailure protocol.TransferFailurePayload
	if err := json.Unmarshal(recvEnv.Payload, &recvFailure); err != nil {
		t.Fatalf("接收方侧失败信解析失败: %v", err)
	}
	if recvFailure.SessionID != second {
		t.Fatalf("接收方侧失败信的 session_id 应为 %s，实际 %q", second, recvFailure.SessionID)
	}
	if recvFailure.ErrorCode != limits.CodeTooManyConcurrent {
		t.Fatalf("接收方侧错误码应为 %s，实际 %s", limits.CodeTooManyConcurrent, recvFailure.ErrorCode)
	}

	// 发送方（ToDevice）——走 rejectToPeer。少了这一半，发送方就永久挂起。
	sendEnv := readAction(t, ctx, sender, protocol.ActionTransferFailure)
	var sendFailure protocol.TransferFailurePayload
	if err := json.Unmarshal(sendEnv.Payload, &sendFailure); err != nil {
		t.Fatalf("发送方侧失败信解析失败: %v", err)
	}
	if sendFailure.SessionID != second {
		t.Fatalf("发送方侧失败信的 session_id 应为 %s，实际 %q", second, sendFailure.SessionID)
	}
	if sendFailure.ErrorCode != limits.CodeTooManyConcurrent {
		t.Fatalf("发送方侧错误码应为 %s，实际 %s", limits.CodeTooManyConcurrent, sendFailure.ErrorCode)
	}
	if !strings.Contains(sendFailure.ErrorMessage, "1") {
		t.Fatalf("文案应写明上限，实际: %q", sendFailure.ErrorMessage)
	}
}

// 被拒的 ANSWER 不得被转发给发送方——转发一条无 token 的 answer 正是改造前的
// 行为，客户端 `if let Some(token)` 会静默跳过，发送方永久挂起。
func TestRejectedAnswerIsNotForwarded(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	lim := limits.FromEnv()
	lim.MaxConcurrentTransfers = 1

	h := newLimitsHarness(t, lim)
	sender, _ := h.connect(t, ctx, "acct", "dev-sender")
	receiver, _ := h.connect(t, ctx, "acct", "dev-receiver")

	first := uuid.NewString()
	sendAnswer(t, ctx, receiver, "dev-receiver", "dev-sender", first)
	firstEnv := readAction(t, ctx, sender, protocol.ActionTransferAnswer)
	var firstAnswer protocol.TransferAnswerPayload
	_ = json.Unmarshal(firstEnv.Payload, &firstAnswer)
	if firstAnswer.Token == "" {
		t.Fatal("第一个会话应当被授权并带回 token")
	}

	second := uuid.NewString()
	sendAnswer(t, ctx, receiver, "dev-receiver", "dev-sender", second)

	// 发送方接下来只应看到 TRANSFER_FAILURE，绝不应看到第二个会话的 ANSWER
	readCtx, readCancel := context.WithTimeout(ctx, 1500*time.Millisecond)
	defer readCancel()
	sawFailure := false
	for {
		_, msgBytes, err := sender.Read(readCtx)
		if err != nil {
			break
		}
		var env protocol.ControlEnvelope
		if json.Unmarshal(msgBytes, &env) != nil {
			continue
		}
		switch env.Action {
		case protocol.ActionTransferAnswer:
			var a protocol.TransferAnswerPayload
			_ = json.Unmarshal(env.Payload, &a)
			if a.SessionID == second {
				t.Fatal("被并发上限拒绝的 ANSWER 不得转发给发送方")
			}
		case protocol.ActionTransferFailure:
			var f protocol.TransferFailurePayload
			_ = json.Unmarshal(env.Payload, &f)
			if f.SessionID == second {
				sawFailure = true
			}
		}
	}
	if !sawFailure {
		t.Fatal("发送方必须收到第二个会话的失败通知，否则它会永久挂起")
	}
}

// 上限之内的 ANSWER 照常授权并回传 token——否则上面两条用「全都拒绝」也能通过。
func TestAnswerWithinConcurrencyLimitStillAuthorized(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	h := newLimitsHarness(t, limits.FromEnv()) // 默认上限 8
	sender, _ := h.connect(t, ctx, "acct", "dev-sender")
	receiver, _ := h.connect(t, ctx, "acct", "dev-receiver")

	sessionID := uuid.NewString()
	sendAnswer(t, ctx, receiver, "dev-receiver", "dev-sender", sessionID)

	env := readAction(t, ctx, sender, protocol.ActionTransferAnswer)
	var answer protocol.TransferAnswerPayload
	_ = json.Unmarshal(env.Payload, &answer)
	if answer.Token == "" {
		t.Fatal("上限之内的 ANSWER 必须被授权并带回 token")
	}
}
