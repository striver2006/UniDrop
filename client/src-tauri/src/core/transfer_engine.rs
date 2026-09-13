use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async_tls_with_config;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::core::cache_manager::CacheManager;
use crate::core::connection_actor::create_tls_connector;
use crate::core::path_guard::PathGuard;
use crate::core::sliding_window::SlidingWindow;
use crate::platform::show_transfer_notification;
use crate::protocol::{
    ActionType, BinaryHeader, ChunkType, ControlEnvelope, HEADER_SIZE, MAX_PAYLOAD_LENGTH,
    TransferItemPayload, TransferOfferPayload,
};
use crate::storage::HistoryRepo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTransfer {
    pub session_id: String,
    pub preview_summary: String,
    pub total_size: i64,
    pub transferred_size: i64,
    pub direction: String, // "SEND" | "RECEIVE"
    pub progress: f64,     // 0..100
    pub status: String,    // "TRANSFERRING" | "COMPLETED" | "FAILED"
    #[serde(default)]
    pub data_type: String, // "FILES" | "TEXT" | "IMAGE"
}

/// Where the sender reads chunk payloads from: real files on disk or an
/// in-memory buffer (clipboard text / image).
#[derive(Debug, Clone)]
pub enum TransferSource {
    Files(Vec<PathBuf>),
    Memory(Vec<u8>),
}

/// Best-effort history status update; failures are logged only.
///
/// 注意 `conn` guard 的作用域**就是本函数体**。任何需要再次取 `db_conn` 锁的动作
/// （例如修剪历史）都不能写进这里——`tokio::sync::Mutex` 不可重入，会永久死锁。
/// 终态请改用 `finalize_history_status`。
async fn update_history_status(app_handle: &AppHandle, session_id: &str, status: &str, error: Option<&str>) {
    let state = app_handle.state::<AppState>();
    let conn = state.db_conn.lock().await;
    if let Err(e) = HistoryRepo::update_task_status(&conn, session_id, status, error) {
        log::warn!("Failed to update history status for {}: {}", session_id, e);
    }
}

/// 写入**终态**并随即按保留上限修剪历史（需求 4）。
///
/// 单独封一层，是为了把「修剪必须发生在锁释放之后」这条约束固定在一个地方：
/// 上面那次 `.await` 返回时 `db_conn` 锁已经释放，这里再取锁才是安全的。
/// 六个终态调用点若各自手写两行，早晚有人把修剪塞进 `update_history_status`
/// 内部——那是静默死锁，整个应用卡住且不报任何错。
async fn finalize_history_status(
    app_handle: &AppHandle,
    session_id: &str,
    status: &str,
    error: Option<&str>,
) {
    update_history_status(app_handle, session_id, status, error).await;
    crate::core::history_pruner::prune_and_notify(app_handle).await;
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

    /// Prepares an offer for in-memory content (clipboard text / image) without touching disk.
    pub fn prepare_offer_from_bytes(
        session_id: Uuid,
        data_type: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<(TransferOfferPayload, TransferSource), String> {
        if bytes.is_empty() {
            return Err("Empty payload".into());
        }

        let size = bytes.len() as i64;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let hash = hex::encode(hasher.finalize());

        let total_chunks = ((size + (MAX_PAYLOAD_LENGTH as i64) - 1) / (MAX_PAYLOAD_LENGTH as i64)) as u32;

        let preview_summary = match data_type {
            "TEXT" => {
                let text = String::from_utf8_lossy(&bytes);
                let truncated: String = text.chars().take(50).collect();
                if text.chars().count() > 50 {
                    format!("文本: {}…", truncated)
                } else {
                    format!("文本: {}", truncated)
                }
            }
            "IMAGE" => {
                let kb = size as f64 / 1024.0;
                if kb >= 1024.0 {
                    format!("图片 ({:.1} MB)", kb / 1024.0)
                } else {
                    format!("图片 ({:.0} KB)", kb)
                }
            }
            _ => name.to_string(),
        };

        let offer = TransferOfferPayload {
            session_id: session_id.to_string(),
            data_type: data_type.to_string(),
            total_size: size,
            total_items: 1,
            preview_summary,
            encrypted: false,
            encrypted_metadata: None,
            items: vec![TransferItemPayload {
                item_index: 0,
                relative_path: name.to_string(),
                size,
                is_dir: false,
                sha256: hash,
                total_chunks,
            }],
        };

        Ok((offer, TransferSource::Memory(bytes)))
    }

    /// Connects to /ws/data as Sender and executes Sliding Window ARQ transfer (P0-3).
    pub async fn start_sender_task(
        server_url: String,
        // 与 server_url 同路取自设置：数据面必须与控制面用同一套 TLS 策略，
        // 否则用户在界面上关掉的校验会在这条连接上悄悄恢复（反之亦然）。
        allow_insecure_tls: bool,
        session_id: String,
        token: String,
        from_device: String,
        to_device: String,
        source: TransferSource,
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

        // 与控制面同一套 TLS 策略：默认校验证书，勾选后才跳过。
        //
        // 注意一处有意接受的取舍：这里的证书失败**不做**专门提示，只会以
        // 「数据通道连接失败」呈现。控制面连不上时数据面根本不会启动，
        // 所以这条路径实际不可达，不值得再铺一条通知链路。
        let (ws_stream, _) = match connect_async_tls_with_config(
            &ws_data_url,
            None,
            false,
            create_tls_connector(allow_insecure_tls),
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                log::error!("Sender failed to connect to /ws/data: {}", e);
                let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                    session_id: session_id.clone(),
                    preview_summary: offer.preview_summary.clone(),
                    total_size: offer.total_size,
                    transferred_size: 0,
                    direction: "SEND".to_string(),
                    progress: 0.0,
                    status: "FAILED".to_string(),
                    data_type: offer.data_type.clone(),
                });
                finalize_history_status(&app_handle, &session_id, "FAILED", Some(&format!("数据通道连接失败: {}", e))).await;
                let fail_env = ControlEnvelope {
                    version: 1,
                    trace_id: Uuid::new_v4().to_string(),
                    action: ActionType::TRANSFER_FAILURE,
                    from_device: from_device.clone(),
                    to_device: Some(to_device.clone()),
                    timestamp: crate::core::connection_actor::current_time_ms(),
                    payload: serde_json::json!({
                        "session_id": session_id,
                        "error_code": "DATA_CONNECT_FAILED",
                        "error_message": format!("Sender failed to connect to data plane: {}", e)
                    }),
                };
                let _ = outgoing_tx.send(fail_env).await;
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
            source: ChunkOrigin,
        }

        enum ChunkOrigin {
            File(PathBuf),
            Memory(Arc<Vec<u8>>),
        }

        fn read_chunk_payload(desc: &ChunkDescriptor) -> std::io::Result<Vec<u8>> {
            match &desc.source {
                ChunkOrigin::File(path) => read_file_chunk(path, desc.offset, desc.length),
                ChunkOrigin::Memory(buf) => {
                    let start = desc.offset as usize;
                    Ok(buf[start..start + desc.length].to_vec())
                }
            }
        }

        let (mem_buf, file_paths) = match source {
            TransferSource::Memory(bytes) => (Some(Arc::new(bytes)), None),
            TransferSource::Files(paths) => (None, Some(paths)),
        };

        let mut all_chunks = Vec::new();
        for (item_idx, item) in offer.items.iter().enumerate() {
            let file_size = item.size as u64;
            for c in 0..item.total_chunks {
                let offset = (c as u64) * (MAX_PAYLOAD_LENGTH as u64);
                let length = if offset + (MAX_PAYLOAD_LENGTH as u64) > file_size {
                    (file_size.saturating_sub(offset)) as usize
                } else {
                    MAX_PAYLOAD_LENGTH as usize
                };
                let origin = if let Some(mem) = &mem_buf {
                    ChunkOrigin::Memory(mem.clone())
                } else if let Some(paths) = &file_paths {
                    match paths.get(item_idx) {
                        Some(p) => ChunkOrigin::File(p.clone()),
                        None => continue,
                    }
                } else {
                    continue;
                };
                all_chunks.push(ChunkDescriptor {
                    item_index: item.item_index,
                    chunk_index: c,
                    total_chunks: item.total_chunks,
                    offset,
                    length,
                    source: origin,
                });
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
            data_type: offer.data_type.clone(),
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
                // Read payload chunk from disk or memory
                let payload = match read_chunk_payload(desc) {
                    Ok(data) => data,
                    Err(e) => {
                        log::error!("Failed to read chunk for session {}: {}", session_id, e);
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
                                                data_type: offer.data_type.clone(),
                                            });
                                        }
                                    } else if ack_hdr.chunk_type == ChunkType::Nack {
                                        // R3 / N1: Fast retransmit on NACK
                                        if let Some(pos) = all_chunks.iter().position(|c| c.item_index == ack_hdr.item_index && c.chunk_index == ack_hdr.chunk_index) {
                                        if let Some(retransmit_idx) = window.on_nack(pos as u32) {
                                            log::warn!("Received NACK for chunk {}, immediately fast-retransmitting", retransmit_idx);
                                            let desc = &all_chunks[retransmit_idx as usize];
                                            if let Ok(payload) = read_chunk_payload(desc) {
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
                        if let Ok(payload) = read_chunk_payload(desc) {
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
                data_type: offer.data_type.clone(),
            });
            finalize_history_status(&app_handle, &session_id, "COMPLETED", None).await;
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
                data_type: offer.data_type.clone(),
            });
            finalize_history_status(&app_handle, &session_id, "FAILED", Some("传输中断或超出重试上限")).await;

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
                    "error_message": "Sender transfer connection terminated before completion or exceeded retries"
                }),
            };
            let _ = outgoing_tx.send(fail_env).await;
        }
    }

    /// Connects to /ws/data as Receiver, writes chunks into sandbox, verifies SHA256 and sends ACKs (P0-3, P1-6).
    pub async fn start_receiver_task(
        server_url: String,
        allow_insecure_tls: bool,
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

        // 与控制面同一套 TLS 策略：默认校验证书，勾选后才跳过。
        //
        // 注意一处有意接受的取舍：这里的证书失败**不做**专门提示，只会以
        // 「数据通道连接失败」呈现。控制面连不上时数据面根本不会启动，
        // 所以这条路径实际不可达，不值得再铺一条通知链路。
        let (ws_stream, _) = match connect_async_tls_with_config(
            &ws_data_url,
            None,
            false,
            create_tls_connector(allow_insecure_tls),
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                log::error!("Receiver failed to connect to /ws/data: {}", e);
                let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                    session_id: session_id.clone(),
                    preview_summary: offer.preview_summary.clone(),
                    total_size: offer.total_size,
                    transferred_size: 0,
                    direction: "RECEIVE".to_string(),
                    progress: 0.0,
                    status: "FAILED".to_string(),
                    data_type: offer.data_type.clone(),
                });
                let _ = show_transfer_notification(&app_handle, "UniDrop 接收失败", "数据通道连接失败，请检查服务器地址与证书配置");
                finalize_history_status(&app_handle, &session_id, "FAILED", Some(&format!("数据通道连接失败: {}", e))).await;
                let fail_env = ControlEnvelope {
                    version: 1,
                    trace_id: Uuid::new_v4().to_string(),
                    action: ActionType::TRANSFER_FAILURE,
                    from_device: to_device.clone(),
                    to_device: Some(from_device.clone()),
                    timestamp: crate::core::connection_actor::current_time_ms(),
                    payload: serde_json::json!({
                        "session_id": session_id,
                        "error_code": "DATA_CONNECT_FAILED",
                        "error_message": format!("Receiver failed to connect to data plane: {}", e)
                    }),
                };
                let _ = outgoing_tx.send(fail_env).await;
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
            data_type: offer.data_type.clone(),
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
                    data_type: offer.data_type.clone(),
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
                        data_type: offer.data_type.clone(),
                    });

                    match offer.data_type.as_str() {
                        "TEXT" => {
                            // Clipboard text: write directly into the system clipboard
                            if let Some(path) = completed_paths.first() {
                                if let Ok(bytes) = fs::read(path) {
                                    let text = String::from_utf8_lossy(&bytes).to_string();
                                    let write_result = tokio::task::spawn_blocking(move || {
                                        crate::platform::write_text_to_clipboard(&text)
                                    })
                                    .await
                                    .unwrap_or_else(|_| Err("clipboard task panicked".to_string()));
                                    if let Err(e) = write_result {
                                        log::error!("Failed to write received text to clipboard: {}", e);
                                    }
                                }
                            }
                            let _ = show_transfer_notification(&app_handle, "UniDrop 文本已同步", "已写入系统剪贴板，可直接粘贴");
                            let _ = cache_manager.mark_clipboard_injected(&session_id).await;
                        }
                        "IMAGE" => {
                            // Clipboard image: write PNG directly into the system clipboard
                            if let Some(path) = completed_paths.first() {
                                if let Ok(bytes) = fs::read(path) {
                                    let write_result = tokio::task::spawn_blocking(move || {
                                        crate::platform::write_image_to_clipboard(&bytes)
                                    })
                                    .await
                                    .unwrap_or_else(|_| Err("clipboard task panicked".to_string()));
                                    if let Err(e) = write_result {
                                        log::error!("Failed to write received image to clipboard: {}", e);
                                    }
                                }
                            }
                            let _ = show_transfer_notification(&app_handle, "UniDrop 图片已同步", "已写入系统剪贴板，可直接粘贴");
                            let _ = cache_manager.mark_clipboard_injected(&session_id).await;
                        }
                        _ => {
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
                                })
                                .await
                                .ok();
                                // M2: Mark 2h immunity lock for auto-injected files
                                let _ = cache_manager.mark_clipboard_injected(&session_id).await;
                            }
                        }
                    }
                    finalize_history_status(&app_handle, &session_id, "COMPLETED", None).await;
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
                data_type: offer.data_type.clone(),
            });
            finalize_history_status(&app_handle, &session_id, "FAILED", Some("接收连接中断或校验失败")).await;

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

    #[test]
    fn test_prepare_offer_from_bytes_text_and_chunking() {
        let session_id = Uuid::new_v4();

        // Small text: single chunk, TEXT summary present
        let (offer, source) =
            TransferEngine::prepare_offer_from_bytes(session_id, "TEXT", "clipboard.txt", b"hello clipboard".to_vec())
                .unwrap();
        assert_eq!(offer.data_type, "TEXT");
        assert_eq!(offer.total_items, 1);
        assert_eq!(offer.total_size, b"hello clipboard".len() as i64);
        assert_eq!(offer.items[0].total_chunks, 1);
        assert!(offer.preview_summary.contains("文本"));
        assert!(matches!(source, TransferSource::Memory(_)));

        // Expected SHA-256 of the payload
        let mut hasher = Sha256::new();
        hasher.update(b"hello clipboard");
        assert_eq!(offer.items[0].sha256, hex::encode(hasher.finalize()));

        // Payload larger than one chunk (>4MB) splits into the right number of chunks
        let big = vec![7u8; (MAX_PAYLOAD_LENGTH as usize) * 2 + 1024];
        let (offer_big, _) =
            TransferEngine::prepare_offer_from_bytes(session_id, "IMAGE", "clipboard.png", big).unwrap();
        assert_eq!(offer_big.items[0].total_chunks, 3);
        assert!(offer_big.preview_summary.contains("图片"));

        // Empty payload is rejected
        assert!(TransferEngine::prepare_offer_from_bytes(session_id, "TEXT", "clipboard.txt", Vec::new()).is_err());
    }
}
