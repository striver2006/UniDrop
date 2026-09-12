use tauri::WebviewWindow;

#[tauri::command]
pub fn cmd_hide_window(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}
