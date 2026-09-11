package protocol

import (
	"bytes"
	"crypto/rand"
	"testing"

	"github.com/google/uuid"
)

func TestBinaryHeaderEncodeDecode(t *testing.T) {
	sessionID := uuid.New()
	payload := []byte("hello UniDrop streaming binary chunk")

	hdr := NewDataHeader(sessionID, 0, 1, 10, payload)
	hdr.Flags |= FlagEncrypted

	buf := make([]byte, HeaderSize)
	if err := hdr.Encode(buf); err != nil {
		t.Fatalf("Encode failed: %v", err)
	}

	decoded, err := DecodeBinaryHeader(buf)
	if err != nil {
		t.Fatalf("Decode failed: %v", err)
	}

	if decoded.Magic != MagicNumber {
		t.Errorf("Magic mismatch: got 0x%04x want 0x%04x", decoded.Magic, MagicNumber)
	}
	if decoded.Version != CurrentVersion {
		t.Errorf("Version mismatch: got %d want %d", decoded.Version, CurrentVersion)
	}
	if decoded.ChunkType != ChunkTypeData {
		t.Errorf("ChunkType mismatch: got %d want %d", decoded.ChunkType, ChunkTypeData)
	}
	if decoded.SessionUUID() != sessionID {
		t.Errorf("SessionID mismatch: got %s want %s", decoded.SessionUUID(), sessionID)
	}
	if decoded.ItemIndex != 0 || decoded.ChunkIndex != 1 || decoded.TotalChunks != 10 {
		t.Errorf("Index mismatch: got item=%d chunk=%d total=%d", decoded.ItemIndex, decoded.ChunkIndex, decoded.TotalChunks)
	}
	if decoded.PayloadLen != uint32(len(payload)) {
		t.Errorf("PayloadLen mismatch: got %d want %d", decoded.PayloadLen, len(payload))
	}
	if decoded.Flags != (FlagEncrypted) {
		t.Errorf("Flags mismatch: got %d want %d", decoded.Flags, FlagEncrypted)
	}

	if err := decoded.VerifyPayloadCRC32(payload); err != nil {
		t.Errorf("CRC32 verify failed: %v", err)
	}
}

func TestBinaryHeaderInvalidMagic(t *testing.T) {
	buf := make([]byte, HeaderSize)
	buf[0] = 0x00
	buf[1] = 0x00

	_, err := DecodeBinaryHeader(buf)
	if err == nil {
		t.Fatal("expected error on invalid magic, got nil")
	}
}

func TestBinaryHeaderPayloadTooLarge(t *testing.T) {
	hdr := NewDataHeader(uuid.New(), 0, 0, 1, nil)
	hdr.PayloadLen = MaxPayloadLength + 100 // Exceed 4MB

	buf := make([]byte, HeaderSize)
	_ = hdr.Encode(buf)

	_, err := DecodeBinaryHeader(buf)
	if err == nil {
		t.Fatal("expected error on payload too large, got nil")
	}
}

func TestAckAndNackHeader(t *testing.T) {
	var sess [16]byte
	_, _ = rand.Read(sess[:])

	ack := NewAckHeader(sess, 2, 5)
	buf := make([]byte, HeaderSize)
	if err := ack.Encode(buf); err != nil {
		t.Fatalf("encode ack failed: %v", err)
	}

	decodedAck, err := DecodeBinaryHeader(buf)
	if err != nil {
		t.Fatalf("decode ack failed: %v", err)
	}
	if decodedAck.ChunkType != ChunkTypeAck {
		t.Errorf("expected ChunkTypeAck, got %d", decodedAck.ChunkType)
	}
	if decodedAck.ItemIndex != 2 || decodedAck.ChunkIndex != 5 {
		t.Errorf("unexpected index: item=%d chunk=%d", decodedAck.ItemIndex, decodedAck.ChunkIndex)
	}
	if decodedAck.PayloadLen != 0 {
		t.Errorf("expected 0 payload length for ACK, got %d", decodedAck.PayloadLen)
	}

	nack := NewNackHeader(sess, 3, 7)
	_ = nack.Encode(buf)
	decodedNack, err := DecodeBinaryHeader(buf)
	if err != nil {
		t.Fatalf("decode nack failed: %v", err)
	}
	if decodedNack.ChunkType != ChunkTypeNack {
		t.Errorf("expected ChunkTypeNack, got %d", decodedNack.ChunkType)
	}
}

func TestCRC32CorruptionDetection(t *testing.T) {
	payload := []byte("authentic payload content")
	hdr := NewDataHeader(uuid.New(), 0, 0, 1, payload)

	corrupted := bytes.Clone(payload)
	corrupted[0] ^= 0xFF

	if err := hdr.VerifyPayloadCRC32(corrupted); err == nil {
		t.Fatal("expected CRC32 verification error for corrupted payload, got nil")
	}
}
