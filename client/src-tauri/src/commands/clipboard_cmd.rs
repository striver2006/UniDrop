use std::path::PathBuf;
use tauri::State;
use uuid::Uuid;
use crate::app_state::AppState;
use crate::core::transfer_engine::TransferEngine;
use crate::platform::inject_files_to_clipboard;
use crate::protocol::{ActionType, ControlEnvelope};

#[tauri::command]
pub async fn cmd_inject_files(state: State<'_, AppState>, session_id: String, paths: Vec<String>) -> Result<(), String> {
    let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    inject_files_to_clipboard(&path_bufs)?;

    // Mark 2h immunity lock
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

    // 1. Prepare offer
    let offer_payload = TransferEngine::prepare_offer(session_id, &path_bufs)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

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

    // 3. Dispatch into outgoing channel
    state.outgoing_tx.send(offer_env).await.map_err(|e| e.to_string())?;

    Ok(session_id.to_string())
}
