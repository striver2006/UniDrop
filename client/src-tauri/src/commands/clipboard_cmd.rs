use std::path::PathBuf;

use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::core::transfer_engine::{TransferEngine, TransferSource};
use crate::platform::{
    inject_files_to_clipboard, read_clipboard, write_image_to_clipboard, write_text_to_clipboard,
    ClipboardContent,
};
use crate::protocol::{ActionType, ControlEnvelope, TransferOfferPayload};
use crate::storage::HistoryRepo;

const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

#[tauri::command]
pub async fn cmd_inject_files(
    state: State<'_, AppState>,
    session_id: String,
    paths: Option<Vec<String>>,
) -> Result<(), String> {
    let path_bufs: Vec<PathBuf> = match paths {
        Some(p) if !p.is_empty() => p.into_iter().map(PathBuf::from).collect(),
        _ => state.cache_manager.get_session_files(&session_id).await?,
    };

    if path_bufs.is_empty() {
        return Err(format!("No files found to inject for session {}", session_id));
    }

    // P1-10: Execute synchronous clipboard FFI in spawn_blocking
    tokio::task::spawn_blocking(move || inject_files_to_clipboard(&path_bufs))
        .await
        .map_err(|e| e.to_string())??;

    // Mark 2h immunity lock (M2)
    state.cache_manager.mark_clipboard_injected(&session_id).await?;

    Ok(())
}

/// Summary of the current clipboard for the send dialog (no data leaves the machine).
#[derive(Debug, Serialize)]
pub struct ClipboardPreview {
    pub kind: String, // "TEXT" | "IMAGE" | "FILES" | "EMPTY"
    pub summary: String,
    pub count: usize,
    pub size_bytes: u64,
}

fn build_preview(content: ClipboardContent) -> ClipboardPreview {
    match content {
        ClipboardContent::Text(t) => {
            let head: String = t.chars().take(30).collect::<String>().replace('\n', " ");
            ClipboardPreview {
                kind: "TEXT".to_string(),
                summary: format!("“{}”", head),
                count: 1,
                size_bytes: t.len() as u64,
            }
        }
        ClipboardContent::Image(png) => {
            let dims = image::load_from_memory(&png)
                .map(|img| format!("{}×{}", img.width(), img.height()))
                .unwrap_or_default();
            let kb = png.len() as f64 / 1024.0;
            let size_str = if kb >= 1024.0 {
                format!("{:.1} MB", kb / 1024.0)
            } else {
                format!("{:.0} KB", kb)
            };
            let summary = if dims.is_empty() {
                format!("图片 ({})", size_str)
            } else {
                format!("图片 {} ({})", dims, size_str)
            };
            ClipboardPreview {
                kind: "IMAGE".to_string(),
                summary,
                count: 1,
                size_bytes: png.len() as u64,
            }
        }
        ClipboardContent::Files(paths) => {
            let first = paths
                .first()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let summary = if paths.len() == 1 {
                first
            } else {
                format!("{} 等 {} 个文件", first, paths.len())
            };
            ClipboardPreview {
                kind: "FILES".to_string(),
                summary,
                count: paths.len(),
                size_bytes: 0,
            }
        }
        ClipboardContent::Empty => ClipboardPreview {
            kind: "EMPTY".to_string(),
            summary: "剪贴板为空或不支持的内容类型".to_string(),
            count: 0,
            size_bytes: 0,
        },
    }
}

#[tauri::command]
pub async fn cmd_read_clipboard_preview() -> Result<ClipboardPreview, String> {
    tokio::task::spawn_blocking(|| build_preview(read_clipboard()))
        .await
        .map_err(|e| e.to_string())
}

/// Stores the pending outbound offer, sends TRANSFER_OFFER and records history.
async fn dispatch_offer(
    state: &State<'_, AppState>,
    target_device: &str,
    offer: TransferOfferPayload,
    source: TransferSource,
) -> Result<String, String> {
    let session_id = offer.session_id.clone();

    let offer_env = ControlEnvelope {
        version: 1,
        trace_id: Uuid::new_v4().to_string(),
        action: ActionType::TRANSFER_OFFER,
        from_device: state.device_id.clone(),
        to_device: Some(target_device.to_string()),
        timestamp: crate::core::connection_actor::current_time_ms(),
        payload: serde_json::to_value(&offer).map_err(|e| e.to_string())?,
    };

    {
        let mut pending = state.pending_outbound.lock().await;
        pending.insert(session_id.clone(), (offer.clone(), source));
    }

    state.outgoing_tx.send(offer_env).await.map_err(|e| e.to_string())?;

    {
        let conn = state.db_conn.lock().await;
        let _ = HistoryRepo::record_task(&conn, &session_id, target_device, "SEND", &offer, "TRANSFERRING");
    }

    Ok(session_id)
}

#[tauri::command]
pub async fn cmd_send_files(state: State<'_, AppState>, target_device: String, paths: Vec<String>) -> Result<String, String> {
    if paths.is_empty() {
        return Err("No files specified".into());
    }

    let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let session_id = Uuid::new_v4();

    // 1. Prepare offer in spawn_blocking (P1-10: prevent hashing from blocking Tokio worker)
    let paths_for_hash = path_bufs.clone();
    let (offer_payload, valid_paths) = tokio::task::spawn_blocking(move || {
        TransferEngine::prepare_offer(session_id, &paths_for_hash)
    })
    .await
    .map_err(|e| e.to_string())??;

    // 2. Dispatch offer envelope (R2 / N2: 1:1 aligned pending entry)
    dispatch_offer(&state, &target_device, offer_payload, TransferSource::Files(valid_paths)).await
}

/// Reads the local clipboard (text / image / file list) and sends it as one transfer.
#[tauri::command]
pub async fn cmd_send_clipboard(state: State<'_, AppState>, target_device: String) -> Result<String, String> {
    let session_id = Uuid::new_v4();

    let (offer_payload, source) = tokio::task::spawn_blocking(move || -> Result<(TransferOfferPayload, TransferSource), String> {
        let content = read_clipboard();
        match content {
            ClipboardContent::Text(t) => {
                if t.len() > MAX_TEXT_BYTES {
                    return Err(format!("文本过大 ({:.1} MB)，超过 4 MB 上限", t.len() as f64 / 1024.0 / 1024.0));
                }
                TransferEngine::prepare_offer_from_bytes(session_id, "TEXT", "clipboard.txt", t.into_bytes())
            }
            ClipboardContent::Image(png) => {
                if png.len() > MAX_IMAGE_BYTES {
                    return Err(format!("图片过大 ({:.1} MB)，超过 32 MB 上限", png.len() as f64 / 1024.0 / 1024.0));
                }
                TransferEngine::prepare_offer_from_bytes(session_id, "IMAGE", "clipboard.png", png)
            }
            ClipboardContent::Files(paths) => {
                let (offer, valid) = TransferEngine::prepare_offer(session_id, &paths)?;
                Ok((offer, TransferSource::Files(valid)))
            }
            ClipboardContent::Empty => Err("剪贴板为空或不包含可发送内容（支持文本 / 图片 / 文件）".into()),
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    dispatch_offer(&state, &target_device, offer_payload, source).await
}

/// Re-loads a finished session into the clipboard, dispatching on its data_type:
/// FILES -> file references, TEXT -> text, IMAGE -> PNG image.
#[tauri::command]
pub async fn cmd_inject_session(state: State<'_, AppState>, session_id: String) -> Result<String, String> {
    let data_type = {
        let conn = state.db_conn.lock().await;
        HistoryRepo::get_task_brief(&conn, &session_id)
            .map(|b| b.data_type)
            .unwrap_or_else(|| "FILES".to_string())
    };

    let files = state.cache_manager.get_session_files(&session_id).await?;
    if files.is_empty() {
        return Err("该会话在缓存中无文件（可能已被清理）".into());
    }

    match data_type.as_str() {
        "TEXT" => {
            let bytes = std::fs::read(&files[0]).map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&bytes).to_string();
            tokio::task::spawn_blocking(move || write_text_to_clipboard(&text))
                .await
                .map_err(|e| e.to_string())??;
            state.cache_manager.mark_clipboard_injected(&session_id).await?;
            Ok("文本已写入系统剪贴板".into())
        }
        "IMAGE" => {
            let bytes = std::fs::read(&files[0]).map_err(|e| e.to_string())?;
            tokio::task::spawn_blocking(move || write_image_to_clipboard(&bytes))
                .await
                .map_err(|e| e.to_string())??;
            state.cache_manager.mark_clipboard_injected(&session_id).await?;
            Ok("图片已写入系统剪贴板".into())
        }
        _ => {
            let paths = files.clone();
            tokio::task::spawn_blocking(move || inject_files_to_clipboard(&paths))
                .await
                .map_err(|e| e.to_string())??;
            state.cache_manager.mark_clipboard_injected(&session_id).await?;
            Ok("文件已装载至系统剪贴板，可直接粘贴".into())
        }
    }
}
