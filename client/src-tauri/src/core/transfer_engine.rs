use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::core::cache_manager::CacheManager;
use crate::core::path_guard::PathGuard;
use crate::core::sliding_window::SlidingWindow;
use crate::platform::show_transfer_notification;
use crate::protocol::{
    ActionType, BinaryHeader, ChunkType, ControlEnvelope, HEADER_SIZE, MAX_PAYLOAD_LENGTH,
    TransferItemPayload, TransferOfferPayload,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTransfer {
    pub session_id: String,
    pub preview_summary: String,
    pub total_size: i64,
    pub transferred_size: i64,
    pub direction: String, // "SEND" | "RECEIVE"
    pub progress: f64,     // 0..100
    pub status: String,    // "TRANSFERRING" | "COMPLETED" | "FAILED"
}

pub struct TransferEngine {
    pub cache_manager: CacheManager,
}

impl TransferEngine {
    pub fn new(cache_manager: CacheManager) -> Self {
        Self { cache_manager }
    }

    /// Prepares a TransferOfferPayload and matching valid file paths (R2 / N2: strictly 1:1 index-aligned).
    pub fn prepare_offer(session_id: Uuid, file_paths: &[PathBuf]) -> Result<(TransferOfferPayload, Vec<PathBuf>), String> {
        let mut items = Vec::new();
        let mut valid_paths = Vec::new();
        let mut total_size = 0i64;

        for p in file_paths.iter() {
            let metadata = match fs::metadata(p) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("Skipping unreadable path {:?}: {}", p, e);
                    continue;
                }
            };

            // P3-8: If it is a directory, skip direct File::open
            if metadata.is_dir() {
                log::warn!("Skipping directory {:?} in file transfer offer", p);
                continue;
            }

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
                item_index: items.len() as u32,
                relative_path: file_name,
                size: file_size,
                is_dir: false,
                sha256: hash,
                total_chunks,
            });
            valid_paths.push(p.clone());
        }

        if items.is_empty() {
            return Err("No valid readable files to send".into());
        }

        let summary = if items.len() == 1 {
            items[0].relative_path.clone()
        } else {
            format!("{} 等 {} 个文件", items[0].relative_path, items.len())
        };

        Ok((
            TransferOfferPayload {
                session_id: session_id.to_string(),
                data_type: "FILES".to_string(),
                total_size,
                total_items: items.len(),
                preview_summary: summary,
                encrypted: false,
                encrypted_metadata: None,
                items,
            },
            valid_paths,
        ))
    }

    /// Connects to /ws/data as Sender and executes Sliding Window ARQ transfer (P0-3).
    pub async fn start_sender_task(
        server_url: String,
        session_id: String,
        token: String,
        from_device: String,
        to_device: String,
        file_paths: Vec<PathBuf>,
        offer: TransferOfferPayload,
        outgoing_tx: mpsc::Sender<ControlEnvelope>,
        app_handle: AppHandle,
    ) {
        let base = server_url.trim().trim_end_matches('/');
        let ws_data_url = if base.starts_with("ws://") || base.starts_with("wss://") {
            format!(
                "{}/ws/data?session_id={}&role=sender&device_id={}&target_device_id={}&token={}",
                base, session_id, from_device, to_device, token
            )
        } else {
            format!(
                "wss://{}/ws/data?session_id={}&role=sender&device_id={}&target_device_id={}&token={}",
                base, session_id, from_device, to_device, token
            )
        };

        log::info!("Sender connecting to data plane: {}", ws_data_url);

        let (ws_stream, _) = match connect_async(&ws_data_url).await {
            Ok(s) => s,
            Err(e) => {
                log::error!("Sender failed to connect to /ws/data: {}", e);
                return;
            }
        };

        let (mut write, mut read) = ws_stream.split();
        let session_uuid = Uuid::parse_str(&session_id).unwrap_or_else(|_| Uuid::new_v4());

        // Flatten all chunks across all items
        struct ChunkDescriptor {
            item_index: u32,
            chunk_index: u32,
            total_chunks: u32,
            offset: u64,
            length: usize,
            file_path: PathBuf,
        }

        let mut all_chunks = Vec::new();
        for (item_idx, item) in offer.items.iter().enumerate() {
            if let Some(path) = file_paths.get(item_idx) {
                let file_size = item.size as u64;
                for c in 0..item.total_chunks {
                    let offset = (c as u64) * (MAX_PAYLOAD_LENGTH as u64);
                    let length = if offset + (MAX_PAYLOAD_LENGTH as u64) > file_size {
                        (file_size.saturating_sub(offset)) as usize
                    } else {
                        MAX_PAYLOAD_LENGTH as usize
                    };
                    all_chunks.push(ChunkDescriptor {
                        item_index: item.item_index,
                        chunk_index: c,
                        total_chunks: item.total_chunks,
                        offset,
                        length,
                        file_path: path.clone(),
                    });
                }
            }
        }

        let total_chunks_count = all_chunks.len() as u32;
        let mut window = SlidingWindow::new(total_chunks_count, 4);
        let mut next_chunk_to_send: u32 = 0;
        let mut transferred_bytes: i64 = 0;
        let total_size = offer.total_size;

        // Emit initial progress
        let _ = app_handle.emit("transfer-progress", ActiveTransfer {
            session_id: session_id.clone(),
            preview_summary: offer.preview_summary.clone(),
            total_size,
            transferred_size: 0,
            direction: "SEND".to_string(),
            progress: 0.0,
            status: "TRANSFERRING".to_string(),
        });

        let mut check_interval = tokio::time::interval(Duration::from_millis(200));

        while !window.is_complete() {
            // Check retry limit (N5)
            if window.has_exceeded_max_retries() {
                log::error!("Max retries exceeded for session {}, aborting transfer", session_id);
                break;
            }

            // 1. Send chunks allowed by window
            while window.can_send(next_chunk_to_send) {
                let desc = &all_chunks[next_chunk_to_send as usize];
                // Read payload chunk from disk
                let payload = match read_file_chunk(&desc.file_path, desc.offset, desc.length) {
                    Ok(data) => data,
                    Err(e) => {
                        log::error!("Failed to read chunk from {:?}: {}", desc.file_path, e);
                        break;
                    }
                };

                // Build BinaryHeader
                let header = BinaryHeader::new_data(
                    session_uuid,
                    desc.item_index,
                    desc.chunk_index,
                    desc.total_chunks,
                    &payload,
                );

                let mut frame = vec![0u8; HEADER_SIZE + payload.len()];
                if header.encode(&mut frame[..HEADER_SIZE]).is_ok() {
                    frame[HEADER_SIZE..].copy_from_slice(&payload);
                    if write.send(Message::Binary(frame)).await.is_ok() {
                        window.on_chunk_sent(next_chunk_to_send);
                        next_chunk_to_send += 1;
                    } else {
                        log::error!("Sender write failed");
                        break;
                    }
                }
            }

            // 2. Select between receiving ACKs/NACKs or timeout checks
            tokio::select! {
                ack_msg = read.next() => {
                    match ack_msg {
                        Some(Ok(Message::Binary(ack_bytes))) => {
                            if ack_bytes.len() >= HEADER_SIZE {
                                if let Ok(ack_hdr) = BinaryHeader::decode(&ack_bytes) {
                                    if ack_hdr.chunk_type == ChunkType::Ack {
                                        // Find corresponding global chunk
                                        if let Some(pos) = all_chunks.iter().position(|c| c.item_index == ack_hdr.item_index && c.chunk_index == ack_hdr.chunk_index) {
                                            window.on_ack(pos as u32);
                                            transferred_bytes += all_chunks[pos].length as i64;
                                            let pct = if total_size > 0 {
                                                ((transferred_bytes as f64) / (total_size as f64) * 100.0).clamp(0.0, 100.0)
                                            } else {
                                                100.0
                                            };
                                            let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                                                session_id: session_id.clone(),
                                                preview_summary: offer.preview_summary.clone(),
                                                total_size,
                                                transferred_size: transferred_bytes,
                                                direction: "SEND".to_string(),
                                                progress: pct,
                                                status: if window.is_complete() { "COMPLETED".to_string() } else { "TRANSFERRING".to_string() },
                                            });
                                        }
                                    } else if ack_hdr.chunk_type == ChunkType::Nack {
                                        // R3 / N1: Fast retransmit on NACK
                                        if let Some(pos) = all_chunks.iter().position(|c| c.item_index == ack_hdr.item_index && c.chunk_index == ack_hdr.chunk_index) {
                                            if let Some(retransmit_idx) = window.on_nack(pos as u32) {
                                                log::warn!("Received NACK for chunk {}, immediately fast-retransmitting", retransmit_idx);
                                                let desc = &all_chunks[retransmit_idx as usize];
                                                if let Ok(payload) = read_file_chunk(&desc.file_path, desc.offset, desc.length) {
                                                    let header = BinaryHeader::new_data(
                                                        session_uuid,
                                                        desc.item_index,
                                                        desc.chunk_index,
                                                        desc.total_chunks,
                                                        &payload,
                                                    );
                                                    let mut frame = vec![0u8; HEADER_SIZE + payload.len()];
                                                    if header.encode(&mut frame[..HEADER_SIZE]).is_ok() {
                                                        frame[HEADER_SIZE..].copy_from_slice(&payload);
                                                        let _ = write.send(Message::Binary(frame)).await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => break, // Connection closed or failed
                    }
                }

                _ = check_interval.tick() => {
                    let timeouts = window.check_timeouts(Instant::now());
                    for timed_out_idx in timeouts {
                        let desc = &all_chunks[timed_out_idx as usize];
                        if let Ok(payload) = read_file_chunk(&desc.file_path, desc.offset, desc.length) {
                            let header = BinaryHeader::new_data(
                                session_uuid,
                                desc.item_index,
                                desc.chunk_index,
                                desc.total_chunks,
                                &payload,
                            );
                            let mut frame = vec![0u8; HEADER_SIZE + payload.len()];
                            if header.encode(&mut frame[..HEADER_SIZE]).is_ok() {
                                frame[HEADER_SIZE..].copy_from_slice(&payload);
                                let _ = write.send(Message::Binary(frame)).await;
                            }
                        }
                    }
                }
            }
        }

        if window.is_complete() {
            log::info!("All chunks acknowledged! Sending TRANSFER_COMPLETE for session {}", session_id);
            let complete_env = ControlEnvelope {
                version: 1,
                trace_id: Uuid::new_v4().to_string(),
                action: ActionType::TRANSFER_COMPLETE,
                from_device: from_device.clone(),
                to_device: Some(to_device.clone()),
                timestamp: crate::core::connection_actor::current_time_ms(),
                payload: serde_json::json!({ "session_id": session_id }),
            };
            let _ = outgoing_tx.send(complete_env).await;

            let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                session_id: session_id.clone(),
                preview_summary: offer.preview_summary.clone(),
                total_size,
                transferred_size: total_size,
                direction: "SEND".to_string(),
                progress: 100.0,
                status: "COMPLETED".to_string(),
            });
        } else {
            // N4: Emit FAILED status and send TRANSFER_FAILURE on interrupted/aborted transfer
            log::error!("Sender transfer failed or was interrupted for session {}", session_id);
            let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                session_id: session_id.clone(),
                preview_summary: offer.preview_summary.clone(),
                total_size,
                transferred_size: transferred_bytes,
                direction: "SEND".to_string(),
                progress: if total_size > 0 { ((transferred_bytes as f64) / (total_size as f64) * 100.0).clamp(0.0, 100.0) } else { 0.0 },
                status: "FAILED".to_string(),
            });

            let fail_env = ControlEnvelope {
                version: 1,
                trace_id: Uuid::new_v4().to_string(),
                action: ActionType::TRANSFER_FAILURE,
                from_device: from_device.clone(),
                to_device: Some(to_device.clone()),
                timestamp: crate::core::connection_actor::current_time_ms(),
                payload: serde_json::json!({
                    "session_id": session_id,
                    "error_code": "TRANSFER_ABORTED",
                    "message": "Sender transfer connection terminated before completion or exceeded retries",
                }),
            };
            let _ = outgoing_tx.send(fail_env).await;
        }
    }

    /// Connects to /ws/data as Receiver, writes chunks into sandbox, verifies SHA256 and sends ACKs (P0-3, P1-6).
    pub async fn start_receiver_task(
        server_url: String,
        session_id: String,
        token: String,
        from_device: String,
        to_device: String,
        offer: TransferOfferPayload,
        cache_manager: CacheManager,
        auto_inject: bool,
        outgoing_tx: mpsc::Sender<ControlEnvelope>,
        app_handle: AppHandle,
    ) {
        let base = server_url.trim().trim_end_matches('/');
        let ws_data_url = if base.starts_with("ws://") || base.starts_with("wss://") {
            format!(
                "{}/ws/data?session_id={}&role=receiver&device_id={}&target_device_id={}&token={}",
                base, session_id, to_device, from_device, token
            )
        } else {
            format!(
                "wss://{}/ws/data?session_id={}&role=receiver&device_id={}&target_device_id={}&token={}",
                base, session_id, to_device, from_device, token
            )
        };

        log::info!("Receiver connecting to data plane: {}", ws_data_url);

        let (ws_stream, _) = match connect_async(&ws_data_url).await {
            Ok(s) => s,
            Err(e) => {
                log::error!("Receiver failed to connect to /ws/data: {}", e);
                return;
            }
        };

        let (mut write, mut read) = ws_stream.split();
        let session_root = cache_manager.cache_root().join(&session_id);
        let _ = fs::create_dir_all(&session_root);

        let mut received_chunks_per_item: Vec<HashSet<u32>> = vec![HashSet::new(); offer.items.len()];
        let mut total_received_bytes: i64 = 0;
        let total_size = offer.total_size;

        // Emit initial receive progress
        let _ = app_handle.emit("transfer-progress", ActiveTransfer {
            session_id: session_id.clone(),
            preview_summary: offer.preview_summary.clone(),
            total_size,
            transferred_size: 0,
            direction: "RECEIVE".to_string(),
            progress: 0.0,
            status: "TRANSFERRING".to_string(),
        });

        let mut fully_completed = false;

        while let Some(msg) = read.next().await {
            let chunk_data = match msg {
                Ok(Message::Binary(bytes)) => bytes,
                _ => break,
            };

            if chunk_data.len() < HEADER_SIZE {
                continue;
            }

            let header = match BinaryHeader::decode(&chunk_data[..HEADER_SIZE]) {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("Invalid binary header from sender: {}", e);
                    break;
                }
            };

            let payload = &chunk_data[HEADER_SIZE..];
            if header.verify_crc32(payload).is_err() {
                log::warn!("CRC32 corruption on chunk {}, requesting retransmit", header.chunk_index);
                // Send NACK
                let nack = BinaryHeader::new_nack(header.session_id, header.item_index, header.chunk_index);
                let mut nack_frame = [0u8; HEADER_SIZE];
                if nack.encode(&mut nack_frame).is_ok() {
                    let _ = write.send(Message::Binary(nack_frame.to_vec())).await;
                }
                continue;
            }

            let item_idx = header.item_index as usize;
            if item_idx >= offer.items.len() {
                continue;
            }
            let item = &offer.items[item_idx];

            // 1. PathGuard: resolve target safely within session root (P1-6, P2-3)
            let safe_target = match PathGuard::sanitize_and_resolve(&session_root, &item.relative_path) {
                Ok(p) => p,
                Err(e) => {
                    log::error!("PathGuard traversal rejected: {}", e);
                    break;
                }
            };

            if let Some(parent) = safe_target.parent() {
                let _ = fs::create_dir_all(parent);
            }

            // 2. Write chunk into file (P1-6: truncate on first chunk)
            let is_first = header.chunk_index == 0 && !safe_target.exists();
            let write_res = write_payload_chunk(&safe_target, header.chunk_index, payload, is_first);
            if let Err(e) = write_res {
                log::error!("Failed to write chunk to {:?}: {}", safe_target, e);
                break;
            }

            // 3. Send ACK frame back
            let ack = BinaryHeader::new_ack(header.session_id, header.item_index, header.chunk_index);
            let mut ack_frame = [0u8; HEADER_SIZE];
            if ack.encode(&mut ack_frame).is_ok() {
                let _ = write.send(Message::Binary(ack_frame.to_vec())).await;
            }

            // 4. Track completion via bitmap set
            let was_new = received_chunks_per_item[item_idx].insert(header.chunk_index);
            if was_new {
                total_received_bytes += payload.len() as i64;
                let pct = if total_size > 0 {
                    ((total_received_bytes as f64) / (total_size as f64) * 100.0).clamp(0.0, 100.0)
                } else {
                    100.0
                };
                let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                    session_id: session_id.clone(),
                    preview_summary: offer.preview_summary.clone(),
                    total_size,
                    transferred_size: total_received_bytes,
                    direction: "RECEIVE".to_string(),
                    progress: pct,
                    status: "TRANSFERRING".to_string(),
                });
            }

            // Check if all items are fully received
            let all_items_done = offer.items.iter().enumerate().all(|(idx, it)| {
                received_chunks_per_item[idx].len() == (it.total_chunks as usize)
            });

            if all_items_done {
                log::info!("All files downloaded for session {}, verifying SHA256 (P1-6)...", session_id);

                let mut all_valid = true;
                let mut completed_paths = Vec::new();

                for item in &offer.items {
                    let path = session_root.join(&item.relative_path);
                    let expected_sha = item.sha256.clone();
                    let check_res = tokio::task::spawn_blocking(move || {
                        verify_file_sha256(&path, &expected_sha)
                    }).await.unwrap_or(Ok(false));

                    match check_res {
                        Ok(true) => {
                            completed_paths.push(session_root.join(&item.relative_path));
                            let _ = cache_manager.register_entry(&session_root.join(&item.relative_path), &session_id, item.size).await;
                        }
                        _ => {
                            all_valid = false;
                            log::error!("SHA256 mismatch on file: {}", item.relative_path);
                            break;
                        }
                    }
                }

                if all_valid {
                    fully_completed = true;
                    log::info!("All files successfully verified with SHA-256 for session {}", session_id);
                    let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                        session_id: session_id.clone(),
                        preview_summary: offer.preview_summary.clone(),
                        total_size,
                        transferred_size: total_size,
                        direction: "RECEIVE".to_string(),
                        progress: 100.0,
                        status: "COMPLETED".to_string(),
                    });

                    // P1-9 / R4: Show transfer notification reflecting injection mode
                    let notification_body = if auto_inject {
                        format!("{} (已自动装载至剪贴板)", offer.preview_summary)
                    } else {
                        format!("{} (已保存在沙盒，可在面板中点击装载)", offer.preview_summary)
                    };
                    let _ = show_transfer_notification(&app_handle, "UniDrop 文件接收完成", &notification_body);

                    // P1-9: Auto inject into clipboard if configured
                    if auto_inject && !completed_paths.is_empty() {
                        let paths_clone = completed_paths.clone();
                        tokio::task::spawn_blocking(move || {
                            crate::platform::inject_files_to_clipboard(&paths_clone)
                        }).await.ok();
                        // M2: Mark 2h immunity lock for auto-injected files
                        let _ = cache_manager.mark_clipboard_injected(&session_id).await;
                    }
                } else {
                    let fail_env = ControlEnvelope {
                        version: 1,
                        trace_id: Uuid::new_v4().to_string(),
                        action: ActionType::TRANSFER_FAILURE,
                        from_device: to_device.clone(),
                        to_device: Some(from_device.clone()),
                        timestamp: crate::core::connection_actor::current_time_ms(),
                        payload: serde_json::json!({
                            "session_id": session_id,
                            "error_code": "SHA256_MISMATCH",
                            "error_message": "Downloaded file SHA256 checksum verification failed"
                        }),
                    };
                    let _ = outgoing_tx.send(fail_env).await;
                }
                break;
            }
        }

        // N4 / P3-8: If receiver exited without full verification, emit FAILED and remove partial sandbox
        if !fully_completed {
            log::error!("Receiver loop exited prematurely or validation failed for session {}", session_id);
            let _ = fs::remove_dir_all(&session_root);

            let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                session_id: session_id.clone(),
                preview_summary: offer.preview_summary.clone(),
                total_size,
                transferred_size: total_received_bytes,
                direction: "RECEIVE".to_string(),
                progress: if total_size > 0 { ((total_received_bytes as f64) / (total_size as f64) * 100.0).clamp(0.0, 100.0) } else { 0.0 },
                status: "FAILED".to_string(),
            });

            let fail_env = ControlEnvelope {
                version: 1,
                trace_id: Uuid::new_v4().to_string(),
                action: ActionType::TRANSFER_FAILURE,
                from_device: to_device.clone(),
                to_device: Some(from_device.clone()),
                timestamp: crate::core::connection_actor::current_time_ms(),
                payload: serde_json::json!({
                    "session_id": session_id,
                    "error_code": "RECEIVER_DISCONNECTED",
                    "error_message": "Receiver connection dropped or checksum verification failed before completion"
                }),
            };
            let _ = outgoing_tx.send(fail_env).await;
        }
    }
}

fn read_file_chunk(path: &Path, offset: u64, length: usize) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; length];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_payload_chunk(path: &Path, chunk_index: u32, payload: &[u8], truncate: bool) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(truncate)
        .open(path)?;

    let offset = (chunk_index as u64) * (MAX_PAYLOAD_LENGTH as u64);
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(payload)?;
    Ok(())
}

fn verify_file_sha256(path: &Path, expected_hex: &str) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual_hex = hex::encode(hasher.finalize());
    Ok(actual_hex.eq_ignore_ascii_case(expected_hex))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_prepare_offer_aligns_valid_paths_and_skips_dirs() {
        let temp_dir = std::env::temp_dir().join(format!("unidrop_test_{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        let sub_dir = temp_dir.join("sub_folder");
        fs::create_dir_all(&sub_dir).unwrap();

        let file1 = temp_dir.join("file1.txt");
        let mut f1 = File::create(&file1).unwrap();
        f1.write_all(b"Hello World 1").unwrap();

        let file2 = temp_dir.join("file2.txt");
        let mut f2 = File::create(&file2).unwrap();
        f2.write_all(b"Hello World 2").unwrap();

        // Input paths has directory first, then file1, non-existent, then file2
        let non_existent = temp_dir.join("does_not_exist.bin");
        let input_paths = vec![sub_dir, file1.clone(), non_existent, file2.clone()];

        let session_id = Uuid::new_v4();
        let (offer, valid_paths) = TransferEngine::prepare_offer(session_id, &input_paths).unwrap();

        assert_eq!(offer.items.len(), 2);
        assert_eq!(valid_paths.len(), 2);

        // Verify 1:1 match
        assert_eq!(valid_paths[0], file1);
        assert_eq!(offer.items[0].relative_path, "file1.txt");

        assert_eq!(valid_paths[1], file2);
        assert_eq!(offer.items[1].relative_path, "file2.txt");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
