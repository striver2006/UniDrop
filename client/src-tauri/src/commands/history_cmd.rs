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
    let account_id = { state.settings.lock().await.account_id.clone() };
    let conn = state.db_conn.lock().await;
    HistoryRepo::list_history(&conn, &account_id, HISTORY_DISPLAY_CAP).map_err(|e| e.to_string())
}

/// 三个直接操作会话文件的 IPC 命令共用的归属闸门。
///
/// 见 `HistoryRepo::session_belongs_to` 的注释：过滤列表只保证「看不见」，
/// 挡不住「点得动」。切账号后界面若还残留着旧账号的卡片，用户点下去就会
/// 真的读到另一个账号的文件。
pub(crate) async fn ensure_session_owned(state: &State<'_, AppState>, session_id: &str) -> Result<(), String> {
    let account_id = { state.settings.lock().await.account_id.clone() };
    let conn = state.db_conn.lock().await;
    if HistoryRepo::session_belongs_to(&conn, session_id, &account_id) {
        Ok(())
    } else {
        Err("该记录不属于当前账号".to_string())
    }
}

/// Opens a native folder picker and copies all cached files of a session into it.
/// Returns Ok(None) when the user cancels the dialog.
#[tauri::command]
pub async fn cmd_save_transfer_as(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Option<u32>, String> {
    ensure_session_owned(&state, &session_id).await?;
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
    ensure_session_owned(&state, &session_id).await?;
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

#[cfg(test)]
mod tests {
    /// 归属闸门的**接线**守卫。
    ///
    /// 闸门本身（`HistoryRepo::session_belongs_to`）在 repo 层有充分的单测，
    /// 但那测的是判断逻辑；「四个动文件的命令各自真的调了它」这件事，
    /// 在删掉任意一处调用后不会让任何既有测试变红——而「点得动」这条防线
    /// 恰恰完全依赖那几处调用在位。
    ///
    /// `State<'_, AppState>` 在单测里构造不出来（需要完整的 Tauri 运行时），
    /// 所以这里退而求其次做源码级断言。它确实脆弱——改个变量名就会红——
    /// 但对「接线」这类东西，脆弱正是想要的：任何触碰都该让人重新确认一遍
    /// 闸门还在。若将来重构使断言失效，**先确认闸门仍在再改断言**，
    /// 不要反过来。
    #[test]
    fn every_session_file_command_keeps_the_ownership_gate() {
        let history = include_str!("history_cmd.rs");
        let clipboard = include_str!("clipboard_cmd.rs");

        // 按函数切片，逐个确认闸门在自己的函数体内，而不是只数总数——
        // 总数对得上但集中在一个函数里的话，守卫就形同虚设。
        for (src, fn_name) in [
            (history, "pub async fn cmd_save_transfer_as"),
            (history, "pub async fn cmd_reveal_session"),
            (clipboard, "pub async fn cmd_inject_files"),
            (clipboard, "pub async fn cmd_inject_session"),
        ] {
            let start = src
                .find(fn_name)
                .unwrap_or_else(|| panic!("找不到命令 {fn_name}，它是否被改名或删除？"));
            let rest = &src[start + fn_name.len()..];
            // 到下一个 #[tauri::command] 为止就是本函数的范围
            let end = rest.find("#[tauri::command]").unwrap_or(rest.len());
            let body = &rest[..end];

            assert!(
                body.contains("ensure_session_owned"),
                "{fn_name} 缺少归属闸门 ensure_session_owned —— \
                 它会读取该会话的缓存文件，切账号后可能读到另一个账号的内容"
            );
        }
    }
}
