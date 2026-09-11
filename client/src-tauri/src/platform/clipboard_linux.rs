use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Injects files into Linux clipboard using text/uri-list MIME.
pub fn inject_files_to_clipboard(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    // 1. Build text/uri-list format: file:///path/to/file\r\n
    let mut uri_list = String::new();
    for p in paths {
        let abs_path = p.canonicalize().unwrap_or_else(|_| p.clone());
        let uri = format!("file://{}\r\n", abs_path.to_string_lossy());
        uri_list.push_str(&uri);
    }

    // 2. Check if Wayland is active (WAYLAND_DISPLAY)
    let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    if is_wayland {
        // Use wl-copy -t text/uri-list
        match Command::new("wl-copy")
            .arg("-t")
            .arg("text/uri-list")
            .stdin(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(uri_list.as_bytes()).map_err(|e| e.to_string())?;
                }
                child.wait().map_err(|e| e.to_string())?;
                return Ok(());
            }
            Err(_) => {
                return Err("wl-copy is not installed. Please install wl-clipboard (e.g. sudo apt install wl-clipboard)".into());
            }
        }
    } else {
        // X11 xclip fallback
        match Command::new("xclip")
            .arg("-selection")
            .arg("clipboard")
            .arg("-t")
            .arg("text/uri-list")
            .stdin(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(uri_list.as_bytes()).map_err(|e| e.to_string())?;
                }
                child.wait().map_err(|e| e.to_string())?;
                return Ok(());
            }
            Err(_) => {
                return Err("xclip is not installed. Please install xclip (e.g. sudo apt install xclip)".into());
            }
        }
    }
}
