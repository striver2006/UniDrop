//! 设置面板的 IPC 壳。模型与保存流程在 `unidrop_core::settings`，
//! 这里只做 Tauri command 胶水与桌面专属的自启管理。

use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;
use unidrop_core::app_state::AppState;

// 旧路径兼容 re-export：`commands::settings_cmd::AppSettings` 在迁移期继续可用。
#[allow(unused_imports)]
pub use unidrop_core::settings::{
    AppSettings, DEFAULT_HISTORY_MAX_ENTRIES, DEFAULT_TRANSFER_CARD_RETAIN_SECS,
    validate_account_id,
};

#[tauri::command]
pub async fn cmd_get_settings(
    state: State<'_, std::sync::Arc<AppState>>,
) -> Result<AppSettings, String> {
    let s = state.settings.lock().await;
    Ok(s.clone())
}

#[tauri::command]
pub async fn cmd_save_settings(
    app: AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    new_settings: AppSettings,
) -> Result<(), String> {
    // 清洗 / 落库 / 重连 / 认领 / 修剪全在 core（桌面与移动共享同一流程），
    // 宿主动作（devices-updated / account-changed 事件）由桌面桥 emit。
    let sink = crate::host_bridge::TauriEventSink::new(app);
    unidrop_core::settings::save_settings_flow(&state, &sink, new_settings).await
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
