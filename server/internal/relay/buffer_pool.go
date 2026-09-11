package relay

import "sync"

const (
	// MaxChunkDataSize is 4MB.
	MaxChunkDataSize = 4 * 1024 * 1024

	// BufferSize accommodates the 64-byte binary header + 4MB payload.
	BufferSize = 64 + MaxChunkDataSize
)

// BufferPool manages reusable byte slices to prevent high-frequency GC pauses.
type BufferPool struct {
	pool sync.Pool
}

// NewBufferPool initializes a BufferPool.
func NewBufferPool() *BufferPool {
	return &BufferPool{
		pool: sync.Pool{
			New: func() any {
				buf := make([]byte, BufferSize)
				return &buf
			},
		},
	}
}

// Get borrows a buffer from the pool.
func (bp *BufferPool) Get() *[]byte {
	return bp.pool.Get().(*[]byte)
}

// Put returns a buffer to the pool.
func (bp *BufferPool) Put(buf *[]byte) {
	if buf == nil || cap(*buf) < BufferSize {
		return
	}
	// Reset slice to full capacity
	*buf = (*buf)[:BufferSize]
	bp.pool.Put(buf)
}

// AckBufferSize accommodates the 64-byte binary header with 0 payload for ACKs/NACKs.
const AckBufferSize = 64

// AckBufferPool manages reusable small byte slices for ACK/NACK frames (P2-2).
type AckBufferPool struct {
	pool sync.Pool
}

// NewAckBufferPool initializes an AckBufferPool.
func NewAckBufferPool() *AckBufferPool {
	return &AckBufferPool{
		pool: sync.Pool{
			New: func() any {
				buf := make([]byte, AckBufferSize)
				return &buf
			},
		},
	}
}

// Get borrows a small buffer from the pool.
func (bp *AckBufferPool) Get() *[]byte {
	return bp.pool.Get().(*[]byte)
}

// Put returns a small buffer to the pool.
func (bp *AckBufferPool) Put(buf *[]byte) {
	if buf == nil || cap(*buf) < AckBufferSize {
		return
	}
	*buf = (*buf)[:AckBufferSize]
	bp.pool.Put(buf)
}
