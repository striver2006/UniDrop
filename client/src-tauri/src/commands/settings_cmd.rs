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
    let json_str = serde_json::to_string(&new_settings).map_err(|e| e.to_string())?;
    {
        let conn = state.db_conn.lock().await;
        crate::storage::db::save_persisted_settings(&conn, &json_str).map_err(|e| e.to_string())?;
    }
    let mut s = state.settings.lock().await;
    *s = new_settings;
    Ok(())
}
