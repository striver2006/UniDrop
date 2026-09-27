use std::io::{self, Error, ErrorKind};
use uuid::Uuid;

pub const HEADER_SIZE: usize = 64;
pub const MAGIC_NUMBER: u16 = 0x5544; // "UD"
pub const CURRENT_VERSION: u8 = 0x01;
pub const MAX_PAYLOAD_LENGTH: u32 = 4 * 1024 * 1024; // 4MB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ChunkType {
    Data = 0x01,
    Ack = 0x02,
    Nack = 0x03,
    Probe = 0x04,
}

impl TryFrom<u8> for ChunkType {
    type Error = io::Error;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0x01 => Ok(ChunkType::Data),
            0x02 => Ok(ChunkType::Ack),
            0x03 => Ok(ChunkType::Nack),
            0x04 => Ok(ChunkType::Probe),
            _ => Err(Error::new(ErrorKind::InvalidData, format!("Unknown ChunkType: 0x{:02x}", v))),
        }
    }
}

pub const FLAG_ENCRYPTED: u32 = 1 << 0;
pub const FLAG_COMPRESSED: u32 = 1 << 1;
pub const FLAG_LAST_CHUNK: u32 = 1 << 2;

pub fn compute_crc32(data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryHeader {
    pub magic: u16,
    pub version: u8,
    pub chunk_type: ChunkType,
    pub session_id: [u8; 16],
    pub item_index: u32,
    pub chunk_index: u32,
    pub total_chunks: u32,
    pub payload_len: u32,
    pub checksum: u32,
    pub nonce: [u8; 12],
    pub flags: u32,
    pub reserved: [u8; 8],
}

impl BinaryHeader {
    pub fn new_data(session_id: Uuid, item_index: u32, chunk_index: u32, total_chunks: u32, payload: &[u8]) -> Self {
        let checksum = compute_crc32(payload);
        let mut flags = 0u32;
        if chunk_index + 1 == total_chunks {
            flags |= FLAG_LAST_CHUNK;
        }

        Self {
            magic: MAGIC_NUMBER,
            version: CURRENT_VERSION,
            chunk_type: ChunkType::Data,
            session_id: *session_id.as_bytes(),
            item_index,
            chunk_index,
            total_chunks,
            payload_len: payload.len() as u32,
            checksum,
            nonce: [0u8; 12],
            flags,
            reserved: [0u8; 8],
        }
    }

    pub fn new_ack(session_id: [u8; 16], item_index: u32, chunk_index: u32) -> Self {
        Self {
            magic: MAGIC_NUMBER,
            version: CURRENT_VERSION,
            chunk_type: ChunkType::Ack,
            session_id,
            item_index,
            chunk_index,
            total_chunks: 0,
            payload_len: 0,
            checksum: 0,
            nonce: [0u8; 12],
            flags: 0,
            reserved: [0u8; 8],
        }
    }

    pub fn new_nack(session_id: [u8; 16], item_index: u32, chunk_index: u32) -> Self {
        Self {
            magic: MAGIC_NUMBER,
            version: CURRENT_VERSION,
            chunk_type: ChunkType::Nack,
            session_id,
            item_index,
            chunk_index,
            total_chunks: 0,
            payload_len: 0,
            checksum: 0,
            nonce: [0u8; 12],
            flags: 0,
            reserved: [0u8; 8],
        }
    }

    pub fn encode(&self, dst: &mut [u8]) -> io::Result<()> {
        if dst.len() < HEADER_SIZE {
            return Err(Error::new(ErrorKind::UnexpectedEof, "buffer too small for 64-byte header"));
        }

        dst[0..2].copy_from_slice(&self.magic.to_be_bytes());
        dst[2] = self.version;
        dst[3] = self.chunk_type as u8;
        dst[4..20].copy_from_slice(&self.session_id);
        dst[20..24].copy_from_slice(&self.item_index.to_be_bytes());
        dst[24..28].copy_from_slice(&self.chunk_index.to_be_bytes());
        dst[28..32].copy_from_slice(&self.total_chunks.to_be_bytes());
        dst[32..36].copy_from_slice(&self.payload_len.to_be_bytes());
        dst[36..40].copy_from_slice(&self.checksum.to_be_bytes());
        dst[40..52].copy_from_slice(&self.nonce);
        dst[52..56].copy_from_slice(&self.flags.to_be_bytes());
        dst[56..64].copy_from_slice(&self.reserved);

        Ok(())
    }

    pub fn decode(src: &[u8]) -> io::Result<Self> {
        if src.len() < HEADER_SIZE {
            return Err(Error::new(ErrorKind::UnexpectedEof, "buffer too small for 64-byte header"));
        }

        let magic = u16::from_be_bytes([src[0], src[1]]);
        if magic != MAGIC_NUMBER {
            return Err(Error::new(ErrorKind::InvalidData, format!("Invalid magic: 0x{:04x}", magic)));
        }

        let version = src[2];
        if version != CURRENT_VERSION {
            return Err(Error::new(ErrorKind::InvalidData, format!("Unsupported version: {}", version)));
        }

        let chunk_type = ChunkType::try_from(src[3])?;

        let mut session_id = [0u8; 16];
        session_id.copy_from_slice(&src[4..20]);

        let item_index = u32::from_be_bytes([src[20], src[21], src[22], src[23]]);
        let chunk_index = u32::from_be_bytes([src[24], src[25], src[26], src[27]]);
        let total_chunks = u32::from_be_bytes([src[28], src[29], src[30], src[31]]);
        let payload_len = u32::from_be_bytes([src[32], src[33], src[34], src[35]]);
        let checksum = u32::from_be_bytes([src[36], src[37], src[38], src[39]]);

        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&src[40..52]);

        let flags = u32::from_be_bytes([src[52], src[53], src[54], src[55]]);

        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&src[56..64]);

        if payload_len > MAX_PAYLOAD_LENGTH {
            return Err(Error::new(ErrorKind::InvalidData, format!("Payload too large: {} bytes", payload_len)));
        }

        Ok(Self {
            magic,
            version,
            chunk_type,
            session_id,
            item_index,
            chunk_index,
            total_chunks,
            payload_len,
            checksum,
            nonce,
            flags,
            reserved,
        })
    }

    pub fn verify_crc32(&self, payload: &[u8]) -> io::Result<()> {
        if payload.len() as u32 != self.payload_len {
            return Err(Error::new(ErrorKind::InvalidData, "Payload length mismatch"));
        }
        let actual = compute_crc32(payload);
        if actual != self.checksum {
            return Err(Error::new(ErrorKind::InvalidData, format!("CRC32 mismatch: declared=0x{:08x}, actual=0x{:08x}", self.checksum, actual)));
        }
        Ok(())
    }
}
