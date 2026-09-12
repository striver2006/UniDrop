// diag-sender is a headless diagnostic client that authenticates to a real
// relay server, sends a small TEXT offer to a target device, dials the data
// plane as sender and pushes one chunk. Mirrors the Rust client's signaling
// (accept-any TLS, same URL params) for production-chain verification.
package main

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
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
	target := flag.String("target", "", "target device id (receiver)")
	flag.Parse()

	if *psk == "" || *target == "" {
		fmt.Fprintln(os.Stderr, "psk and target are required")
		os.Exit(1)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 120*time.Second)
	defer cancel()

	verifier, err := auth.NewVerifier(*psk)
	if err != nil {
		panic(err)
	}
	deviceID := uuid.NewString()
	insecureHTTP := &http.Client{Transport: &http.Transport{TLSClientConfig: &tls.Config{InsecureSkipVerify: true}}}

	fmt.Printf("[diag-sender] dialing control as %s, target %s\n", deviceID, *target)
	conn, _, err := websocket.Dial(ctx, *serverURL+"/ws/control", &websocket.DialOptions{HTTPClient: insecureHTTP})
	if err != nil {
		panic(err)
	}
	defer conn.Close(websocket.StatusNormalClosure, "done")
	conn.SetReadLimit(512 * 1024)

	// Auth handshake
	_, challengeBytes, err := conn.Read(ctx)
	if err != nil {
		panic(err)
	}
	var challengeEnv protocol.ControlEnvelope
	if err := json.Unmarshal(challengeBytes, &challengeEnv); err != nil {
		panic(err)
	}
	var challengePayload protocol.AuthChallengePayload
	_ = json.Unmarshal(challengeEnv.Payload, &challengePayload)

	now := time.Now().UnixMilli()
	nonce := uuid.NewString()
	sig := verifier.GenerateSignatureWithSalt(*account, deviceID, nonce, now, challengePayload.NonceSalt)

	reqPayload, _ := json.Marshal(protocol.AuthRequestPayload{
		AccountID:  *account,
		DeviceID:   deviceID,
		Hostname:   "diag-sender",
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

	// Build and send the offer while waiting for the answer
	payload := []byte("diag-e2e-clipboard-text-payload-0123456789")
	sum := sha256.Sum256(payload)
	sessionUUID := uuid.New()

	offerPayload, _ := json.Marshal(protocol.TransferOfferPayload{
		SessionID:      sessionUUID.String(),
		DataType:       "TEXT",
		TotalSize:      int64(len(payload)),
		TotalItems:     1,
		PreviewSummary: "文本: diag-e2e",
		Items: []protocol.TransferItemPayload{{
			ItemIndex:    0,
			RelativePath: "clipboard.txt",
			Size:         int64(len(payload)),
			IsDir:        false,
			SHA256:       hex.EncodeToString(sum[:]),
			TotalChunks:  1,
		}},
	})
	offerEnv, _ := json.Marshal(protocol.ControlEnvelope{
		Version:    1,
		TraceID:    uuid.NewString(),
		Action:     protocol.ActionTransferOffer,
		FromDevice: deviceID,
		ToDevice:   *target,
		Timestamp:  time.Now().UnixMilli(),
		Payload:    offerPayload,
	})

	var token string
	sent := false
	for token == "" {
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
			fmt.Printf("[diag-sender] auth success=%v\n", resp.Success)
			if !resp.Success {
				panic("auth rejected: " + resp.ErrorMessage)
			}
			if err := conn.Write(ctx, websocket.MessageText, offerEnv); err != nil {
				panic(err)
			}
			sent = true
			fmt.Println("[diag-sender] offer sent")
		case protocol.ActionDeviceListSync:
			// may arrive before/after auth response; send offer once ready
			if !sent {
				if err := conn.Write(ctx, websocket.MessageText, offerEnv); err != nil {
					panic(err)
				}
				sent = true
				fmt.Println("[diag-sender] offer sent (after sync)")
			}
		case protocol.ActionTransferAnswer:
			var answer protocol.TransferAnswerPayload
			if err := json.Unmarshal(env.Payload, &answer); err == nil && answer.Accepted && answer.Token != "" {
				token = answer.Token
				fmt.Printf("[diag-sender] answer accepted, token %s...\n", token[:8])
			}
		}
	}

	// Dial data plane as sender
	dataURL := fmt.Sprintf("%s/ws/data?session_id=%s&role=sender&device_id=%s&target_device_id=%s&token=%s",
		*serverURL, sessionUUID.String(), deviceID, *target, token)
	dataConn, _, err := websocket.Dial(ctx, dataURL, &websocket.DialOptions{HTTPClient: insecureHTTP})
	if err != nil {
		panic("data dial failed: " + err.Error())
	}
	defer dataConn.Close(websocket.StatusNormalClosure, "done")
	dataConn.SetReadLimit(int64(protocol.HeaderSize + protocol.MaxPayloadLength + 1024))
	fmt.Println("[diag-sender] data plane connected, pushing 1 chunk")

	hdr := protocol.NewDataHeader(sessionUUID, 0, 0, 1, payload)
	frame := make([]byte, protocol.HeaderSize+len(payload))
	_ = hdr.Encode(frame[:protocol.HeaderSize])
	copy(frame[protocol.HeaderSize:], payload)
	if err := dataConn.Write(ctx, websocket.MessageBinary, frame); err != nil {
		panic("chunk write failed: " + err.Error())
	}

	// Wait for ACK
	_, ack, err := dataConn.Read(ctx)
	if err != nil {
		panic("ack read failed: " + err.Error())
	}
	ackHdr, err := protocol.DecodeBinaryHeader(ack)
	if err != nil || ackHdr.ChunkType != protocol.ChunkTypeAck {
		panic(fmt.Sprintf("unexpected ack: err=%v type=%v", err, ackHdr.ChunkType))
	}

	fmt.Printf("[diag-sender] SUCCESS: chunk acked (item=%d chunk=%d) — sender->relay->receiver chain OK\n",
		ackHdr.ItemIndex, ackHdr.ChunkIndex)
}
