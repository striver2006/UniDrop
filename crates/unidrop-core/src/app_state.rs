use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use rusqlite::Connection;
use tokio::sync::{mpsc, Mutex, Notify, RwLock};

use crate::core::cache_manager::CacheManager;
use crate::core::connection_actor::ConnectionConfig;
use crate::core::transfer_engine::{TransferEngine, TransferSource};
use crate::protocol::{ControlEnvelope, OnlineDevice, ServerLimits, TransferOfferPayload};
use crate::settings::AppSettings;

/// 应用版本的唯一事实源，编译期取自 workspace 的 `version`。
///
/// 此前这里和桌面壳各写了一份 "0.1.0" 字面量，而各处 manifest 都已是更新版本
/// ——对端设备列表里显示的版本号因此始终停在旧值。字面量不会随发版更新，
/// 用 env! 从根上断掉这种漂移。
///
/// **三端（桌面 / 移动）共享本 crate，因此天然同号**——这正是
/// `peer_supports_e2ee` 版本门槛想要的形态。发版时只改 workspace 的
/// `[workspace.package] version` 一处。
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

    /// 服务端在 AUTH_RESPONSE 里下发的传输限额。
    ///
    /// `None` 有两种来源，行为相同：尚未连上，或对端是不发这个字段的老服务端。
    /// 两种情况都**不做本地预检**，沿用 `send_flow` 里的兜底常量——
    /// 绝不能当成「无限制」或「全部为 0」来用。
    pub server_limits: Arc<Mutex<Option<ServerLimits>>>,
}

/// 宿主相关的启动参数。
///
/// 桌面壳填：hostname 来自 `whoami`，cache_dir 为 None（沿用各平台默认缓存目录）。
/// 移动壳填：hostname 来自平台设备名（iOS `UIDevice.name` / Android `Build.MODEL` /
/// 鸿蒙 `deviceInfo.marketName`），cache_dir 必须显式给——移动端没有可靠的环境变量
/// 兜底，且收件目录须落在用户可见位置（iOS Documents/UniDrop、
/// Android/ohos 外部私有目录），详见 V2 计划 §4。
pub struct HostEnv {
    pub hostname: String,
    pub cache_dir: Option<PathBuf>,
}

impl AppState {
    pub fn new(
        db_conn: Arc<Mutex<Connection>>,
        outgoing_tx: mpsc::Sender<ControlEnvelope>,
        device_id: String,
        initial_settings: AppSettings,
        config_actor: Arc<RwLock<ConnectionConfig>>,
        reconnect_notify: Arc<Notify>,
        host: HostEnv,
    ) -> Self {
        let cache_manager =
            CacheManager::with_dir(db_conn.clone(), host.cache_dir).expect("Failed to initialize CacheManager");
        let transfer_engine = Arc::new(TransferEngine::new(cache_manager.clone()));

        Self {
            device_id,
            hostname: host.hostname,
            // std::env::consts::OS 在桌面产出 windows/macos/linux，
            // 在移动端产出 ios/android，在鸿蒙产出 ohos——与服务端的
            // 自由字符串约定（仅存储与日志）正好对齐。
            os_type: std::env::consts::OS.to_string(),
            app_version: APP_VERSION.to_string(),
            db_conn,
            cache_manager,
            transfer_engine,
            online_devices: Arc::new(Mutex::new(Vec::new())),
            settings: Arc::new(Mutex::new(initial_settings)),
            outgoing_tx,
            pending_outbound: Arc::new(Mutex::new(HashMap::new())),
            config_actor,
            reconnect_notify,
            server_limits: Arc::new(Mutex::new(None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 版本号必须与 workspace manifest 同步。此前这里与桌面壳各写死一份 "0.1.0"，
    /// 而版本早已更新，对端设备因此一直显示旧版本。
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
