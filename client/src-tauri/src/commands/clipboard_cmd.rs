//! 发送 / 装载的 IPC 壳。发送业务（限额预检、E2EE 协商、OFFER 构造与派发）
//! 在 `unidrop_core::send_flow`，桌面与移动共享；这里只剩桌面专属的部分：
//! 系统剪贴板读取与注入（FFI）、剪贴板预览摘要。

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, State};

use unidrop_core::app_state::AppState;

use crate::host_bridge::TauriEventSink;
use crate::platform::{
    inject_files_to_clipboard, read_clipboard, write_image_to_clipboard, write_text_to_clipboard,
    ClipboardContent,
};
use unidrop_core::send_flow;
use unidrop_core::storage::HistoryRepo;

#[tauri::command]
pub async fn cmd_inject_files(
    state: State<'_, std::sync::Arc<AppState>>,
    session_id: String,
    paths: Option<Vec<String>>,
) -> Result<(), String> {
    // 第四条会话文件路径，同样要过闸门。
    //
    // 它当前没有任何前端调用点，但注册在 invoke_handler 上就等于对外开放：
    // `paths` 缺省时它回退到 get_session_files(&session_id) 读会话缓存并写剪贴板，
    // 与 cmd_inject_session 是同一条事故链，只是处于休眠状态。
    // `paths` 显式给出时也要拦——它仍然用 session_id 打免疫标记，那是在
    // 改另一个账号的缓存状态。
    crate::commands::history_cmd::ensure_session_owned(&state, &session_id).await?;

    let path_bufs: Vec<PathBuf> = match paths {
        Some(p) if !p.is_empty() => p.into_iter().map(PathBuf::from).collect(),
        _ => state.cache_manager.get_session_files(&session_id).await?,
    };

    if path_bufs.is_empty() {
        return Err(format!("No files found to inject for session {}", session_id));
    }

    // 剪贴板 FFI 是同步调用，放 spawn_blocking 防止阻塞 Tokio worker
    tokio::task::spawn_blocking(move || inject_files_to_clipboard(&path_bufs))
        .await
        .map_err(|e| e.to_string())??;

    // Mark 2h immunity lock (M2)
    state.cache_manager.mark_clipboard_injected(&session_id).await?;

    Ok(())
}

/// Summary of the current clipboard for the send dialog (no data leaves the machine).
#[derive(Debug, Serialize)]
pub struct ClipboardPreview {
    pub kind: String, // "TEXT" | "IMAGE" | "FILES" | "EMPTY"
    pub summary: String,
    pub count: usize,
    pub size_bytes: u64,
}

fn build_preview(content: ClipboardContent) -> ClipboardPreview {
    match content {
        ClipboardContent::Text(t) => {
            let head: String = t.chars().take(30).collect::<String>().replace('\n', " ");
            ClipboardPreview {
                kind: "TEXT".to_string(),
                summary: format!("“{}”", head),
                count: 1,
                size_bytes: t.len() as u64,
            }
        }
        ClipboardContent::Image(png) => {
            let dims = image::load_from_memory(&png)
                .map(|img| format!("{}×{}", img.width(), img.height()))
                .unwrap_or_default();
            let kb = png.len() as f64 / 1024.0;
            let size_str = if kb >= 1024.0 {
                format!("{:.1} MB", kb / 1024.0)
            } else {
                format!("{:.0} KB", kb)
            };
            let summary = if dims.is_empty() {
                format!("图片 ({})", size_str)
            } else {
                format!("图片 {} ({})", dims, size_str)
            };
            ClipboardPreview {
                kind: "IMAGE".to_string(),
                summary,
                count: 1,
                size_bytes: png.len() as u64,
            }
        }
        ClipboardContent::Files(paths) => {
            let first = paths
                .first()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let summary = if paths.len() == 1 {
                first
            } else {
                format!("{} 等 {} 个文件", first, paths.len())
            };
            ClipboardPreview {
                kind: "FILES".to_string(),
                summary,
                count: paths.len(),
                size_bytes: 0,
            }
        }
        ClipboardContent::Empty => ClipboardPreview {
            kind: "EMPTY".to_string(),
            summary: "剪贴板为空或不支持的内容类型".to_string(),
            count: 0,
            size_bytes: 0,
        },
    }
}

#[tauri::command]
pub async fn cmd_read_clipboard_preview() -> Result<ClipboardPreview, String> {
    tokio::task::spawn_blocking(|| build_preview(read_clipboard()))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_send_files(
    app: AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    target_device: String,
    paths: Vec<String>,
) -> Result<String, String> {
    // 限额预检 → E2EE 协商 → 哈希与 OFFER → 派发，全在 core。
    let sink = TauriEventSink::new(app);
    send_flow::send_files_flow(&state, &sink, target_device, paths).await
}

/// Reads the local clipboard (text / image / file list) and sends it as one transfer.
#[tauri::command]
pub async fn cmd_send_clipboard(
    app: AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    target_device: String,
) -> Result<String, String> {
    let sink = TauriEventSink::new(app);
    let content = tokio::task::spawn_blocking(read_clipboard)
        .await
        .map_err(|e| e.to_string())?;

    match content {
        ClipboardContent::Text(t) => {
            send_flow::send_bytes_flow(&state, &sink, target_device, "TEXT", "clipboard.txt", t.into_bytes()).await
        }
        ClipboardContent::Image(png) => {
            send_flow::send_bytes_flow(&state, &sink, target_device, "IMAGE", "clipboard.png", png).await
        }
        ClipboardContent::Files(paths) => {
            // 剪贴板里的文件走与 cmd_send_files 相同的流程（限额预检在 core 内），
            // 否则「复制文件再发送」会绕开限额。
            let paths: Vec<String> = paths
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            send_flow::send_files_flow(&state, &sink, target_device, paths).await
        }
        ClipboardContent::Empty => Err("剪贴板为空或不包含可发送内容（支持文本 / 图片 / 文件）".into()),
    }
}

/// Re-loads a finished session into the clipboard, dispatching on its data_type:
/// FILES -> file references, TEXT -> text, IMAGE -> PNG image.
#[tauri::command]
pub async fn cmd_inject_session(
    state: State<'_, std::sync::Arc<AppState>>,
    session_id: String,
) -> Result<String, String> {
    // 归属闸门。这条路径直接把文件写进系统剪贴板，是「切账号后误装载上一个
    // 账号的文件」这条事故链的终点，必须挡在读文件之前。
    crate::commands::history_cmd::ensure_session_owned(&state, &session_id).await?;

    let data_type = {
        let conn = state.db_conn.lock().await;
        HistoryRepo::get_task_brief(&conn, &session_id)
            .map(|b| b.data_type)
            .unwrap_or_else(|| "FILES".to_string())
    };

    let files = state.cache_manager.get_session_files(&session_id).await?;
    if files.is_empty() {
        return Err("该会话在缓存中无文件（可能已被清理）".into());
    }

    match data_type.as_str() {
        "TEXT" => {
            let bytes = std::fs::read(&files[0]).map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&bytes).to_string();
            tokio::task::spawn_blocking(move || write_text_to_clipboard(&text))
                .await
                .map_err(|e| e.to_string())??;
            state.cache_manager.mark_clipboard_injected(&session_id).await?;
            Ok("文本已写入系统剪贴板".into())
        }
        "IMAGE" => {
            let bytes = std::fs::read(&files[0]).map_err(|e| e.to_string())?;
            tokio::task::spawn_blocking(move || write_image_to_clipboard(&bytes))
                .await
                .map_err(|e| e.to_string())??;
            state.cache_manager.mark_clipboard_injected(&session_id).await?;
            Ok("图片已写入系统剪贴板".into())
        }
        _ => {
            let paths = files.clone();
            tokio::task::spawn_blocking(move || inject_files_to_clipboard(&paths))
                .await
                .map_err(|e| e.to_string())??;
            state.cache_manager.mark_clipboard_injected(&session_id).await?;
            Ok("文件已装载至系统剪贴板，可直接粘贴".into())
        }
    }
}
