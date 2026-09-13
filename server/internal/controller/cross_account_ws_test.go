package controller_test

import (
	"context"
	"encoding/json"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/google/uuid"
	"github.com/unidrop/unidrop-server/internal/limits"
	"github.com/unidrop/unidrop-server/internal/protocol"
)

// 跨账号隔离的端到端守卫。
//
// 这一组刻意让两个账号使用**完全相同的 device_id** —— 那正是改造前
// registry 以裸 DeviceID 为键时会互相踢下线的形态。


// readSignal 读到第一条**传输类**信令为止，跳过设备上下线广播。
//
// 同账号内每有一台新设备接入，既有连接都会收到一条 DEVICE_ONLINE；
// 逐个数着消费会让用例与握手顺序耦合，改一行连接次序就全线红。
func readSignal(t *testing.T, ctx context.Context, ws *websocket.Conn) protocol.ControlEnvelope {
	t.Helper()
	for {
		_, raw, err := ws.Read(ctx)
		if err != nil {
			t.Fatalf("读取信令失败：%v", err)
		}
		var env protocol.ControlEnvelope
		_ = json.Unmarshal(raw, &env)
		switch env.Action {
		case protocol.ActionDeviceOnline, protocol.ActionDeviceOffline,
			protocol.ActionDeviceListSync, protocol.ActionHeartbeatPong:
			continue
		}
		return env
	}
}

// 用户最在意的那条：另一个账号用同名 device_id 连上来，
// 本账号的设备不得掉线，也不得看见对方。
func TestCrossAccountSameDeviceIDDoesNotKickExistingPeer(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	wsA, _ := h.connect(t, ctx, "acctA", "devX")
	wsB, _ := h.connect(t, ctx, "acctB", "devX")

	if got := h.registry.Count(); got != 2 {
		t.Fatalf("两个账号的同名设备应当共存，期望 2 个会话，实得 %d", got)
	}
	if got := h.registry.CountAccounts(); got != 2 {
		t.Fatalf("在线账号数应为 2，实得 %d", got)
	}

	// A 的连接必须仍然可用：发一次 PING 并读回 PONG。
	pingEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:   1,
		TraceID:   uuid.NewString(),
		Action:    protocol.ActionHeartbeatPing,
		Timestamp: time.Now().UnixMilli(),
	})
	if err := wsA.Write(ctx, websocket.MessageText, pingEnv); err != nil {
		t.Fatalf("B 账号接入后，A 的连接不得失效：%v", err)
	}
	readCtx, readCancel := context.WithTimeout(ctx, 2*time.Second)
	defer readCancel()
	_, pongBytes, err := wsA.Read(readCtx)
	if err != nil {
		t.Fatalf("A 的连接必须仍能收到 PONG：%v", err)
	}
	var pongEnv protocol.ControlEnvelope
	_ = json.Unmarshal(pongBytes, &pongEnv)
	if pongEnv.Action != protocol.ActionHeartbeatPong {
		t.Fatalf("期望 PONG，实得 %s", pongEnv.Action)
	}

	_ = wsB
}

// A 账号内发给 devX 的信令只能到 A 的那台，B 的同名设备必须读超时。
func TestRoutingNeverCrossesAccountWithIdenticalDeviceID(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	wsA1, _ := h.connect(t, ctx, "acctA", "devX")
	wsA2, _ := h.connect(t, ctx, "acctA", "devPeer")
	wsB, _ := h.connect(t, ctx, "acctB", "devX")

	// A2 → devX（同账号内）
	offer, _ := json.Marshal(protocol.TransferOfferPayload{
		SessionID:      uuid.NewString(),
		DataType:       "TEXT",
		TotalSize:      4,
		TotalItems:     1,
		PreviewSummary: "hi",
	})
	env, _ := json.Marshal(protocol.ControlEnvelope{
		Version:   1,
		TraceID:   uuid.NewString(),
		Action:    protocol.ActionTransferOffer,
		ToDevice:  "devX",
		Timestamp: time.Now().UnixMilli(),
		Payload:   offer,
	})
	if err := wsA2.Write(ctx, websocket.MessageText, env); err != nil {
		t.Fatalf("发送 OFFER 失败：%v", err)
	}

	// A 账号的 devX 必须收到
	recvCtx, recvCancel := context.WithTimeout(ctx, 3*time.Second)
	defer recvCancel()
	gotEnv := readSignal(t, recvCtx, wsA1)
	if gotEnv.Action != protocol.ActionTransferOffer {
		t.Fatalf("期望 TRANSFER_OFFER，实得 %s", gotEnv.Action)
	}

	// B 账号的同名 devX 必须什么都收不到
	quietCtx, quietCancel := context.WithTimeout(ctx, 500*time.Millisecond)
	defer quietCancel()
	if _, leaked, err := wsB.Read(quietCtx); err == nil {
		t.Fatalf("另一个账号的同名设备不得收到任何信令，却收到了：%s", leaked)
	}
}

// A3 改动 B 的端到端守卫，也是本轮杀伤力最大的一条。
// B 拿着 A 的 session_id 发 CANCEL，A 的管道与授权条目必须纹丝不动。
func TestCrossAccountPipeTeardownBlocked(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	_, _ = h.connect(t, ctx, "acctA", "devA1")
	wsB, _ := h.connect(t, ctx, "acctB", "devB1")

	// 直接在 relay 上铺好 A 账号的授权与管道
	sessionID := uuid.NewString()
	token, _, err := h.relay.AuthorizeSessionIfUnderLimit(
		sessionID, "devA1", "devA2", "acctA", 5*time.Minute, 0)
	if err != nil {
		t.Fatalf("铺设 A 账号的授权失败：%v", err)
	}
	if _, err := h.relay.ValidateAndGetOrCreatePipe(sessionID, "sender", "devA1", "devA2", token); err != nil {
		t.Fatalf("铺设 A 账号的管道失败：%v", err)
	}

	// B 自报 A 的 session_id 发 CANCEL
	cancelPayload, _ := json.Marshal(map[string]string{"session_id": sessionID})
	cancelEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:   1,
		TraceID:   uuid.NewString(),
		Action:    protocol.ActionTransferCancel,
		ToDevice:  "devB2",
		Timestamp: time.Now().UnixMilli(),
		Payload:   cancelPayload,
	})
	if err := wsB.Write(ctx, websocket.MessageText, cancelEnv); err != nil {
		t.Fatalf("发送 CANCEL 失败：%v", err)
	}

	// 给服务端一点处理时间
	time.Sleep(200 * time.Millisecond)

	// A 的管道必须还在，且 token 仍然有效
	if _, ok := h.relay.GetPipe(sessionID); !ok {
		t.Fatal("另一个账号自报 session_id 不得拆掉本账号的管道")
	}
	if err := h.relay.CheckAuthorization(sessionID, "sender", "devA1", "devA2", token); err != nil {
		t.Fatalf("A 账号的授权条目不得被连带删除，实得 %v", err)
	}
}

// A3 改动 C：授权失败必须双向拒绝，而不是转发一个没有 token 的 ANSWER。
// 转发会让发送方永久挂起（客户端 `if let Some(token)` 静默跳过）。
func TestAnswerAuthorizationFailureRejectsBothPeersInsteadOfForwarding(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	wsSender, _ := h.connect(t, ctx, "acctA", "devSender")
	wsReceiver, _ := h.connect(t, ctx, "acctA", "devReceiver")

	// 先让另一组设备占住这个 session_id，制造 ErrSessionIDConflict
	sessionID := uuid.NewString()
	if _, _, err := h.relay.AuthorizeSessionIfUnderLimit(
		sessionID, "otherSender", "otherReceiver", "acctA", 5*time.Minute, 0); err != nil {
		t.Fatalf("铺设占位授权失败：%v", err)
	}

	answer, _ := json.Marshal(protocol.TransferAnswerPayload{
		SessionID: sessionID,
		Accepted:  true,
	})
	answerEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:   1,
		TraceID:   uuid.NewString(),
		Action:    protocol.ActionTransferAnswer,
		ToDevice:  "devSender",
		Timestamp: time.Now().UnixMilli(),
		Payload:   answer,
	})
	if err := wsReceiver.Write(ctx, websocket.MessageText, answerEnv); err != nil {
		t.Fatalf("发送 ANSWER 失败：%v", err)
	}

	// 接收方（当前连接）必须收到 TRANSFER_FAILURE
	recvCtx, recvCancel := context.WithTimeout(ctx, 3*time.Second)
	defer recvCancel()
	selfEnv := readSignal(t, recvCtx, wsReceiver)
	if selfEnv.Action != protocol.ActionTransferFailure {
		t.Fatalf("接收方期望 TRANSFER_FAILURE，实得 %s", selfEnv.Action)
	}

	// 发送方必须收到 TRANSFER_FAILURE，且**不是**一个没有 token 的 ANSWER
	sendCtx, sendCancel := context.WithTimeout(ctx, 3*time.Second)
	defer sendCancel()
	peerEnv := readSignal(t, sendCtx, wsSender)
	if peerEnv.Action == protocol.ActionTransferAnswer {
		t.Fatal("授权失败时不得把没有 token 的 ANSWER 转发给发送方：客户端会静默跳过并永久等待")
	}
	if peerEnv.Action != protocol.ActionTransferFailure {
		t.Fatalf("发送方期望 TRANSFER_FAILURE，实得 %s", peerEnv.Action)
	}

	var failure protocol.TransferFailurePayload
	_ = json.Unmarshal(peerEnv.Payload, &failure)
	if failure.ErrorCode != limits.CodeSessionConflict {
		t.Fatalf("期望错误码 %s，实得 %s", limits.CodeSessionConflict, failure.ErrorCode)
	}
}
