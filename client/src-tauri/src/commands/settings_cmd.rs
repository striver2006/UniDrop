use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
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
pub async fn cmd_save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    let mut clean_settings = new_settings;
    clean_settings.server_url = clean_settings.server_url.trim().trim_end_matches('/').to_string();
    if !clean_settings.server_url.is_empty()
        && !clean_settings.server_url.starts_with("ws://")
        && !clean_settings.server_url.starts_with("wss://")
    {
        clean_settings.server_url = format!("wss://{}", clean_settings.server_url);
    }

    let json_str = serde_json::to_string(&clean_settings).map_err(|e| e.to_string())?;
    {
        let conn = state.db_conn.lock().await;
        crate::storage::db::save_persisted_settings(&conn, &json_str).map_err(|e| e.to_string())?;
    }

    // 1. Update in-memory settings
    {
        let mut s = state.settings.lock().await;
        *s = clean_settings.clone();
    }

    // 2. Update dynamic actor config
    {
        let mut cfg = state.config_actor.write().await;
        cfg.server_url = clean_settings.server_url.clone();
        cfg.account_id = clean_settings.account_id.clone();
        cfg.psk_secret = clean_settings.psk_secret.clone();
    }

    // 3. Clear online devices from previous server/account and notify frontend
    {
        let mut devs = state.online_devices.lock().await;
        devs.clear();
    }
    let _ = app.emit("devices-updated", ());

    // 4. Trigger immediate actor reconnection with new configuration
    state.reconnect_notify.notify_waiters();
    log::info!("Settings saved and reconnected immediately with new config");

    Ok(())
}
