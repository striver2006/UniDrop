use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::{path_from_file_uri, ClipboardContent};

fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
}

/// Runs a command capturing stdout as bytes; None on spawn failure or non-zero exit.
fn run_capture(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output.stdout)
}

/// MIME types currently offered by the clipboard ("wl-paste -l" / "xclip -t TARGETS").
fn available_types() -> Vec<String> {
    let out = if is_wayland() {
        run_capture("wl-paste", &["-l"])
    } else {
        run_capture("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"])
    };
    match out {
        Some(bytes) => String::from_utf8_lossy(&bytes)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        None => Vec::new(),
    }
}

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

/// Reads the Linux clipboard: uri-list file URIs > PNG image > plain text.
pub fn read_clipboard() -> ClipboardContent {
    let types = available_types();

    // 1. File list via text/uri-list
    if types.iter().any(|t| t.starts_with("text/uri-list")) {
        let out = if is_wayland() {
            run_capture("wl-paste", &["-t", "text/uri-list"])
        } else {
            run_capture("xclip", &["-selection", "clipboard", "-t", "text/uri-list", "-o"])
        };
        if let Some(bytes) = out {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let paths: Vec<PathBuf> = text
                .lines()
                .filter(|l| !l.starts_with('#'))
                .filter_map(|l| path_from_file_uri(l.trim()))
                .map(PathBuf::from)
                .collect();
            if !paths.is_empty() {
                return ClipboardContent::Files(paths);
            }
        }
    }

    // 2. Image via image/png
    if types.iter().any(|t| t == "image/png") {
        let out = if is_wayland() {
            run_capture("wl-paste", &["-t", "image/png"])
        } else {
            run_capture("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"])
        };
        if let Some(bytes) = out {
            if bytes.len() > 8 && bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
                return ClipboardContent::Image(bytes);
            }
        }
    }

    // 3. Plain text
    let out = if is_wayland() {
        run_capture("wl-paste", &["--no-newline"])
    } else {
        run_capture("xclip", &["-selection", "clipboard", "-o"])
    };
    if let Some(bytes) = out {
        let text = String::from_utf8_lossy(&bytes).to_string();
        if !text.is_empty() {
            return ClipboardContent::Text(text);
        }
    }

    ClipboardContent::Empty
}

/// Pipes bytes into a clipboard command that reads stdin.
fn pipe_to_clipboard(program: &str, args: &[&str], data: &[u8], install_hint: &str) -> Result<(), String> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|_| install_hint.to_string())?;
    let mut child = child;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(data).map_err(|e| e.to_string())?;
    }
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn write_text_to_clipboard(text: &str) -> Result<(), String> {
    if is_wayland() {
        pipe_to_clipboard("wl-copy", &[], text.as_bytes(), "wl-copy is not installed. Please install wl-clipboard (e.g. sudo apt install wl-clipboard)")
    } else {
        pipe_to_clipboard("xclip", &["-selection", "clipboard", "-i"], text.as_bytes(), "xclip is not installed. Please install xclip (e.g. sudo apt install xclip)")
    }
}

pub fn write_image_to_clipboard(png: &[u8]) -> Result<(), String> {
    if is_wayland() {
        pipe_to_clipboard("wl-copy", &["-t", "image/png"], png, "wl-copy is not installed. Please install wl-clipboard (e.g. sudo apt install wl-clipboard)")
    } else {
        pipe_to_clipboard("xclip", &["-selection", "clipboard", "-t", "image/png", "-i"], png, "xclip is not installed. Please install xclip (e.g. sudo apt install xclip)")
    }
}
