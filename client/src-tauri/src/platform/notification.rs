use tauri::AppHandle;

#[cfg(not(target_os = "macos"))]
use tauri_plugin_notification::NotificationExt;

/// 投递一条传输相关的系统通知。
///
/// macOS 走 UNUserNotificationCenter（见 notification_macos.rs），不走
/// tauri-plugin-notification——后者底层的 NSUserNotificationCenter 已被系统废弃，
/// 在 macOS 26 上不再投递任何通知。Windows / Linux 维持插件实现不变。
pub fn show_transfer_notification(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        super::notification_macos::show_notification(app, title, body)
    }

    #[cfg(not(target_os = "macos"))]
    {
        app.notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}
