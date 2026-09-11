package protocol

import (
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"

	"github.com/google/uuid"
)

const (
	// HeaderSize is the fixed size of the binary frame header in bytes.
	HeaderSize = 64

	// MagicNumber is the protocol magic "UD" (0x5544).
	MagicNumber uint16 = 0x5544

	// CurrentVersion is the protocol version (0x01).
	CurrentVersion uint8 = 0x01

	// MaxPayloadLength is the maximum allowed payload size for a chunk (4MB).
	MaxPayloadLength uint32 = 4 * 1024 * 1024 // 4,194,304 bytes
)

// ChunkType represents the frame type.
type ChunkType uint8

const (
	ChunkTypeData  ChunkType = 0x01 // Data payload frame
	ChunkTypeAck   ChunkType = 0x02 // Acknowledgement frame
	ChunkTypeNack  ChunkType = 0x03 // Negative acknowledgement frame
	ChunkTypeProbe ChunkType = 0x04 // Keepalive / RTT probe frame
)

// FrameFlags represents bit flags in the header.
type FrameFlags uint32

const (
	FlagEncrypted  FrameFlags = 1 << 0 // Payload is encrypted with E2EE
	FlagCompressed FrameFlags = 1 << 1 // Payload is compressed
	FlagLastChunk  FrameFlags = 1 << 2 // Final chunk of the current item
)

var (
	ErrBufferTooSmall     = errors.New("buffer is smaller than the 64-byte header size")
	ErrInvalidMagic       = errors.New("invalid protocol magic number")
	ErrUnsupportedVersion = errors.New("unsupported protocol version")
	ErrPayloadTooLarge    = errors.New("payload length exceeds maximum 4MB limit")
	ErrChecksumMismatch   = errors.New("CRC32 checksum mismatch")
)

// BinaryHeader represents the 64-byte fixed frame header.
type BinaryHeader struct {
	Magic       uint16     // Offset 0..1
	Version     uint8      // Offset 2
	ChunkType   ChunkType  // Offset 3
	SessionID   [16]byte   // Offset 4..19
	ItemIndex   uint32     // Offset 20..23
	ChunkIndex  uint32     // Offset 24..27
	TotalChunks uint32     // Offset 28..31
	PayloadLen  uint32     // Offset 32..35
	Checksum    uint32     // Offset 36..39 (CRC32-IEEE)
	Nonce       [12]byte   // Offset 40..51 (AES-256-GCM 96-bit Nonce)
	Flags       FrameFlags // Offset 52..55
	Reserved    [8]byte    // Offset 56..63
}

// NewDataHeader creates a standard data chunk header.
func NewDataHeader(sessionID uuid.UUID, itemIndex, chunkIndex, totalChunks uint32, payload []byte) BinaryHeader {
	var sessBytes [16]byte
	copy(sessBytes[:], sessionID[:])

	checksum := crc32.ChecksumIEEE(payload)
	var flags FrameFlags
	if chunkIndex+1 == totalChunks {
		flags |= FlagLastChunk
	}

	return BinaryHeader{
		Magic:       MagicNumber,
		Version:     CurrentVersion,
		ChunkType:   ChunkTypeData,
		SessionID:   sessBytes,
		ItemIndex:   itemIndex,
		ChunkIndex:  chunkIndex,
		TotalChunks: totalChunks,
		PayloadLen:  uint32(len(payload)),
		Checksum:    checksum,
		Flags:       flags,
	}
}

// NewAckHeader creates an ACK response header.
func NewAckHeader(sessionID [16]byte, itemIndex, chunkIndex uint32) BinaryHeader {
	return BinaryHeader{
		Magic:      MagicNumber,
		Version:    CurrentVersion,
		ChunkType:  ChunkTypeAck,
		SessionID:  sessionID,
		ItemIndex:  itemIndex,
		ChunkIndex: chunkIndex,
		PayloadLen: 0,
	}
}

// NewNackHeader creates a NACK response header requesting immediate retransmission.
func NewNackHeader(sessionID [16]byte, itemIndex, chunkIndex uint32) BinaryHeader {
	return BinaryHeader{
		Magic:      MagicNumber,
		Version:    CurrentVersion,
		ChunkType:  ChunkTypeNack,
		SessionID:  sessionID,
		ItemIndex:  itemIndex,
		ChunkIndex: chunkIndex,
		PayloadLen: 0,
	}
}

// Encode serializes the header into a 64-byte buffer (Big-Endian).
func (h *BinaryHeader) Encode(dst []byte) error {
	if len(dst) < HeaderSize {
		return ErrBufferTooSmall
	}

	binary.BigEndian.PutUint16(dst[0:2], h.Magic)
	dst[2] = h.Version
	dst[3] = byte(h.ChunkType)
	copy(dst[4:20], h.SessionID[:])
	binary.BigEndian.PutUint32(dst[20:24], h.ItemIndex)
	binary.BigEndian.PutUint32(dst[24:28], h.ChunkIndex)
	binary.BigEndian.PutUint32(dst[28:32], h.TotalChunks)
	binary.BigEndian.PutUint32(dst[32:36], h.PayloadLen)
	binary.BigEndian.PutUint32(dst[36:40], h.Checksum)
	copy(dst[40:52], h.Nonce[:])
	binary.BigEndian.PutUint32(dst[52:56], uint32(h.Flags))
	copy(dst[56:64], h.Reserved[:])

	return nil
}

// Decode deserializes the header from a 64-byte buffer (Big-Endian) with validation.
func DecodeBinaryHeader(src []byte) (BinaryHeader, error) {
	if len(src) < HeaderSize {
		return BinaryHeader{}, ErrBufferTooSmall
	}

	magic := binary.BigEndian.Uint16(src[0:2])
	if magic != MagicNumber {
		return BinaryHeader{}, fmt.Errorf("%w: got 0x%04x expected 0x%04x", ErrInvalidMagic, magic, MagicNumber)
	}

	version := src[2]
	if version != CurrentVersion {
		return BinaryHeader{}, fmt.Errorf("%w: got %d expected %d", ErrUnsupportedVersion, version, CurrentVersion)
	}

	var h BinaryHeader
	h.Magic = magic
	h.Version = version
	h.ChunkType = ChunkType(src[3])
	copy(h.SessionID[:], src[4:20])
	h.ItemIndex = binary.BigEndian.Uint32(src[20:24])
	h.ChunkIndex = binary.BigEndian.Uint32(src[24:28])
	h.TotalChunks = binary.BigEndian.Uint32(src[28:32])
	h.PayloadLen = binary.BigEndian.Uint32(src[32:36])
	h.Checksum = binary.BigEndian.Uint32(src[36:40])
	copy(h.Nonce[:], src[40:52])
	h.Flags = FrameFlags(binary.BigEndian.Uint32(src[52:56]))
	copy(h.Reserved[:], src[56:64])

	if h.PayloadLen > MaxPayloadLength {
		return BinaryHeader{}, fmt.Errorf("%w: %d bytes", ErrPayloadTooLarge, h.PayloadLen)
	}

	return h, nil
}

// VerifyPayloadCRC32 checks if the payload matches the header's CRC32 checksum.
func (h *BinaryHeader) VerifyPayloadCRC32(payload []byte) error {
	if uint32(len(payload)) != h.PayloadLen {
		return fmt.Errorf("payload length mismatch: header declares %d but received %d", h.PayloadLen, len(payload))
	}
	actual := crc32.ChecksumIEEE(payload)
	if actual != h.Checksum {
		return fmt.Errorf("%w: header=0x%08x actual=0x%08x", ErrChecksumMismatch, h.Checksum, actual)
	}
	return nil
}

// SessionUUID returns the SessionID formatted as a standard google/uuid.UUID.
func (h *BinaryHeader) SessionUUID() uuid.UUID {
	return uuid.UUID(h.SessionID)
}
