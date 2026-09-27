//! 宿主桥：core 与具体壳（Tauri 桌面 / Flutter 移动）之间唯一的边界。
//!
//! 设计约束：
//! - 方法全部**同步**。实现方若需异步（如 Tauri 的 emit 本身是同步的、
//!   Flutter 侧的事件回调是跨线程投递），在实现内部自行处理；
//!   调用方（传输引擎）处于 async 上下文，同步方法可直接调用。
//! - `emit` 的事件名与载荷结构和桌面端 Tauri 事件**逐字段一致**
//!   （前端两侧共用同一套事件协议），移动端壳把 JSON 原样推给 Dart。

use std::path::PathBuf;

/// 接收策略（v1 计划 §1.3-A / §4.3 的修复：接收端不再无条件自动接受）。
///
/// 序列化为小写字符串存进 AppSettings；默认 `Always` 保持桌面端旧行为，
/// 移动端首启默认建议 `WifiOnly`（由移动壳在初始化默认设置时指定，
/// 而不是改这里的 serde default——桌面老库兼容优先）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReceivePolicy {
    /// 全部自动接受（桌面端旧行为，也是桌面端默认）。
    #[serde(rename = "always")]
    Always,
    /// 仅 Wi-Fi / 以太网自动接受；蜂窝网络下按数据类型分流：
    /// 小体积（TEXT/IMAGE）仍自动收，文件类转 Ask。
    #[serde(rename = "wifi_only")]
    WifiOnly,
    /// 每次弹确认，超时未响应则拒绝（走既有 reject_reason 通路）。
    #[serde(rename = "ask")]
    Ask,
}

impl Default for ReceivePolicy {
    fn default() -> Self {
        Self::Always
    }
}

/// 网络类型判定结果。`Unknown` 仅在宿主未实现判定时出现；
/// 判定源（iOS NWPathMonitor / Android ConnectivityManager）可靠时
/// 不应出现 Unknown。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkKind {
    Wifi,
    Cellular,
    Ethernet,
    Unknown,
}

/// core 对宿主的能力需求。
///
/// 桌面实现（`client/src-tauri`）：emit → tauri 事件；notify → 系统通知；
/// 剪贴板三件套 → `platform` 模块的 OS 实现。
///
/// 移动实现（`client-mobile/native`）：emit → C 回调推给 Dart；
/// notify → 以事件形式交给 Dart 侧通知插件；剪贴板 → 以事件形式交给
/// Dart 侧系统剪贴板 API（尽力而为，无法同步回执，见实现侧说明）。
pub trait HostBridge: Send + Sync + 'static {
    /// 发送一条 UI 事件。事件名与桌面端 Tauri 事件一致，载荷为 JSON。
    fn emit(&self, event: &str, payload: &serde_json::Value);

    /// 系统通知（接收完成 / 接收失败的用户可见提示）。
    /// 实现失败只记日志，不向上传播——通知是尽力而为的辅助通道。
    fn notify(&self, title: &str, body: &str);

    /// 把文本写入系统剪贴板（接收 TEXT 自动注入 / 装载会话）。
    fn write_clipboard_text(&self, text: String) -> Result<(), String>;

    /// 把 PNG 写入系统剪贴板（接收 IMAGE 自动注入 / 装载会话）。
    fn write_clipboard_image(&self, png: Vec<u8>) -> Result<(), String>;

    /// 把文件列表注入系统剪贴板（文件级粘贴）。
    /// 移动端语义为「尽力而为」：Android 写 content:// ClipData、iOS 无系统级
    /// 等价物（由 Dart 侧引导分享 / 保存），返回 Ok 不代表已入剪贴板。
    fn inject_files_to_clipboard(&self, paths: Vec<PathBuf>) -> Result<(), String>;

    /// 当前网络类型。桌面实现返回 Ethernet/Unknown（桌面不受蜂窝策略约束）。
    fn network_kind(&self) -> NetworkKind {
        NetworkKind::Unknown
    }
}

/// 便捷封装：以 JSON 载荷发出结构化事件。
pub fn emit_json<T: serde::Serialize>(bridge: &dyn HostBridge, event: &str, payload: &T) {
    match serde_json::to_value(payload) {
        Ok(v) => bridge.emit(event, &v),
        Err(e) => log::error!("序列化事件 {} 载荷失败: {}", event, e),
    }
}
