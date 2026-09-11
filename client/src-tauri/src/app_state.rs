use std::sync::Arc;
use rusqlite::Connection;
use tokio::sync::{mpsc, Mutex};

use crate::commands::settings_cmd::AppSettings;
use crate::core::cache_manager::CacheManager;
use crate::core::transfer_engine::TransferEngine;
use crate::protocol::{ControlEnvelope, OnlineDevice};

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
}

impl AppState {
    pub fn new(db_conn: Arc<Mutex<Connection>>, outgoing_tx: mpsc::Sender<ControlEnvelope>) -> Self {
        let cache_manager = CacheManager::new(db_conn.clone()).expect("Failed to initialize CacheManager");
        let transfer_engine = Arc::new(TransferEngine::new(cache_manager.clone()));

        let device_id = uuid::Uuid::new_v4().to_string();
        let hostname = whoami_hostname();
        let os_type = std::env::consts::OS.to_string();
        let app_version = "0.1.0".to_string();

        let default_settings = AppSettings {
            server_url: "ws://127.0.0.1:8080".to_string(),
            account_id: "default_user".to_string(),
            psk_secret: "YOUR_SHARED_SECRET_KEY_HERE".to_string(),
            auto_inject: false,
            rate_limit_mb: 10,
        };

        Self {
            device_id,
            hostname,
            os_type,
            app_version,
            db_conn,
            cache_manager,
            transfer_engine,
            online_devices: Arc::new(Mutex::new(Vec::new())),
            settings: Arc::new(Mutex::new(default_settings)),
            outgoing_tx,
        }
    }
}

fn whoami_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "localhost".to_string())
}
