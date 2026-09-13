use std::path::PathBuf;

use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::app_state::AppState;
use crate::storage::{HistoryRepo, TransferHistoryEntry};

/// 列表**显示**上限，与 `AppSettings::history_max_entries` 的**淘汰**上限是两回事。
///
/// 不做成设置项：它是防御性兜底，不是用户偏好。历史面板没有虚拟滚动，
/// 全量渲染 DOM；用户把保留条数设成 `0`（不限制）后若让查询也跟着不限制，
/// 列表迟早会把界面拖死。另外免疫锁会让库内条数短暂超过保留上限，
/// 显示上限留出余量才不会把用户刚装载过的那条挡在外面。
const HISTORY_DISPLAY_CAP: u32 = 200;

#[tauri::command]
pub async fn cmd_list_history(state: State<'_, AppState>) -> Result<Vec<TransferHistoryEntry>, String> {
    let conn = state.db_conn.lock().await;
    HistoryRepo::list_history(&conn, HISTORY_DISPLAY_CAP).map_err(|e| e.to_string())
}

/// Opens a native folder picker and copies all cached files of a session into it.
/// Returns Ok(None) when the user cancels the dialog.
#[tauri::command]
pub async fn cmd_save_transfer_as(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Option<u32>, String> {
    let files = state.cache_manager.get_session_files(&session_id).await?;
    if files.is_empty() {
        return Err("该会话在缓存中无文件（可能已被清理）".into());
    }

    let folder = app.dialog().file().blocking_pick_folder();
    let Some(folder) = folder else {
        return Ok(None);
    };
    let dest_dir: PathBuf = folder.into_path().map_err(|e| e.to_string())?;
    if !dest_dir.is_dir() {
        return Err("所选位置不是文件夹".into());
    }

    let mut copied = 0u32;
    for src in &files {
        if !src.exists() {
            continue;
        }

        // Name collision: append " (n)" before the extension instead of overwriting
        let file_name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("unidrop-file-{}", copied));
        let mut target = dest_dir.join(&file_name);
        if target.exists() {
            let as_path = PathBuf::from(&file_name);
            let stem = as_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| file_name.clone());
            let ext = as_path
                .extension()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut n = 1u32;
            loop {
                let candidate = if ext.is_empty() {
                    dest_dir.join(format!("{} ({})", stem, n))
                } else {
                    dest_dir.join(format!("{} ({}).{}", stem, n, ext))
                };
                if !candidate.exists() {
                    target = candidate;
                    break;
                }
                n += 1;
            }
        }

        std::fs::copy(src, &target).map_err(|e| format!("复制 {} 失败: {}", src.display(), e))?;
        copied += 1;
    }

    if copied == 0 {
        return Err("缓存文件已不存在（可能已被清理）".into());
    }
    Ok(Some(copied))
}

/// Reveals the first cached file of a session in the platform file manager.
#[tauri::command]
pub async fn cmd_reveal_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    let files = state.cache_manager.get_session_files(&session_id).await?;
    let target = files.first().ok_or("该会话在缓存中无文件（可能已被清理）")?;

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg("-R").arg(target).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer")
        .arg(format!("/select,{}", target.display()))
        .spawn();
    #[cfg(target_os = "linux")]
    let result = {
        let dir = target.parent().unwrap_or(target);
        std::process::Command::new("xdg-open").arg(dir).spawn()
    };

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("打开文件位置失败: {}", e)),
    }
}
