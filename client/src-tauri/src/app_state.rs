use std::collections::HashMap;
use std::sync::Arc;
use rusqlite::Connection;
use tokio::sync::{mpsc, Mutex, Notify, RwLock};

use crate::commands::settings_cmd::AppSettings;
use crate::core::cache_manager::CacheManager;
use crate::core::connection_actor::ConnectionConfig;
use crate::core::transfer_engine::{TransferEngine, TransferSource};
use crate::protocol::{ControlEnvelope, OnlineDevice, TransferOfferPayload};

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
        let app_version = "0.1.0".to_string();

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
