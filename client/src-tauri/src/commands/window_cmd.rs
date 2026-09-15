use tauri::WebviewWindow;

#[tauri::command]
pub fn cmd_hide_window(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

/// 查询菜单栏图标是否真被系统放上了菜单栏。
///
/// 与 `PLACEMENT_EVENT` 事件构成「推 + 拉」双通道，两个都要有：
/// 开了「启动即最小化」时窗口是隐藏的，探测很可能早于前端注册 listener，
/// 只推不拉会让横幅永远不出现。前端须在每次 `fetchInitialData` 里拉一遍，
/// 而不只是首次挂载时。
///
/// 返回 `None` 表示「本平台没有这套机制」（Windows / Linux 的托盘不受此限），
/// 前端据此什么都不显示。刻意用 `serde_json::Value` 而不是把 macOS 专用的
/// 载荷类型提到跨平台层：那会让一个平台特有的概念长进三平台共用的类型里，
/// 而它在另外两个平台上永远是 `None`。
#[tauri::command]
pub async fn cmd_get_tray_placement(
    #[allow(unused_variables)] app: tauri::AppHandle,
) -> Result<Option<serde_json::Value>, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::platform::tray_placement_macos as tp;
        let payload = tp::current_payload(&app).await;
        return Ok(serde_json::to_value(payload).ok());
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

/// 用户在横幅上点了「知道了，下次不要自动打开」。
///
/// 只关掉「因托盘不可见而强制唤起窗口」这一个行为，**不动** 用户的
/// `start_minimized` 设置——那是他的明确选择，被系统缺陷连累不是改它的理由。
#[tauri::command]
pub async fn cmd_dismiss_tray_guidance(
    #[allow(unused_variables)] app: tauri::AppHandle,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::platform::tray_placement_macos::mark_force_reveal_optout(&app).await;
    }
    Ok(())
}

/// 打开「系统设置 → 菜单栏」。
///
/// URL scheme 是未文档化的，且 `open` 无论目标面板存不存在都会返回成功，
/// 所以**不能**据其返回值认为用户已经到达正确页面——横幅里那条文字路径
/// 必须始终显示，它才是唯一可靠的指引，这个按钮只是顺手。
#[tauri::command]
pub fn cmd_open_menu_bar_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Tahoe 把菜单栏拆成了独立面板；老一点的系统上只有控制中心那一个，
        // 所以第一个打不开时回退过去，至少能落到相邻的设置页。
        const CANDIDATES: [&str; 2] = [
            "x-apple.systempreferences:com.apple.Menubar-Settings.extension",
            "x-apple.systempreferences:com.apple.ControlCenter-Settings.extension",
        ];
        for url in CANDIDATES {
            match std::process::Command::new("open").arg(url).status() {
                Ok(status) if status.success() => return Ok(()),
                Ok(_) => continue,
                Err(e) => return Err(format!("无法打开系统设置：{e}")),
            }
        }
        return Err("无法打开系统设置的菜单栏面板".into());
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("仅 macOS 支持".into())
    }
}

/// 拉取系统通知授权状态（授权引导横幅的「拉」通道，与
/// cmd_get_tray_placement 的推拉口径一致）。
#[tauri::command]
pub fn cmd_get_notification_auth_status(app: tauri::AppHandle) -> Result<Option<bool>, String> {
    Ok(crate::platform::notification::notification_auth_status(&app))
}

/// 打开「系统设置 → 通知」。与 cmd_open_menu_bar_settings 同一批先例：
/// URL scheme 未文档化，文字路径才是可靠指引，按钮只是顺手。
#[tauri::command]
pub fn cmd_open_notification_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Ventura 起通知是独立面板；Monterey 及以前挂在旧的 preference id 下，
        // 所以第一个打不开时回退过去。
        const CANDIDATES: [&str; 2] = [
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
            "x-apple.systempreferences:com.apple.preference.notifications",
        ];
        for url in CANDIDATES {
            match std::process::Command::new("open").arg(url).status() {
                Ok(status) if status.success() => return Ok(()),
                Ok(_) => continue,
                Err(e) => return Err(format!("无法打开系统设置：{e}")),
            }
        }
        return Err("无法打开系统设置的通知面板".into());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:notifications"])
            .status()
            .map_err(|e| format!("无法打开系统设置：{e}"))?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("仅 macOS / Windows 支持".into())
    }
}
