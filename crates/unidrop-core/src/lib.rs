//! UniDrop 平台无关核心库。
//!
//! 桌面端（Tauri 壳，`client/src-tauri`）与移动端（Flutter FFI 壳，
//! `client-mobile/native`）共享同一份协议、传输引擎、E2EE、TLS 信任、
//! 存储与信令路由实现，保证五端线协议逐字节一致。
//!
//! 宿主差异（UI 事件、系统通知、剪贴板、设备名、路径策略）全部通过
//! [`host::HostBridge`] 注入；本 crate 自身不触碰任何 GUI 框架。

pub mod app_state;
pub mod core;
pub mod host;
pub mod protocol;
pub mod send_flow;
pub mod settings;
pub mod signal_router;
pub mod storage;

/// 安装 rustls 的 ring CryptoProvider（进程级一次）。
///
/// 桌面壳与移动 FFI 层都要在启动第一行调用。它住在 core 是因为
/// provider 的选择与 TLS 信任实现绑定（ring feature），不该让两个壳
/// 各自声明一份依赖再各自装一遍——装两次没有危害但也没意义，
/// 而分散声明迟早漂移成两个 provider。
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
