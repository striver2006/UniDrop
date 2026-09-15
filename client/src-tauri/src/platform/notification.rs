use tauri::AppHandle;

#[cfg(not(target_os = "macos"))]
use tauri_plugin_notification::NotificationExt;

/// 查询系统通知授权状态，供前端决定是否显示引导横幅。
///
/// `None` = 本平台无此概念或尚未探测（macOS dev 裸跑被
/// running_as_app_bundle 拦下、插件查询失败等），前端据此不显示横幅——
/// 判据失效时甩给用户一个查不出所以然的告警，比不告警更糟。
pub fn notification_auth_status(
    #[allow(unused_variables)] app: &AppHandle,
) -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        super::notification_macos::cached_auth_status()
    }

    #[cfg(not(target_os = "macos"))]
    {
        app.notification().is_permission_granted().ok()
    }
}

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
