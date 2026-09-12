use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt;
use crate::app_state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub server_url: String,
    pub account_id: String,
    pub psk_secret: String,
    pub auto_inject: bool,
    pub rate_limit_mb: u32,
    /// 启动时不弹出主窗口，仅在托盘常驻。对手动启动与开机自启同样生效。
    ///
    /// `#[serde(default)]` 不可删除：整份设置以一条 JSON 存在 SQLite 的
    /// `local_config` 表里，老库的 JSON 没有本字段。缺了 default，
    /// 反序列化会整条失败并走 `lib.rs` 的 `unwrap_or_else` 静默回落到全部默认值，
    /// 把用户已配置的 server_url / account_id / psk_secret 一起冲掉。
    #[serde(default)]
    pub start_minimized: bool,
}

impl AppSettings {
    /// 首次启动与反序列化失败时的兜底配置（唯一定义点，避免多处字面量漏改）
    pub fn default_config() -> Self {
        Self {
            server_url: "wss://drop.yourdomain.com:58921".to_string(),
            account_id: "default_user".to_string(),
            psk_secret: "dev-insecure-psk-secret".to_string(),
            auto_inject: false,
            rate_limit_mb: 10,
            start_minimized: false,
        }
    }
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
    clean_settings.server_url = clean_settings
        .server_url
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .trim_end_matches('/')
        .to_string();
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
    let _ = app.emit("devices-updated", Vec::<crate::protocol::OnlineDevice>::new());

    // 4. Trigger immediate actor reconnection with new configuration
    state.reconnect_notify.notify_waiters();
    log::info!("Settings saved and reconnected immediately with new config");

    Ok(())
}

/// 读取开机自启的**操作系统实时状态**。
///
/// 自启状态的唯一事实源是操作系统（Windows 注册表 Run 键 / macOS LaunchAgent /
/// Linux ~/.config/autostart），不在 AppSettings 里保留副本——用户完全可能绕过
/// 本应用、在系统设置里改它，有副本就必然漂移。
///
/// **注意「存在」不等于「会生效」**：底层 auto-launch 的 `is_enabled()` 只检查
/// 注册项 / plist 是否存在，**不校验其中的路径是否仍指向当前可执行文件**
/// （auto-launch 0.5.0 `windows.rs:73-83`、`macos.rs:161-176`）。因此把应用移到
/// 别处、或先在 dev 二进制上开启自启后再安装正式版，本函数仍会返回 true，
/// 而开机时拉起的是一个失效路径。返回 true 只保证「注册项在」，
/// 不保证「开机真的能起来」。
#[tauri::command]
pub async fn cmd_get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("读取开机自启状态失败: {}", e))
}

/// 设置开机自启。幂等：目标态与 OS 现值一致时直接返回，不触碰系统。
///
/// 幂等是刻意的：macOS 13+ 每次重写 LaunchAgent 都会弹一次「后台项已添加」横幅，
/// 无条件重写会在用户保存任何无关配置时反复打扰。
///
/// 代价是写入后的复查同样只能确认「注册项存在」而非「路径有效」
/// （见 `cmd_get_autostart` 的说明）——要覆盖路径漂移就得强制 disable + enable
/// 重写，而那正好破坏上面这条幂等。本轮选择保幂等。
#[tauri::command]
pub async fn cmd_set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();

    // 比对基准必须是 OS 实时值而不是前端提交时的旧值：用户可能在本次会话中途
    // 于系统设置里改过它，按旧值写回会把用户的操作悄悄覆盖掉。
    let current = manager
        .is_enabled()
        .map_err(|e| format!("读取开机自启状态失败: {}", e))?;
    if current == enabled {
        log::info!("Autostart already {}, skipping OS write", enabled);
        return Ok(());
    }

    if enabled {
        manager
            .enable()
            .map_err(|e| format!("开启开机自启失败: {}", e))?;
    } else {
        manager
            .disable()
            .map_err(|e| format!("关闭开机自启失败: {}", e))?;
    }

    // 复查：让「返回成功」严格等于「OS 里确实是这个状态」
    let applied = manager
        .is_enabled()
        .map_err(|e| format!("校验开机自启状态失败: {}", e))?;
    if applied != enabled {
        return Err(format!(
            "开机自启设置未生效：期望 {}，实际 {}",
            enabled, applied
        ));
    }

    log::info!("Autostart set to {}", enabled);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 老库里的 JSON 没有 start_minimized 字段。本测试守护的是
    /// `#[serde(default)]`：删掉它，这里会红，而不是等到用户配置被静默冲掉。
    #[test]
    fn legacy_settings_json_deserializes_without_data_loss() {
        let legacy = r#"{
            "server_url": "wss://relay.example.com:58921",
            "account_id": "alice",
            "psk_secret": "user-configured-secret",
            "auto_inject": true,
            "rate_limit_mb": 42
        }"#;

        let parsed: AppSettings =
            serde_json::from_str(legacy).expect("老格式设置必须能反序列化，否则用户配置会被冲掉");

        assert_eq!(parsed.server_url, "wss://relay.example.com:58921");
        assert_eq!(parsed.account_id, "alice");
        assert_eq!(parsed.psk_secret, "user-configured-secret");
        assert!(parsed.auto_inject);
        assert_eq!(parsed.rate_limit_mb, 42);
        assert!(!parsed.start_minimized, "缺失的新字段应取默认值 false");
    }

    #[test]
    fn current_settings_json_round_trips() {
        let settings = AppSettings {
            start_minimized: true,
            ..AppSettings::default_config()
        };
        let json = serde_json::to_string(&settings).expect("序列化失败");
        let parsed: AppSettings = serde_json::from_str(&json).expect("反序列化失败");

        assert_eq!(parsed.server_url, settings.server_url);
        assert_eq!(parsed.account_id, settings.account_id);
        assert_eq!(parsed.psk_secret, settings.psk_secret);
        assert_eq!(parsed.auto_inject, settings.auto_inject);
        assert_eq!(parsed.rate_limit_mb, settings.rate_limit_mb);
        assert!(parsed.start_minimized);
    }

    #[test]
    fn autostart_is_not_part_of_persisted_settings() {
        // 自启状态只存在于操作系统，不得出现在落库的 JSON 里
        let json = serde_json::to_string(&AppSettings::default_config()).expect("序列化失败");
        assert!(!json.contains("autostart"));
    }
}
