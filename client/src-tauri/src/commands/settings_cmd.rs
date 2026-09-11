use serde::{Deserialize, Serialize};
use tauri::State;
use crate::app_state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub server_url: String,
    pub account_id: String,
    pub psk_secret: String,
    pub auto_inject: bool,
    pub rate_limit_mb: u32,
}

#[tauri::command]
pub async fn cmd_get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    let s = state.settings.lock().await;
    Ok(s.clone())
}

#[tauri::command]
pub async fn cmd_save_settings(state: State<'_, AppState>, new_settings: AppSettings) -> Result<(), String> {
    let mut s = state.settings.lock().await;
    *s = new_settings;
    Ok(())
}
