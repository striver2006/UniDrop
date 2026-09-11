use tauri::State;
use crate::app_state::AppState;
use crate::protocol::OnlineDevice;

#[tauri::command]
pub async fn cmd_get_online_devices(state: State<'_, AppState>) -> Result<Vec<OnlineDevice>, String> {
    let devices = state.online_devices.lock().await;
    Ok(devices.clone())
}

#[tauri::command]
pub async fn cmd_get_self_info(state: State<'_, AppState>) -> Result<OnlineDevice, String> {
    Ok(OnlineDevice {
        device_id: state.device_id.clone(),
        hostname: state.hostname.clone(),
        os_type: state.os_type.clone(),
        app_version: state.app_version.clone(),
        remote_ip: None,
    })
}
