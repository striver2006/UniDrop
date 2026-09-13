use std::collections::HashMap;
use std::sync::Arc;
use rusqlite::Connection;
use tokio::sync::{mpsc, Mutex, Notify, RwLock};

use crate::commands::settings_cmd::AppSettings;
use crate::core::cache_manager::CacheManager;
use crate::core::connection_actor::ConnectionConfig;
use crate::core::transfer_engine::{TransferEngine, TransferSource};
use crate::protocol::{ControlEnvelope, OnlineDevice, TransferOfferPayload};

/// 应用版本的唯一事实源，编译期取自 Cargo.toml 的 `version`。
///
/// 此前这里和 `lib.rs` 各写了一份 "0.1.0" 字面量，而 Cargo.toml、
/// tauri.conf.json、package.json 都已是 0.1.1——对端设备列表里显示的版本号
/// 因此始终停在 0.1.0。字面量不会随发版更新，用 env! 从根上断掉这种漂移。
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct AppState {
    pub device_id: String,
    pub hostname: String,
    pub os_type: String,
    pub app_version: String,
    pub db_conn: Arc<Mutex<Connection>>,
    pub cache_manager: CacheManager,
    pub transfer_engine: Arc<TransferEngine>,
    pub online_devices: Arc<Mutex<Vec<OnlineDevice>>>,
    pub settings: Arc<Mutex<AppSettings>>,
    pub outgoing_tx: mpsc::Sender<ControlEnvelope>,
    pub pending_outbound: Arc<Mutex<HashMap<String, (TransferOfferPayload, TransferSource)>>>,
    pub config_actor: Arc<RwLock<ConnectionConfig>>,
    pub reconnect_notify: Arc<Notify>,
}

impl AppState {
    pub fn new(
        db_conn: Arc<Mutex<Connection>>,
        outgoing_tx: mpsc::Sender<ControlEnvelope>,
        device_id: String,
        initial_settings: AppSettings,
        config_actor: Arc<RwLock<ConnectionConfig>>,
        reconnect_notify: Arc<Notify>,
    ) -> Self {
        let cache_manager = CacheManager::new(db_conn.clone()).expect("Failed to initialize CacheManager");
        let transfer_engine = Arc::new(TransferEngine::new(cache_manager.clone()));

        let hostname = whoami_hostname();
        let os_type = std::env::consts::OS.to_string();
        let app_version = APP_VERSION.to_string();

        Self {
            device_id,
            hostname,
            os_type,
            app_version,
            db_conn,
            cache_manager,
            transfer_engine,
            online_devices: Arc::new(Mutex::new(Vec::new())),
            settings: Arc::new(Mutex::new(initial_settings)),
            outgoing_tx,
            pending_outbound: Arc::new(Mutex::new(HashMap::new())),
            config_actor,
            reconnect_notify,
        }
    }
}

/// Resolves the real host name via gethostname (works for GUI-launched apps on
/// macOS where the HOSTNAME env var is never set), trimming the mDNS ".local"
/// suffix. Env vars are only a fallback for containerized environments.
pub fn whoami_hostname() -> String {
    if let Ok(name) = whoami::fallible::hostname() {
        let trimmed = name.trim_end_matches(".local").trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "localhost".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 版本号必须与 Cargo.toml 同步。此前 lib.rs 与本文件各写死一份 "0.1.0"，
    /// 而 Cargo.toml 早已是 0.1.1，对端设备列表因此一直显示旧版本。
    #[test]
    fn app_version_tracks_cargo_manifest() {
        assert_eq!(APP_VERSION, env!("CARGO_PKG_VERSION"));
        assert!(!APP_VERSION.is_empty());
        // 形如 x.y.z，防止有人改成别的字面量
        assert_eq!(APP_VERSION.split('.').count(), 3, "版本号应为三段式");
    }

    #[test]
    fn app_version_is_not_a_stale_literal() {
        assert_ne!(APP_VERSION, "0.1.0", "不得退回硬编码的旧版本号");
    }
}
