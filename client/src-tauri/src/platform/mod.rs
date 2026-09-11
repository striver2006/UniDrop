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

#[cfg(target_os = "macos")]
pub use listener_macos::start_clipboard_listener;
#[cfg(target_os = "windows")]
pub use listener_windows::start_clipboard_listener;
#[cfg(target_os = "linux")]
pub use listener_linux::start_clipboard_listener;
