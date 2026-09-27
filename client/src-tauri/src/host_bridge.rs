//! 桌面壳对 `unidrop_core::host::HostBridge` 的实现。
//!
//! emit → Tauri 事件（事件名与载荷 JSON 与移动端 Dart 侧完全同构）；
//! notify → `platform::show_transfer_notification`；
//! 剪贴板三件套 → `platform` 的各 OS 实现。

use std::path::PathBuf;

use unidrop_core::host::{HostBridge, NetworkKind};

/// 桌面端网络判定：恒报 Ethernet。
///
/// 桌面默认策略是 `Always`，这一档不影响任何行为；它只对显式改过
/// `wifi_only` 的桌面用户生效，而把有线桌面当「非蜂窝」放行正是预期语义。
/// 不引入 SystemConfiguration / Win32 网络枚举依赖——为一个小众组合
/// 拉平台库不划算，移动端才是该策略的主场。
///
/// 用具体的 `AppHandle`（默认 Wry runtime）而不是泛型 R：
/// `platform::show_transfer_notification` 的签名就是具体类型。
pub struct TauriEventSink {
    app: tauri::AppHandle,
}

impl TauriEventSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl HostBridge for TauriEventSink {
    fn emit(&self, event: &str, payload: &serde_json::Value) {
        use tauri::Emitter;
        let _ = self.app.emit(event, payload);
    }

    fn notify(&self, title: &str, body: &str) {
        if let Err(e) = crate::platform::show_transfer_notification(&self.app, title, body) {
            log::warn!("Failed to show notification: {}", e);
        }
    }

    fn write_clipboard_text(&self, text: String) -> Result<(), String> {
        crate::platform::write_text_to_clipboard(&text)
    }

    fn write_clipboard_image(&self, png: Vec<u8>) -> Result<(), String> {
        crate::platform::write_image_to_clipboard(&png)
    }

    fn inject_files_to_clipboard(&self, paths: Vec<PathBuf>) -> Result<(), String> {
        crate::platform::inject_files_to_clipboard(&paths)
    }

    fn network_kind(&self) -> NetworkKind {
        NetworkKind::Ethernet
    }
}
