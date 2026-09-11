pub mod notification;
pub use notification::show_transfer_notification;

#[cfg(target_os = "windows")]
pub mod clipboard_windows;
#[cfg(target_os = "windows")]
pub mod listener_windows;

#[cfg(target_os = "macos")]
pub mod clipboard_macos;
#[cfg(target_os = "macos")]
pub mod listener_macos;

#[cfg(target_os = "linux")]
pub mod clipboard_linux;
#[cfg(target_os = "linux")]
pub mod listener_linux;

use std::path::PathBuf;

/// A snapshot of the current system clipboard content, normalized across platforms.
/// Priority when multiple flavors coexist: file list > image > text.
#[derive(Debug, Clone)]
pub enum ClipboardContent {
    Text(String),
    /// Raw PNG-encoded image bytes
    Image(Vec<u8>),
    Files(Vec<PathBuf>),
    Empty,
}

/// Injects downloaded files into the host operating system's native clipboard.
pub fn inject_files_to_clipboard(paths: &[PathBuf]) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return clipboard_windows::inject_files_to_clipboard(paths);
    }

    #[cfg(target_os = "macos")]
    {
        return clipboard_macos::inject_files_to_clipboard(paths);
    }

    #[cfg(target_os = "linux")]
    {
        return clipboard_linux::inject_files_to_clipboard(paths);
    }

    #[allow(unreachable_code)]
    Err("Unsupported platform for native clipboard injection".into())
}

/// Reads the current system clipboard.
pub fn read_clipboard() -> ClipboardContent {
    #[cfg(target_os = "windows")]
    {
        return clipboard_windows::read_clipboard();
    }

    #[cfg(target_os = "macos")]
    {
        return clipboard_macos::read_clipboard();
    }

    #[cfg(target_os = "linux")]
    {
        return clipboard_linux::read_clipboard();
    }

    #[allow(unreachable_code)]
    ClipboardContent::Empty
}

/// Writes plain UTF-8 text into the system clipboard.
pub fn write_text_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return clipboard_windows::write_text_to_clipboard(text);
    }

    #[cfg(target_os = "macos")]
    {
        return clipboard_macos::write_text_to_clipboard(text);
    }

    #[cfg(target_os = "linux")]
    {
        return clipboard_linux::write_text_to_clipboard(text);
    }

    #[allow(unreachable_code)]
    Err("Unsupported platform for clipboard write".into())
}

/// Writes a PNG image into the system clipboard (auto-converted to the native
/// image format where required, e.g. CF_DIB on Windows).
pub fn write_image_to_clipboard(png: &[u8]) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return clipboard_windows::write_image_to_clipboard(png);
    }

    #[cfg(target_os = "macos")]
    {
        return clipboard_macos::write_image_to_clipboard(png);
    }

    #[cfg(target_os = "linux")]
    {
        return clipboard_linux::write_image_to_clipboard(png);
    }

    #[allow(unreachable_code)]
    Err("Unsupported platform for clipboard write".into())
}

/// Minimal percent-decoding for file:// URIs ("%20" -> " ", ...). Returns None
/// on malformed escape sequences so callers can skip the URI.
pub(crate) fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = (*bytes.get(i + 1)? as char).to_digit(16)?;
            let lo = (*bytes.get(i + 2)? as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Extracts a filesystem path from a "file://host/path" URI string.
pub(crate) fn path_from_file_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let path = percent_decode(rest)?;
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percent_decode_and_file_uri() {
        assert_eq!(percent_decode("plain"), Some("plain".to_string()));
        assert_eq!(percent_decode("a%20b%2Fc"), Some("a b/c".to_string()));
        assert_eq!(percent_decode("bad%zz"), None);
        assert_eq!(percent_decode("trunc%2"), None);

        assert_eq!(
            path_from_file_uri("file:///Users/foo/My%20File.txt"),
            Some("/Users/foo/My File.txt".to_string())
        );
        assert_eq!(
            path_from_file_uri("file://localhost/tmp/a.txt"),
            Some("/tmp/a.txt".to_string())
        );
        assert_eq!(path_from_file_uri("https://example.com"), None);
    }
}

#[cfg(target_os = "macos")]
pub use listener_macos::start_clipboard_listener;
#[cfg(target_os = "windows")]
pub use listener_windows::start_clipboard_listener;
#[cfg(target_os = "linux")]
pub use listener_linux::start_clipboard_listener;
