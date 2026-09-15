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
use crate::core::tls_trust::create_tls_connector;
use crate::core::path_guard::PathGuard;
use crate::core::sliding_window::SlidingWindow;
use crate::platform::show_transfer_notification;
use crate::protocol::{
    ActionType, BinaryHeader, ChunkType, ControlEnvelope, FLAG_ENCRYPTED, HEADER_SIZE,
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

/// 把 URL 查询串里的 `token=` 值抹掉，供日志输出使用。
///
/// 数据面 URL 把一次性会话令牌拼在查询串里，而这两条连接日志是 `info` 级别。
/// 在日志只走 stderr、且默认级别是 `Error` 的年代这不构成问题——什么都没输出。
/// 接入落盘日志后它就变成了「每传一次文件，令牌写一次磁盘」，
/// 所以脱敏必须与日志改造同批次落地，不能分开做。
///
/// 只处理 `token`：同在查询串里的 session_id / device_id 都不是凭据，
/// 而它们恰恰是排查时最需要看到的东西。
fn redact_query_token(url: &str) -> String {
    let Some(start) = url.find("token=") else {
        return url.to_string();
    };
    let value_start = start + "token=".len();
    // 令牌是查询串末位参数，但不能假定它一直是——否则哪天换了顺序，
    // 脱敏会静默地把后面所有参数一起吃掉。
    let value_end = url[value_start..]
        .find('&')
        .map(|i| value_start + i)
        .unwrap_or(url.len());
    format!("{}<redacted>{}", &url[..value_start], &url[value_end..])
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

/// 把对端 device_id 解析成设备名（在线设备表的 hostname），供通知标题使用。
///
/// 查不到时返回 None：发送方恰好离线、设备列表尚未同步都会走到这里，
/// 调用方退回无设备名的通用标题——辅助信息缺失不该连累整条通知不发。
async fn resolve_peer_name(app_handle: &AppHandle, device_id: &str) -> Option<String> {
    let state = app_handle.state::<AppState>();
    let devs = state.online_devices.lock().await;
    devs.iter()
        .find(|d| d.device_id == device_id)
        .map(|d| d.hostname.clone())
}

/// PRD §4.1.4 样例「48.5 MB」：一位小数，逐级向下换算。
fn human_size(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * KB;
    const GB: f64 = 1024.0 * MB;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

pub struct TransferEngine {
    pub cache_manager: CacheManager,
}

impl TransferEngine {
    pub fn new(cache_manager: CacheManager) -> Self {
        Self { cache_manager }
    }

    /// Prepares a TransferOfferPayload and matching valid file paths (R2 / N2: strictly 1:1 index-aligned).
    /// 构造文件传输的 OFFER。
    ///
    /// `e2ee_key` 为 `Some` 时本次传输加密：分块按明文块长切（给 GCM tag 留出
    /// 16 字节，否则满块密文会超 4MB 上限被中继断连），且敏感元数据会被摘出加密。
    /// 传 `Option<&LessSafeKey>` 而不是 `bool`，是为了让「声称加密却没有密钥」
    /// 这个状态在类型上就不可表示。
    pub fn prepare_offer(
        session_id: Uuid,
        file_paths: &[PathBuf],
        e2ee_key: Option<&ring::aead::LessSafeKey>,
    ) -> Result<(TransferOfferPayload, Vec<PathBuf>), String> {
        // 四处同源之一：块长只从 plaintext_chunk_len 取，不得就地写 MAX_PAYLOAD_LENGTH。
        let chunk_len = crate::core::e2ee::plaintext_chunk_len(e2ee_key.is_some()) as i64;
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
                ((file_size + chunk_len - 1) / chunk_len) as u32
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

        let mut offer = TransferOfferPayload {
            session_id: session_id.to_string(),
            data_type: "FILES".to_string(),
            total_size,
            total_items: items.len(),
            preview_summary: summary,
            encrypted: false,
            encrypted_metadata: None,
            e2ee_version: None,
            items,
        };
        // 只打标记，**不在这里密封**：密封会抹空文件名与摘要，而这份 offer 还要
        // 进本地的 pending_outbound 与历史库。密封在 dispatch_offer 里对副本做。
        if e2ee_key.is_some() {
            offer.encrypted = true;
            offer.e2ee_version = Some(crate::core::e2ee::E2EE_VERSION);
        }

        Ok((offer, valid_paths))
    }

    /// Prepares an offer for in-memory content (clipboard text / image) without touching disk.
    pub fn prepare_offer_from_bytes(
        session_id: Uuid,
        data_type: &str,
        name: &str,
        bytes: Vec<u8>,
        e2ee_key: Option<&ring::aead::LessSafeKey>,
    ) -> Result<(TransferOfferPayload, TransferSource), String> {
        if bytes.is_empty() {
            return Err("Empty payload".into());
        }

        let size = bytes.len() as i64;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let hash = hex::encode(hasher.finalize());

        // 四处同源之一，理由见 prepare_offer。剪贴板内容同样按明文块长切。
        let chunk_len = crate::core::e2ee::plaintext_chunk_len(e2ee_key.is_some()) as i64;
        let total_chunks = ((size + chunk_len - 1) / chunk_len) as u32;

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

        let mut offer = TransferOfferPayload {
            session_id: session_id.to_string(),
            data_type: data_type.to_string(),
            total_size: size,
            total_items: 1,
            preview_summary,
            encrypted: false,
            encrypted_metadata: None,
            e2ee_version: None,
            items: vec![TransferItemPayload {
                item_index: 0,
                relative_path: name.to_string(),
                size,
                is_dir: false,
                sha256: hash,
                total_chunks,
            }],
        };
        // 只打标记，**不在这里密封**：密封会抹空文件名与摘要，而这份 offer 还要
        // 进本地的 pending_outbound 与历史库。密封在 dispatch_offer 里对副本做。
        if e2ee_key.is_some() {
            offer.encrypted = true;
            offer.e2ee_version = Some(crate::core::e2ee::E2EE_VERSION);
        }

        Ok((offer, TransferSource::Memory(bytes)))
    }

    /// Connects to /ws/data as Sender and executes Sliding Window ARQ transfer (P0-3).
    pub async fn start_sender_task(
        server_url: String,
        // 与 server_url 同路取自设置：数据面必须与控制面用同一套 TLS 策略，
        // 否则用户在界面上关掉的校验会在这条连接上悄悄恢复（反之亦然）。
        tls_trust: crate::core::tls_trust::TlsTrustConfig,
        session_id: String,
        token: String,
        from_device: String,
        to_device: String,
        source: TransferSource,
        offer: TransferOfferPayload,
        // 本次传输的会话密钥。None = 明文传输。
        // 由调用方派生并传入，而不是在这里重新派生：派生需要 PSK 与 account_id，
        // 把它们再传一遍只会多两个参数，且多一处「用错 session_id 派生」的机会。
        e2ee_key: Option<ring::aead::LessSafeKey>,
        outgoing_tx: mpsc::Sender<ControlEnvelope>,
        app_handle: AppHandle,
    ) {
        // 四处同源之一：与 prepare_offer 算 total_chunks 时用的块长必须一致。
        let chunk_len = crate::core::e2ee::plaintext_chunk_len(e2ee_key.is_some());
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

        log::info!("Sender connecting to data plane: {}", redact_query_token(&ws_data_url));

        // 与控制面同一套 TLS 策略：默认校验证书，勾选后才跳过。
        //
        // 注意一处有意接受的取舍：这里的证书失败**不做**专门提示，只会以
        // 「数据通道连接失败」呈现。控制面连不上时数据面根本不会启动，
        // 所以这条路径实际不可达，不值得再铺一条通知链路。
        let (ws_stream, _) = match connect_async_tls_with_config(
            &ws_data_url,
            None,
            false,
            create_tls_connector(&tls_trust, None),
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
                // 四处同源之一：切块必须与 total_chunks 用同一个块长，
                // 否则最后一块的长度会算错，或多切/少切一块。
                let offset = (c as u64) * (chunk_len as u64);
                let length = if offset + (chunk_len as u64) > file_size {
                    (file_size.saturating_sub(offset)) as usize
                } else {
                    chunk_len
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

                // 成帧（加密在这里发生，三条发送路径共用同一个函数）
                let frame = match build_data_frame(
                    session_uuid,
                    desc.item_index,
                    desc.chunk_index,
                    desc.total_chunks,
                    &payload,
                    e2ee_key.as_ref(),
                ) {
                    Ok(f) => f,
                    Err(e) => {
                        log::error!("成帧失败（session {}）: {}", session_id, e);
                        break;
                    }
                };
                {
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
                                                    if let Ok(frame) = build_data_frame(
                                                        session_uuid,
                                                        desc.item_index,
                                                        desc.chunk_index,
                                                        desc.total_chunks,
                                                        &payload,
                                                        e2ee_key.as_ref(),
                                                    ) {
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
                            if let Ok(frame) = build_data_frame(
                                session_uuid,
                                desc.item_index,
                                desc.chunk_index,
                                desc.total_chunks,
                                &payload,
                                e2ee_key.as_ref(),
                            ) {
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
        tls_trust: crate::core::tls_trust::TlsTrustConfig,
        session_id: String,
        token: String,
        from_device: String,
        to_device: String,
        offer: TransferOfferPayload,
        cache_manager: CacheManager,
        auto_inject: bool,
        // 会话密钥。None = 明文传输。必须与 offer.encrypted 一致：
        // 不一致时要么把密文当明文写盘（文件损坏），要么对明文做 AEAD 解密（全块失败）。
        e2ee_key: Option<ring::aead::LessSafeKey>,
        outgoing_tx: mpsc::Sender<ControlEnvelope>,
        app_handle: AppHandle,
    ) {
        // 四处同源之一：写盘定位必须与发送端切块用同一个块长。
        let chunk_len = crate::core::e2ee::plaintext_chunk_len(e2ee_key.is_some());

        // §D 双层熔断的计数器。
        //
        // 单块阈值处理「CRC32 漏检后仍 tag 失败」这种极罕见情形；
        // **会话级阈值才是主力**：滑动窗口有 4 个在途块，PSK 不一致时它们会
        // 同时 tag 失败、各自只计到 1，永远够不到单块阈值——只按单块计数的话
        // 限次在最常见的触发场景下完全失效，退化成无限重传。
        let mut tag_fail_total: u32 = 0;
        // 是否因 AEAD 熔断而主动中止。置位后收尾阶段不再补发泛化的失败信令——
        // 两条失败信令走同一个 session_id，后到的会覆盖先到的错误文案，
        // 于是**越精准的诊断越先被发出、也越先被覆盖**。
        let mut e2ee_aborted = false;
        let mut tag_fail_per_chunk: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();

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

        log::info!("Receiver connecting to data plane: {}", redact_query_token(&ws_data_url));

        // 与控制面同一套 TLS 策略：默认校验证书，勾选后才跳过。
        //
        // 注意一处有意接受的取舍：这里的证书失败**不做**专门提示，只会以
        // 「数据通道连接失败」呈现。控制面连不上时数据面根本不会启动，
        // 所以这条路径实际不可达，不值得再铺一条通知链路。
        let (ws_stream, _) = match connect_async_tls_with_config(
            &ws_data_url,
            None,
            false,
            create_tls_connector(&tls_trust, None),
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
                // Err 必须留痕：通知发不出去本身就是哑故障，而「收不到提醒」
                // 恰恰是这个功能要防的那件事。此前四处都是 let _ =，
                // 所以 macOS 上通知整整失效了都没有任何信号。
                let failure_title = match resolve_peer_name(&app_handle, &from_device).await {
                    Some(name) => format!("来自 {name} 的接收失败"),
                    None => "UniDrop 接收失败".to_string(),
                };
                if let Err(e) = show_transfer_notification(&app_handle, &failure_title, "数据通道连接失败，请检查服务器地址与证书配置") {
                    log::warn!("Failed to show notification: {}", e);
                }
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

            // 线路上的字节：加密时这里是密文（含 tag）。
            let wire_payload = &chunk_data[HEADER_SIZE..];

            // 第一道：CRC32。它算在**密文**上（见 build_data_frame），所以这一步
            // 不需要先解密，能在做任何 AES 运算之前就把损坏的块打回重传。
            if header.verify_crc32(wire_payload).is_err() {
                log::warn!("CRC32 corruption on chunk {}, requesting retransmit", header.chunk_index);
                // Send NACK
                let nack = BinaryHeader::new_nack(header.session_id, header.item_index, header.chunk_index);
                let mut nack_frame = [0u8; HEADER_SIZE];
                if nack.encode(&mut nack_frame).is_ok() {
                    let _ = write.send(Message::Binary(nack_frame.to_vec())).await;
                }
                continue;
            }

            // 第二道：AEAD。CRC32 过了而 tag 不过，实践中几乎只剩篡改或 PSK 不一致
            // （CRC32 碰撞在 2^-32 量级），所以这里不能像损坏那样无限重传。
            let decrypted;
            let payload: &[u8] = match e2ee_key.as_ref() {
                Some(key) => {
                    // nonce **自行重算**，不读 header.nonce：发送端填了它只为抓包可读。
                    // 信任帧头里的 nonce 会让攻击者靠改它制造解密失败——后果虽与直接
                    // 改密文相同，但自己算消除了一整类需要论证的问题，成本为零。
                    let nonce = crate::core::e2ee::data_nonce(header.item_index, header.chunk_index);
                    match crate::core::e2ee::open(key, nonce, wire_payload) {
                        Ok(pt) => {
                            decrypted = pt;
                            &decrypted
                        }
                        Err(e) => {
                            let should_abort = record_tag_failure(
                                &mut tag_fail_per_chunk,
                                &mut tag_fail_total,
                                header.item_index,
                                header.chunk_index,
                            );
                            log::warn!(
                                "AEAD 校验失败（item {} chunk {}，本会话累计 {} 次）: {}",
                                header.item_index, header.chunk_index, tag_fail_total, e
                            );

                            if should_abort {
                                log::error!(
                                    "session {} 触发 E2EE 熔断，中止接收", session_id
                                );
                                let fail_env = ControlEnvelope {
                                    version: 1,
                                    trace_id: Uuid::new_v4().to_string(),
                                    action: ActionType::TRANSFER_FAILURE,
                                    from_device: to_device.clone(),
                                    to_device: Some(from_device.clone()),
                                    timestamp: crate::core::connection_actor::current_time_ms(),
                                    payload: serde_json::json!({
                                        "session_id": session_id,
                                        "error_code": "E2EE_AUTH_FAILED",
                                        // 最常见的原因是两端 PSK 不一致，不是篡改——
                                        // 文案先指向它，否则用户会去排查网络。
                                        "error_message": "内容校验失败：两端密钥可能不一致，或数据在传输中被篡改"
                                    }),
                                };
                                let _ = outgoing_tx.send(fail_env).await;
                                e2ee_aborted = true;
                                break;
                            }

                            // 未触顶：当作可恢复的损坏，请求重传。
                            let nack = BinaryHeader::new_nack(
                                header.session_id, header.item_index, header.chunk_index,
                            );
                            let mut nack_frame = [0u8; HEADER_SIZE];
                            if nack.encode(&mut nack_frame).is_ok() {
                                let _ = write.send(Message::Binary(nack_frame.to_vec())).await;
                            }
                            continue;
                        }
                    }
                }
                None => wire_payload,
            };

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
            let write_res = write_payload_chunk(&safe_target, header.chunk_index, payload, is_first, chunk_len);
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
                // 这里必须累加 `payload`（解密后的明文）而不是 `wire_payload`（密文）：
                // total_size 是明文口径，密文每块多 16 字节，累加密文会让
                // transferred_size 虚高、进度提前触顶。上面的 clamp 会盖住百分比的
                // 越界，但不会修正 transferred_size 本身——所以守卫测试断言的是
                // transferred_size，不是百分比。
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

                    // 自动写剪贴板前核一次归属。
                    //
                    // 这是第四条会剪贴板的路径，而且是唯一一条**无法**靠
                    // ensure_session_owned 兜住的：它不是用户点出来的，而是接收
                    // 任务跑完自己触发的。接收任务在 spawn 时把 settings 快照了
                    // 下来，保存设置只重连控制面 actor，已在飞的任务会继续跑完——
                    // 于是用户若在传输途中切了账号，这次传输的内容会落进**新**账号
                    // 的剪贴板，而历史行归属的是旧账号。
                    //
                    // 场景窄（同一个人、恰在传输窗口内切账号），但代价只是一次
                    // 按主键的查询，而收益是「所有写剪贴板的路径都受账号约束」
                    // 这句话可以成立。不一致时只发通知、不写剪贴板。
                    let still_owned = {
                        let state = app_handle.state::<AppState>();
                        let current = { state.settings.lock().await.account_id.clone() };
                        let conn = state.db_conn.lock().await;
                        crate::storage::HistoryRepo::session_belongs_to(&conn, &session_id, &current)
                    };
                    if !still_owned {
                        log::warn!(
                            "Skipped auto clipboard injection for session {}: account changed mid-transfer",
                            session_id
                        );
                    }

                    match offer.data_type.as_str() {
                        "TEXT" if still_owned => {
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
                            // PRD §4.1.4：标题带发送方设备名，用户不点开就知道是谁发来的。
                            let text_title = match resolve_peer_name(&app_handle, &from_device).await {
                                Some(name) => format!("来自 {name} 的文本"),
                                None => "UniDrop 文本已同步".to_string(),
                            };
                            if let Err(e) = show_transfer_notification(&app_handle, &text_title, "已写入系统剪贴板，可直接粘贴") {
                                log::warn!("Failed to show notification: {}", e);
                            }
                            let _ = cache_manager.mark_clipboard_injected(&session_id).await;
                        }
                        "IMAGE" if still_owned => {
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
                            let image_title = match resolve_peer_name(&app_handle, &from_device).await {
                                Some(name) => format!("来自 {name} 的图片"),
                                None => "UniDrop 图片已同步".to_string(),
                            };
                            if let Err(e) = show_transfer_notification(&app_handle, &image_title, "已写入系统剪贴板，可直接粘贴") {
                                log::warn!("Failed to show notification: {}", e);
                            }
                            let _ = cache_manager.mark_clipboard_injected(&session_id).await;
                        }
                        _ => {
                            // P1-9 / R4: Show transfer notification reflecting injection mode
                            let notification_body = if auto_inject {
                                format!("{} (已自动装载至剪贴板)", offer.preview_summary)
                            } else {
                                format!("{} (已保存在沙盒，可在面板中点击装载)", offer.preview_summary)
                            };
                            // PRD §4.1.4 样例：「来自 [MacBook-Pro] 的文件 (3个文件, 48.5 MB)」。
                            let files_title = match resolve_peer_name(&app_handle, &from_device).await {
                                Some(name) => format!(
                                    "来自 {name} 的文件 ({}项, {})",
                                    offer.total_items,
                                    human_size(offer.total_size)
                                ),
                                None => format!("UniDrop 文件接收完成 ({}项)", offer.total_items),
                            };
                            if let Err(e) = show_transfer_notification(&app_handle, &files_title, &notification_body) {
                                log::warn!("Failed to show notification: {}", e);
                            }

                            // P1-9: Auto inject into clipboard if configured
                            if auto_inject && still_owned && !completed_paths.is_empty() {
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
            // 熔断路径已经发过 E2EE_AUTH_FAILED 并给出了精准原因，这里既不能改写
            // 本地历史文案，也不能再补一条泛化信令——否则两端最终看到的都是
            // 「连接中断」，而真正的原因（多半是两端 PSK 不一致）被自己盖掉了。
            let (reason, send_generic_failure) = if e2ee_aborted {
                ("内容校验失败：两端密钥可能不一致，或数据在传输中被篡改", false)
            } else {
                ("接收连接中断或校验失败", true)
            };
            finalize_history_status(&app_handle, &session_id, "FAILED", Some(reason)).await;

            if send_generic_failure {
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
}

/// AEAD 校验失败的熔断阈值。单块与会话级共用这一个数。
///
/// CRC32 已算在密文上，线路偶发损坏会先被它拦下走 NACK；能同时通过 CRC32
/// 又 tag 失败的情形实践中几乎只剩篡改或 PSK 不一致（CRC32 碰撞在 2^-32 量级），
/// 所以快速熔断不会误伤正常传输。
pub(crate) const TAG_FAIL_LIMIT: u32 = 3;

/// 记录一次 AEAD 失败，返回是否应当熔断整个会话。
///
/// **双层计数缺一不可**：滑动窗口有 4 个在途块，PSK 不一致时它们会同时 tag 失败，
/// 每块各自只计到 1——只按单块计数的话永远够不到阈值，限次在最常见的触发场景下
/// 完全失效，退化成无限 NACK 重传（传输永不结束，用户看不到任何错误）。
/// 会话级累计才是主力，单块阈值负责「同一块反复失败」这种更窄的情形。
pub(crate) fn record_tag_failure(
    per_chunk: &mut std::collections::HashMap<(u32, u32), u32>,
    total: &mut u32,
    item_index: u32,
    chunk_index: u32,
) -> bool {
    *total += 1;
    let per = per_chunk.entry((item_index, chunk_index)).or_insert(0);
    *per += 1;
    *per >= TAG_FAIL_LIMIT || *total >= TAG_FAIL_LIMIT
}

/// 把一块明文封装成完整数据帧。
///
/// 加密时：payload 换成 AEAD 密文，**CRC32 因此算在密文上**，nonce 写进帧头、
/// 置 `FLAG_ENCRYPTED`。
///
/// CRC32 为什么算密文而不是明文：帧头对中继完全可见，而明文的 32 位指纹足以让
/// 短内容被暴力枚举确认——剪贴板文本正是短内容。加密了正文却在帧头附赠校验和，
/// 等于自己拆掉一半。算在密文上还保留了「解密之前就能拒绝坏块」的能力，
/// 不必对已知损坏的数据做无用的 AES 运算。
///
/// **三条发送路径（首发 / NACK 快重传 / 超时重传）都必须走这个函数。**
/// 漏掉任一条的后果是那条路径发出明文而接收端按加密解析，
/// 表现为随机的 tag 失败——极难定位到是某条重传路径没加密。
fn build_data_frame(
    session_uuid: Uuid,
    item_index: u32,
    chunk_index: u32,
    total_chunks: u32,
    plaintext: &[u8],
    e2ee_key: Option<&ring::aead::LessSafeKey>,
) -> Result<Vec<u8>, String> {
    let (payload, nonce) = match e2ee_key {
        Some(k) => {
            let n = crate::core::e2ee::data_nonce(item_index, chunk_index);
            (crate::core::e2ee::seal(k, n, plaintext)?, Some(n))
        }
        None => (plaintext.to_vec(), None),
    };

    let mut header =
        BinaryHeader::new_data(session_uuid, item_index, chunk_index, total_chunks, &payload);
    if let Some(n) = nonce {
        header.nonce = n;
        header.flags |= FLAG_ENCRYPTED;
    }

    let mut frame = vec![0u8; HEADER_SIZE + payload.len()];
    header
        .encode(&mut frame[..HEADER_SIZE])
        .map_err(|e| e.to_string())?;
    frame[HEADER_SIZE..].copy_from_slice(&payload);
    Ok(frame)
}

fn read_file_chunk(path: &Path, offset: u64, length: usize) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; length];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// 把一块**明文**写进目标文件的对应位置。
///
/// `chunk_len` 必须与发送端切块、以及 `total_chunks` 的计算用同一个值。
/// 四处同源里这一处的失败形态最隐蔽：offset 用错不会报错、不会 CRC 失败、
/// 也不会 tag 失败——每块都成功解密、成功写入，只是写在错误的位置，
/// 直到最后 SHA256 才发现文件是坏的，而那时整个传输已经跑完了。
fn write_payload_chunk(
    path: &Path,
    chunk_index: u32,
    payload: &[u8],
    truncate: bool,
    chunk_len: usize,
) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(truncate)
        .open(path)?;

    let offset = (chunk_index as u64) * (chunk_len as u64);
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
    // 生产代码已全部改走 plaintext_chunk_len，只有测试还需要这个原始上限。
    use crate::protocol::MAX_PAYLOAD_LENGTH;
    use std::io::Write;

    #[test]
    fn redacts_token_at_end_of_query_string() {
        let url = "wss://h/ws/data?session_id=abc&role=sender&device_id=d1&token=secret-tok";
        let out = redact_query_token(url);
        assert!(!out.contains("secret-tok"), "令牌泄漏进日志: {out}");
        assert!(out.ends_with("token=<redacted>"));
        // 排查需要的字段必须原样保留
        assert!(out.contains("session_id=abc"));
        assert!(out.contains("device_id=d1"));
    }

    #[test]
    fn redacts_token_in_middle_and_keeps_trailing_params() {
        // 今天 token 恰好在末位，这条钉的是「换了顺序也不会把后面的参数一起吃掉」。
        let url = "wss://h/ws/data?token=secret-tok&session_id=abc";
        let out = redact_query_token(url);
        assert_eq!(out, "wss://h/ws/data?token=<redacted>&session_id=abc");
    }

    #[test]
    fn url_without_token_is_unchanged() {
        let url = "wss://h/ws/control?device_id=d1";
        assert_eq!(redact_query_token(url), url);
    }

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
        let (offer, valid_paths) = TransferEngine::prepare_offer(session_id, &input_paths, None).unwrap();

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
            TransferEngine::prepare_offer_from_bytes(session_id, "TEXT", "clipboard.txt", b"hello clipboard".to_vec(), None)
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
            TransferEngine::prepare_offer_from_bytes(session_id, "IMAGE", "clipboard.png", big, None).unwrap();
        assert_eq!(offer_big.items[0].total_chunks, 3);
        assert!(offer_big.preview_summary.contains("图片"));

        // Empty payload is rejected
        assert!(TransferEngine::prepare_offer_from_bytes(session_id, "TEXT", "clipboard.txt", Vec::new(), None).is_err());
    }

    // ---------- E2EE ----------

    fn test_key(sid: &Uuid) -> ring::aead::LessSafeKey {
        crate::core::e2ee::derive_session_key("psk-for-test", sid, "acct").unwrap()
    }

    /// 帧头的 CRC32 必须算在**密文**上。
    ///
    /// 算在明文上会把明文的 32 位指纹写进对中继完全可见的帧头，
    /// 短内容（剪贴板文本正是短内容）可被暴力枚举确认——加密了正文却附赠校验和。
    /// 后半条断言（≠ 明文 CRC32）才是真正的守卫：它防的是有人"顺手"改回明文。
    #[test]
    fn crc32_is_computed_over_ciphertext_not_plaintext() {
        let sid = Uuid::new_v4();
        let key = test_key(&sid);
        let plaintext = b"secret clipboard payload";

        let frame = build_data_frame(sid, 0, 0, 1, plaintext, Some(&key)).unwrap();
        let header = BinaryHeader::decode(&frame[..HEADER_SIZE]).unwrap();
        let wire = &frame[HEADER_SIZE..];

        assert_eq!(
            header.checksum,
            crate::protocol::binary_header::compute_crc32(wire),
            "帧头 CRC32 应等于密文的 CRC32"
        );
        assert_ne!(
            header.checksum,
            crate::protocol::binary_header::compute_crc32(plaintext),
            "帧头 CRC32 等于明文的 CRC32——明文指纹泄露给了中继"
        );
        assert_ne!(wire, plaintext, "线路上出现了明文");
        assert_eq!(header.flags & FLAG_ENCRYPTED, FLAG_ENCRYPTED, "未置加密标志位");
    }

    /// 不加密时行为必须与改造前完全一致：CRC32 算在明文上、不置标志位。
    #[test]
    fn plaintext_frame_is_unchanged() {
        let sid = Uuid::new_v4();
        let plaintext = b"plain payload";
        let frame = build_data_frame(sid, 0, 0, 1, plaintext, None).unwrap();
        let header = BinaryHeader::decode(&frame[..HEADER_SIZE]).unwrap();
        assert_eq!(&frame[HEADER_SIZE..], plaintext);
        assert_eq!(
            header.checksum,
            crate::protocol::binary_header::compute_crc32(plaintext)
        );
        assert_eq!(header.flags & FLAG_ENCRYPTED, 0);
    }

    /// 满的加密块整帧不得超过中继的 4MB 上限。
    /// 超了的话中继会 ErrPayloadTooLarge 断连，而客户端只看到"连接被关"。
    #[test]
    fn full_encrypted_frame_fits_the_relay_limit() {
        let sid = Uuid::new_v4();
        let key = test_key(&sid);
        let pt = vec![3u8; crate::core::e2ee::plaintext_chunk_len(true)];
        let frame = build_data_frame(sid, 0, 0, 1, &pt, Some(&key)).unwrap();
        let payload_len = frame.len() - HEADER_SIZE;
        assert_eq!(
            payload_len, MAX_PAYLOAD_LENGTH as usize,
            "满块密文应恰好等于上限（多一字节就会被中继拒收）"
        );
    }

    /// **端到端错位守卫**：跨 3 块的加密传输，按发送端切块 → 成帧 → 接收端解密 →
    /// 按同一块长写盘，重组后必须与原文逐字节相同。
    ///
    /// 这条防的是"四处同源"漏改任意一处：offset 用错时每块都能解密、能写入，
    /// 只有重组后的字节会不同——单看每一块都是成功的。
    #[test]
    fn encrypted_multichunk_roundtrip_is_byte_identical() {
        let sid = Uuid::new_v4();
        let key = test_key(&sid);
        let chunk_len = crate::core::e2ee::plaintext_chunk_len(true);

        // 3 块：两满块 + 一个零头，确保覆盖"最后一块长度不同"的分支
        let original: Vec<u8> = (0..(chunk_len * 2 + 777)).map(|i| (i % 251) as u8).collect();
        let total_chunks = ((original.len() + chunk_len - 1) / chunk_len) as u32;
        assert_eq!(total_chunks, 3);

        let mut rebuilt = vec![0u8; original.len()];
        for c in 0..total_chunks {
            let offset = c as usize * chunk_len;
            let end = (offset + chunk_len).min(original.len());
            let frame =
                build_data_frame(sid, 0, c, total_chunks, &original[offset..end], Some(&key)).unwrap();

            let header = BinaryHeader::decode(&frame[..HEADER_SIZE]).unwrap();
            let wire = &frame[HEADER_SIZE..];
            assert!(header.verify_crc32(wire).is_ok(), "第 {c} 块 CRC32 未通过");

            // 接收端自行重算 nonce，不读 header.nonce
            let nonce = crate::core::e2ee::data_nonce(header.item_index, header.chunk_index);
            let plain = crate::core::e2ee::open(&key, nonce, wire).unwrap();

            // 与 write_payload_chunk 相同的定位方式
            let write_at = header.chunk_index as usize * chunk_len;
            rebuilt[write_at..write_at + plain.len()].copy_from_slice(&plain);
        }

        assert_eq!(rebuilt, original, "重组结果与原文不一致——块长或写盘 offset 不同源");
    }

    /// 加密与不加密的分块数可能不同（块长差 16 字节），而两端必须各自
    /// 按同一个 encrypted 取块长。这条钉死 total_chunks 确实跟着 encrypted 走。
    #[test]
    fn total_chunks_follows_the_encrypted_flag() {
        let sid = Uuid::new_v4();
        let key = test_key(&sid);
        // 恰好等于一个明文满块：不加密时 1 块，加密时因为块长少 16 字节而变成 2 块
        let size = MAX_PAYLOAD_LENGTH as usize;
        let bytes = vec![9u8; size];

        let (plain_offer, _) =
            TransferEngine::prepare_offer_from_bytes(sid, "IMAGE", "a.png", bytes.clone(), None).unwrap();
        let (enc_offer, _) =
            TransferEngine::prepare_offer_from_bytes(sid, "IMAGE", "a.png", bytes, Some(&key)).unwrap();

        assert_eq!(plain_offer.items[0].total_chunks, 1);
        assert_eq!(
            enc_offer.items[0].total_chunks, 2,
            "加密块长少 16 字节，这个尺寸必须多切一块"
        );
    }

    /// 加密 OFFER 序列化后**不得**出现明文文件名与 sha256。
    ///
    /// 直接对序列化后的字符串做子串断言，而不是检查结构体字段——
    /// 后者在新增字段时不会跟着变红，而真正上线的是这串 JSON。
    #[test]
    fn encrypted_offer_leaks_no_filename_or_hash() {
        let sid = Uuid::new_v4();
        let key = test_key(&sid);
        let (offer, _) = TransferEngine::prepare_offer_from_bytes(
            sid, "TEXT", "my-secret-notes.txt", b"hello".to_vec(), Some(&key),
        ).unwrap();

        let mut hasher = Sha256::new();
        hasher.update(b"hello");
        let real_hash = hex::encode(hasher.finalize());

        // 发往中继的是密封副本，断言必须打在它身上
        let wire = crate::core::e2ee::sealed_offer_for_wire(&offer, &key).unwrap();
        let json = serde_json::to_string(&wire).unwrap();
        assert!(!json.contains("my-secret-notes.txt"), "文件名明文出现在 OFFER 里");
        assert!(!json.contains(&real_hash), "内容 sha256 明文出现在 OFFER 里——查表即可确认传了哪个文件");
        assert!(!json.contains("文本: hello"), "预览摘要明文出现在 OFFER 里");
        assert!(wire.encrypted);
        assert_eq!(wire.e2ee_version, Some(crate::core::e2ee::E2EE_VERSION));

        // 服务端限额要读的字段必须仍是明文，否则限额会静默失效或误拒
        assert_eq!(wire.total_size, 5);
        assert_eq!(wire.total_items, 1);
        assert_eq!(wire.data_type, "TEXT");
        assert_eq!(wire.items[0].size, 5);

        // 接收端解密后必须原样还原
        let mut received = wire.clone();
        crate::core::e2ee::open_offer_metadata(&mut received, &key).unwrap();
        assert_eq!(received.items[0].relative_path, "my-secret-notes.txt");
        assert_eq!(received.items[0].sha256, real_hash);
        assert!(received.preview_summary.contains("hello"));
    }

    /// **发送端本地必须保留明文元数据。**
    ///
    /// 这条与 `encrypted_offer_leaks_no_filename_or_hash` 是一正一反的一对：
    /// 那条管「线上不能有明文」（安全属性），这条管「本地还得看得见」（功能属性）。
    /// 先前只有前者，于是密封原地抹空了 offer，而那个 offer 随后进了发送端的
    /// pending_outbound 与历史库——线上确实干净了，发送端自己的卡片与历史
    /// 也一起变成了空文件名，全套测试照样全绿。
    #[test]
    fn sender_keeps_plaintext_metadata_locally() {
        let sid = Uuid::new_v4();
        let key = test_key(&sid);
        let (local, _) = TransferEngine::prepare_offer_from_bytes(
            sid, "TEXT", "my-notes.txt", b"hello".to_vec(), Some(&key),
        ).unwrap();

        // 本地这一份：标记为加密，但元数据仍是明文，供卡片与历史使用
        assert!(local.encrypted, "本地这份也要标记加密，块长与接收端据此判断");
        assert_eq!(local.items[0].relative_path, "my-notes.txt");
        assert!(!local.items[0].sha256.is_empty());
        assert!(local.preview_summary.contains("hello"));
        assert!(local.encrypted_metadata.is_none(), "本地这份不该带密文元数据");

        // 发往中继的那一份：抹空且带密文
        let wire = crate::core::e2ee::sealed_offer_for_wire(&local, &key).unwrap();
        assert_eq!(wire.items[0].relative_path, "");
        assert_eq!(wire.items[0].sha256, "");
        assert_eq!(wire.preview_summary, "");
        assert!(wire.encrypted_metadata.is_some());

        // 密封不得回头改动原件
        assert_eq!(local.items[0].relative_path, "my-notes.txt", "密封污染了本地原件");
    }

    /// 用错的密钥解 OFFER 元数据必须失败——这条是"PSK 不一致在信令阶段就被发现"的依据。
    #[test]
    fn offer_metadata_rejects_wrong_key() {
        let sid = Uuid::new_v4();
        let (offer, _) = TransferEngine::prepare_offer_from_bytes(
            sid, "TEXT", "a.txt", b"x".to_vec(), Some(&test_key(&sid)),
        ).unwrap();

        let wire = crate::core::e2ee::sealed_offer_for_wire(&offer, &test_key(&sid)).unwrap();
        let wrong = crate::core::e2ee::derive_session_key("another-psk", &sid, "acct").unwrap();
        let mut received = wire.clone();
        assert!(crate::core::e2ee::open_offer_metadata(&mut received, &wrong).is_err());
    }

    /// **AGY-04**：滑动窗口有 4 个在途块，PSK 不一致时它们同时 tag 失败、
    /// 每块各自只计到 1。只按单块计数的话永远够不到阈值 3，会无限重传。
    /// 这条钉死会话级累计存在。
    #[test]
    fn concurrent_window_failures_trip_the_session_breaker() {
        let mut per = std::collections::HashMap::new();
        let mut total = 0u32;

        // 四个**不同**的块各失败一次，模拟窗口内并发失败
        assert!(!record_tag_failure(&mut per, &mut total, 0, 0));
        assert!(!record_tag_failure(&mut per, &mut total, 0, 1));
        assert!(
            record_tag_failure(&mut per, &mut total, 0, 2),
            "三个不同块各失败一次后必须熔断——只按单块计数会在这里放行并无限重传"
        );
    }

    /// 同一块连续失败同样要熔断（更窄的情形，由单块阈值负责）。
    #[test]
    fn repeated_failure_on_one_chunk_trips_the_breaker() {
        let mut per = std::collections::HashMap::new();
        let mut total = 0u32;
        assert!(!record_tag_failure(&mut per, &mut total, 5, 5));
        assert!(!record_tag_failure(&mut per, &mut total, 5, 5));
        assert!(record_tag_failure(&mut per, &mut total, 5, 5));
    }
}
