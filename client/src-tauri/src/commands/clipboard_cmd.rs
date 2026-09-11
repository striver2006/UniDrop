use std::path::PathBuf;
use tauri::State;
use uuid::Uuid;
use crate::app_state::AppState;
use crate::core::transfer_engine::TransferEngine;
use crate::platform::inject_files_to_clipboard;
use crate::protocol::{ActionType, ControlEnvelope};

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
    tokio::task::spawn_blocking(move || {
        inject_files_to_clipboard(&path_bufs)
    })
    .await
    .map_err(|e| e.to_string())??;

    // Mark 2h immunity lock (M2)
    state.cache_manager.mark_clipboard_injected(&session_id).await?;

    Ok(())
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

    let now = crate::core::connection_actor::current_time_ms();

    // 2. Build TRANSFER_OFFER envelope
    let offer_env = ControlEnvelope {
        version: 1,
        trace_id: Uuid::new_v4().to_string(),
        action: ActionType::TRANSFER_OFFER,
        from_device: state.device_id.clone(),
        to_device: Some(target_device),
        timestamp: now,
        payload: serde_json::to_value(&offer_payload).map_err(|e| e.to_string())?,
    };

    // Store in pending_outbound for when receiver responds with TRANSFER_ANSWER (R2 / N2: 1:1 aligned)
    {
        let mut pending = state.pending_outbound.lock().await;
        pending.insert(session_id.to_string(), (offer_payload, valid_paths));
    }

    // 3. Dispatch into outgoing channel (P0-2: now actively read and sent)
    state.outgoing_tx.send(offer_env).await.map_err(|e| e.to_string())?;

    Ok(session_id.to_string())
}
