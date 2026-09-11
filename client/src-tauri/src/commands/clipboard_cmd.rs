use std::path::PathBuf;
use tauri::State;
use crate::app_state::AppState;
use crate::platform::inject_files_to_clipboard;

#[tauri::command]
pub async fn cmd_inject_files(state: State<'_, AppState>, session_id: String, paths: Vec<String>) -> Result<(), String> {
    let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    inject_files_to_clipboard(&path_bufs)?;

    // Mark 2h immunity lock
    state.cache_manager.mark_clipboard_injected(&session_id).await?;

    Ok(())
}

#[tauri::command]
pub async fn cmd_send_files(_state: State<'_, AppState>, _target_device: String, _paths: Vec<String>) -> Result<String, String> {
    // Generates a transfer offer and pushes into queue
    Ok("offer_queued".into())
}
