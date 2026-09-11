use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::core::cache_manager::CacheManager;
use crate::core::path_guard::PathGuard;
use crate::protocol::{BinaryHeader, TransferItemPayload, TransferOfferPayload, MAX_PAYLOAD_LENGTH};

pub struct TransferEngine {
    cache_manager: CacheManager,
}

impl TransferEngine {
    pub fn new(cache_manager: CacheManager) -> Self {
        Self { cache_manager }
    }

    /// Prepares a TransferOfferPayload from a list of local file paths.
    pub fn prepare_offer(session_id: Uuid, file_paths: &[PathBuf]) -> Result<TransferOfferPayload, String> {
        let mut items = Vec::new();
        let mut total_size = 0i64;

        for (idx, p) in file_paths.iter().enumerate() {
            let metadata = fs::metadata(p).map_err(|e| format!("cannot read metadata for {:?}: {}", p, e))?;
            let file_size = metadata.len() as i64;
            total_size += file_size;

            let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("unnamed").to_string();

            // Compute SHA-256
            let mut file = File::open(p).map_err(|e| format!("cannot open {:?}: {}", p, e))?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = file.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            let hash = hex::encode(hasher.finalize());

            let total_chunks = if file_size == 0 {
                1
            } else {
                ((file_size + (MAX_PAYLOAD_LENGTH as i64) - 1) / (MAX_PAYLOAD_LENGTH as i64)) as u32
            };

            items.push(TransferItemPayload {
                item_index: idx as u32,
                relative_path: file_name,
                size: file_size,
                is_dir: metadata.is_dir(),
                sha256: hash,
                total_chunks,
            });
        }

        let summary = if items.len() == 1 {
            items[0].relative_path.clone()
        } else {
            format!("{} 等 {} 个文件", items[0].relative_path, items.len())
        };

        Ok(TransferOfferPayload {
            session_id: session_id.to_string(),
            data_type: "FILES".to_string(),
            total_size,
            total_items: items.len(),
            preview_summary: summary,
            encrypted: false,
            encrypted_metadata: None,
            items,
        })
    }

    /// Handles an incoming chunk and writes it into the local cache sandbox.
    pub async fn process_incoming_chunk(
        &self,
        session_id: &str,
        item: &TransferItemPayload,
        header: &BinaryHeader,
        payload: &[u8],
    ) -> Result<bool, String> {
        // 1. PathGuard: check path traversal security
        let safe_target = PathGuard::sanitize_and_resolve(self.cache_manager.cache_root(), &item.relative_path)
            .map_err(|e| format!("Path traversal error: {}", e))?;

        if let Some(parent) = safe_target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create dir failed: {}", e))?;
        }

        // 2. Open file and write chunk at designated offset
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&safe_target)
            .map_err(|e| format!("cannot open cache file: {}", e))?;

        let offset = (header.chunk_index as u64) * (MAX_PAYLOAD_LENGTH as u64);
        file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        file.write_all(payload).map_err(|e| e.to_string())?;

        // 3. Register in cache DB
        self.cache_manager.register_entry(&safe_target, session_id, item.size).await?;

        // Check if item is finished (last chunk)
        let is_last = header.chunk_index + 1 == header.total_chunks;
        Ok(is_last)
    }
}
