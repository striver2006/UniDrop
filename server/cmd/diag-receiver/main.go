// diag-receiver is a headless diagnostic client that authenticates to a real
// relay server as an extra device, auto-accepts one incoming transfer offer,
// dials the data plane as receiver and ACKs every chunk. Used to verify the
// full sender→relay→receiver chain against production deployments.
package main

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"flag"
	"fmt"
	"net/http"
	"os"
	"time"

	"github.com/coder/websocket"
	"github.com/google/uuid"
	"github.com/unidrop/unidrop-server/internal/auth"
	"github.com/unidrop/unidrop-server/internal/protocol"
)

func main() {
	serverURL := flag.String("server", "wss://127.0.0.1:58921", "server base ws url")
	account := flag.String("account", "default_user", "account id")
	psk := flag.String("psk", "", "psk secret")
	hostname := flag.String("hostname", "diag-receiver", "hostname to announce")
	flag.Parse()

	if *psk == "" {
		fmt.Fprintln(os.Stderr, "psk is required")
		os.Exit(1)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 300*time.Second)
	defer cancel()

	verifier, err := auth.NewVerifier(*psk)
	if err != nil {
		panic(err)
	}
	deviceID := uuid.NewString()

	insecureHTTP := &http.Client{Transport: &http.Transport{TLSClientConfig: &tls.Config{InsecureSkipVerify: true}}}

	fmt.Printf("[diag] dialing control %s/ws/control as %s (%s)\n", *serverURL, deviceID, *hostname)
	conn, _, err := websocket.Dial(ctx, *serverURL+"/ws/control", &websocket.DialOptions{HTTPClient: insecureHTTP})
	if err != nil {
		panic(err)
	}
	defer conn.Close(websocket.StatusNormalClosure, "done")
	conn.SetReadLimit(512 * 1024)

	// 1. Auth handshake
	_, challengeBytes, err := conn.Read(ctx)
	if err != nil {
		panic(err)
	}
	var challengeEnv protocol.ControlEnvelope
	if err := json.Unmarshal(challengeBytes, &challengeEnv); err != nil || challengeEnv.Action != protocol.ActionAuthChallenge {
		panic(fmt.Sprintf("unexpected challenge: %s", challengeBytes))
	}
	var challengePayload protocol.AuthChallengePayload
	_ = json.Unmarshal(challengeEnv.Payload, &challengePayload)

	now := time.Now().UnixMilli()
	nonce := uuid.NewString()
	sig := verifier.GenerateSignatureWithSalt(*account, deviceID, nonce, now, challengePayload.NonceSalt)

	reqPayload, _ := json.Marshal(protocol.AuthRequestPayload{
		AccountID:  *account,
		DeviceID:   deviceID,
		Hostname:   *hostname,
		OSType:     "linux",
		AppVersion: "diag-0.1.0",
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
	if err := conn.Write(ctx, websocket.MessageText, reqEnv); err != nil {
		panic(err)
	}

	// 2. Wait for offer / answer echo
	var offer protocol.TransferOfferPayload
	var answerToken string
	var senderDevice string
	var totalExpected uint32

	for answerToken == "" {
		_, msgBytes, err := conn.Read(ctx)
		if err != nil {
			panic(err)
		}
		var env protocol.ControlEnvelope
		if err := json.Unmarshal(msgBytes, &env); err != nil {
			continue
		}

		switch env.Action {
		case protocol.ActionAuthResponse:
			var resp protocol.AuthResponsePayload
			_ = json.Unmarshal(env.Payload, &resp)
			fmt.Printf("[diag] auth response: success=%v\n", resp.Success)
			if !resp.Success {
				panic("auth rejected: " + resp.ErrorMessage)
			}
		case protocol.ActionDeviceListSync:
			fmt.Println("[diag] device list sync received (waiting as extra device)")
		case protocol.ActionTransferOffer:
			if err := json.Unmarshal(env.Payload, &offer); err != nil {
				panic(err)
			}
			senderDevice = env.FromDevice
			for _, it := range offer.Items {
				totalExpected += it.TotalChunks
			}
			fmt.Printf("[diag] offer received: session=%s type=%s items=%d total_chunks=%d summary=%s\n",
				offer.SessionID, offer.DataType, offer.TotalItems, totalExpected, offer.PreviewSummary)

			answerPayload, _ := json.Marshal(protocol.TransferAnswerPayload{
				SessionID: offer.SessionID,
				Accepted:  true,
			})
			answerEnv, _ := json.Marshal(protocol.ControlEnvelope{
				Version:    1,
				TraceID:    uuid.NewString(),
				Action:     protocol.ActionTransferAnswer,
				FromDevice: deviceID,
				ToDevice:   senderDevice,
				Timestamp:  time.Now().UnixMilli(),
				Payload:    answerPayload,
			})
			if err := conn.Write(ctx, websocket.MessageText, answerEnv); err != nil {
				panic(err)
			}
			fmt.Println("[diag] answer(accepted) sent, waiting for token echo")
		case protocol.ActionTransferAnswer:
			var answer protocol.TransferAnswerPayload
			if err := json.Unmarshal(env.Payload, &answer); err == nil && answer.Accepted && answer.Token != "" {
				answerToken = answer.Token
				fmt.Printf("[diag] token echo received: %s...\n", answerToken[:8])
			}
		default:
			// heartbeats, device online etc.
		}
	}

	// 3. Dial data plane as receiver
	dataURL := fmt.Sprintf("%s/ws/data?session_id=%s&role=receiver&device_id=%s&target_device_id=%s&token=%s",
		*serverURL, offer.SessionID, deviceID, senderDevice, answerToken)
	fmt.Printf("[diag] dialing data plane (receiver)\n")
	dataConn, _, err := websocket.Dial(ctx, dataURL, &websocket.DialOptions{HTTPClient: insecureHTTP})
	if err != nil {
		panic("data dial failed: " + err.Error())
	}
	defer dataConn.Close(websocket.StatusNormalClosure, "done")
	dataConn.SetReadLimit(int64(protocol.HeaderSize + protocol.MaxPayloadLength + 1024))
	fmt.Println("[diag] data plane connected, receiving chunks...")

	received := uint32(0)
	payloadBytes := int64(0)
	sessionUUID := uuid.MustParse(offer.SessionID)
	ackIdx := uint32(0)
	seen := map[uint64]bool{}

	start := time.Now()
	for received < totalExpected {
		_, frame, err := dataConn.Read(ctx)
		if err != nil {
			panic("data read failed: " + err.Error())
		}
		hdr, err := protocol.DecodeBinaryHeader(frame)
		if err != nil {
			panic("bad frame header: " + err.Error())
		}
		if hdr.ChunkType != protocol.ChunkTypeData {
			continue
		}
		key := uint64(hdr.ItemIndex)<<32 | uint64(hdr.ChunkIndex)
		ack := protocol.NewAckHeader(sessionUUID, hdr.ItemIndex, hdr.ChunkIndex)
		ackBuf := make([]byte, protocol.HeaderSize)
		_ = ack.Encode(ackBuf)
		if err := dataConn.Write(ctx, websocket.MessageBinary, ackBuf); err != nil {
			panic("ack write failed: " + err.Error())
		}
		if !seen[key] {
			seen[key] = true
			received++
			payloadBytes += int64(hdr.PayloadLen)
		}
		ackIdx++
	}

	fmt.Printf("[diag] SUCCESS: received %d chunks (%d bytes) in %s — sender->relay->receiver chain OK\n",
		received, payloadBytes, time.Since(start).Round(time.Millisecond))
}
