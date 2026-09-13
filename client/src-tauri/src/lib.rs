pub mod app_state;
pub mod commands;
pub mod core;
pub mod platform;
pub mod protocol;
pub mod storage;

use std::collections::HashMap;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use tokio::sync::{mpsc, Mutex};

use app_state::AppState;
use core::connection_actor::{ConnectionActor, ConnectionConfig};
use core::transfer_engine::{ActiveTransfer, TransferEngine};
use protocol::{
    ActionType, DeviceListSyncPayload, DeviceOfflinePayload, DeviceOnlinePayload,
    TransferAnswerPayload, TransferOfferPayload,
};
use storage::db::init_database;
use storage::HistoryRepo;

/// 唤起主窗口的唯一入口。
///
/// 「隐藏到托盘」与「最小化到任务栏」是两种不同的形态：后者窗口仍是 visible，
/// 只调 show() + set_focus() 在 Windows 上无法还原，必须先 unminimize()。
/// 托盘菜单、托盘点击、第二实例、macOS Reopen 全部走这里，避免各处行为不一致。
fn reveal_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    match app.get_webview_window("main") {
        Some(win) => {
            if win.is_minimized().unwrap_or(false) {
                let _ = win.unminimize();
            }
            let _ = win.show();
            let _ = win.set_focus();
        }
        None => log::warn!("Requested to reveal main window but it is gone"),
    }
}

/// 按会话 ID 字符串派生密钥。
///
/// session_id 在信令里是字符串，而密钥派生要的是 16 字节原始 UUID
/// （字符串形式有大小写与连字符的歧义，见 `e2ee::derive_session_key`）。
/// 解析失败必须报错而不是回落到一个随便造的 UUID——那会让两端派生出不同的 key，
/// 表现为整批 tag 失败，而真正的原因（ID 格式不对）完全看不出来。
fn derive_key_for(
    session_id: &str,
    psk: &str,
    account_id: &str,
) -> Result<ring::aead::LessSafeKey, String> {
    let uuid = uuid::Uuid::parse_str(session_id)
        .map_err(|e| format!("session_id 不是合法 UUID（{session_id}）: {e}"))?;
    core::e2ee::derive_session_key(psk, &uuid, account_id)
}

pub fn run() {
    env_logger::init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 1. Initialize SQLite local database and persistent identity (P1-4, P1-5)
    let db = init_database(None).expect("Failed to initialize SQLite database");
    let device_id = storage::db::get_or_create_device_id(&db).expect("Failed to get/create device_id");

    let initial_settings = if let Some(json_str) = storage::db::get_persisted_settings(&db) {
        let mut loaded = serde_json::from_str::<commands::settings_cmd::AppSettings>(&json_str)
            .unwrap_or_else(|e| {
                // 走到这里意味着用户已存的配置会被全量丢弃，必须留下痕迹
                log::warn!("Failed to parse persisted settings ({}), falling back to defaults", e);
                commands::settings_cmd::AppSettings::default_config()
            });
        if loaded.server_url == "ws://127.0.0.1:8080" {
            loaded.server_url = "wss://drop.yourdomain.com:58921".to_string();
        }
        loaded
    } else {
        commands::settings_cmd::AppSettings::default_config()
    };

    // 存量历史认领：把老库里没有归属的行归给当前账号。
    //
    // 必须在这里而不是 init_database 里——那时还没读到 settings。
    // 也必须先校验账号合法性：认领只触 NULL 行，一旦用空串之类的非法账号
    // 认领过，那些行既不会被重新认领、又永远不可能被看见（非法账号无法
    // 通过 validate_account_id 成为当前账号）。跳过则是安全的：NULL 行原样
    // 留着，推迟到某个合法账号启动时再认领。
    //
    // cmd_save_settings 里也有一次同样的认领（见那边的注释）。两处都做是因为
    // 这里可能因账号非法而跳过，而用户修正账号的地方正是设置面板——只在启动
    // 认领的话，他改对之后还得重启一次才能看见老历史。
    //
    // 早先这里写着「不在保存路径认领，那会把上一个账号的历史搬过来」——
    // 那个理由不成立：认领只触 NULL 行，已有归属的行根本搬不动，
    // claim_is_idempotent_and_does_not_resteal 就是钉这一点的。
    if commands::settings_cmd::validate_account_id(&initial_settings.account_id).is_ok() {
        match storage::db::claim_unowned_history(&db, &initial_settings.account_id) {
            Ok(0) => {}
            Ok(n) => log::info!("Claimed {} legacy history rows for account {}", n, initial_settings.account_id),
            Err(e) => log::warn!("Failed to claim legacy history rows: {}", e),
        }
    } else {
        log::warn!("Skipped claiming legacy history: current account id is invalid");
    }

    let config = ConnectionConfig {
        server_url: initial_settings.server_url.clone(),
        account_id: initial_settings.account_id.clone(),
        device_id: device_id.clone(),
        psk_secret: initial_settings.psk_secret.clone(),
        hostname: app_state::whoami_hostname(),
        os_type: std::env::consts::OS.to_string(),
        app_version: app_state::APP_VERSION.to_string(),
        allow_insecure_tls: initial_settings.allow_insecure_tls,
    };

    let config_actor = Arc::new(tokio::sync::RwLock::new(config));
    let reconnect_notify = Arc::new(tokio::sync::Notify::new());

    let (actor, outgoing_tx) = ConnectionActor::new(config_actor.clone(), reconnect_notify.clone());
    let app_state = AppState::new(
        Arc::new(Mutex::new(db)),
        outgoing_tx.clone(),
        device_id.clone(),
        initial_settings.clone(),
        config_actor.clone(),
        reconnect_notify.clone(),
    );

    let online_devices_ref = app_state.online_devices.clone();
    let pending_outbound_ref = app_state.pending_outbound.clone();
    let cache_manager_ref = app_state.cache_manager.clone();
    let settings_ref = app_state.settings.clone();
    let self_device_id = device_id.clone();

    // Map to track inbound offers waiting for transfer token
    let pending_inbound: Arc<Mutex<HashMap<String, (TransferOfferPayload, String)>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_inbound_ref = pending_inbound.clone();
    let start_minimized_on_launch = initial_settings.start_minimized;

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // 系统拉起的第二实例（带 --silent）不该抢焦点；用户双击图标的必须唤起
            if !core::startup::should_focus_second_instance(&args) {
                log::info!("Second instance is an autostart launch, keeping window state");
                return;
            }
            log::info!("Another instance attempted to start, focusing existing window");
            reveal_main_window(app);
        }))
        // 自启注册项固定带 --silent，只作为「系统拉起」的标记；
        // 是否显示窗口由 AppSettings.start_minimized 决定，与拉起方式无关。
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![core::startup::AUTOSTART_FLAG]),
        ))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let outgoing_tx_actor = outgoing_tx.clone();
            let app_handle_for_actor = app_handle.clone();
            let cache_manager_for_actor = cache_manager_ref.clone();
            let settings_for_actor = settings_ref.clone();

            // 3. Spawn Connection Actor in Tokio runtime
            tauri::async_runtime::spawn(async move {
                let app_handle = app_handle_for_actor;
                let cache_manager_ref = cache_manager_for_actor;
                let settings_ref = settings_for_actor;
                let (internal_tx, mut internal_rx) = mpsc::channel(128);
                tokio::spawn(actor.run(internal_tx, app_handle.clone()));

                while let Some(env) = internal_rx.recv().await {
                    match env.action {
                        ActionType::DEVICE_LIST_SYNC => {
                            if let Ok(payload) = serde_json::from_value::<DeviceListSyncPayload>(env.payload) {
                                let mut devs = online_devices_ref.lock().await;
                                *devs = payload.devices;
                                let _ = app_handle.emit("devices-updated", &*devs);
                            }
                        }
                        ActionType::DEVICE_ONLINE => {
                            if let Ok(payload) = serde_json::from_value::<DeviceOnlinePayload>(env.payload) {
                                let mut devs = online_devices_ref.lock().await;
                                if !devs.iter().any(|d| d.device_id == payload.device.device_id) {
                                    devs.push(payload.device);
                                    let _ = app_handle.emit("devices-updated", &*devs);
                                }
                            }
                        }
                        ActionType::DEVICE_OFFLINE => {
                            if let Ok(payload) = serde_json::from_value::<DeviceOfflinePayload>(env.payload) {
                                let mut devs = online_devices_ref.lock().await;
                                devs.retain(|d| d.device_id != payload.device_id);
                                let _ = app_handle.emit("devices-updated", &*devs);
                            }
                        }
                        ActionType::TRANSFER_OFFER => {
                            // Receiver received offer: send TRANSFER_ANSWER(accepted=true) back
                            if let Ok(mut offer) = serde_json::from_value::<TransferOfferPayload>(env.payload.clone()) {
                                log::info!("Received TRANSFER_OFFER from {} for session {}", env.from_device, offer.session_id);

                                // 加密 OFFER：必须**先解密元数据再做任何别的事**。
                                //
                                // 下面几步会 emit 给界面并把历史行落库，而加密 OFFER 里的
                                // relative_path / sha256 / preview_summary 都是空的——
                                // 不先还原就会展示并持久化空文件名，后续展示与修剪都带着它。
                                //
                                // 解密失败就地拒收，不建数据面：最常见的原因是两端 PSK 不一致，
                                // 让它在信令阶段以明确理由失败，比拖到数据面靠 tag 失败发现要好得多
                                // （那条路径给出的现象与真正的原因隔了好几层）。
                                let mut reject_reason: Option<String> = None;
                                if offer.encrypted {
                                    let (psk, acct) = {
                                        let s = settings_ref.lock().await;
                                        (s.psk_secret.clone(), s.account_id.clone())
                                    };
                                    match derive_key_for(&offer.session_id, &psk, &acct) {
                                        Ok(key) => {
                                            if let Err(e) = core::e2ee::open_offer_metadata(&mut offer, &key) {
                                                log::warn!("加密 OFFER 的元数据解密失败，拒收 session {}: {}", offer.session_id, e);
                                                reject_reason = Some("E2EE_KEY_MISMATCH".to_string());
                                            }
                                        }
                                        Err(e) => {
                                            log::warn!("接收端密钥派生失败，拒收 session {}: {}", offer.session_id, e);
                                            reject_reason = Some("E2EE_KEY_MISMATCH".to_string());
                                        }
                                    }
                                }

                                if let Some(reason) = reject_reason {
                                    let answer = TransferAnswerPayload {
                                        session_id: offer.session_id.clone(),
                                        accepted: false,
                                        reject_reason: Some(reason),
                                        resumed_items: Vec::new(),
                                        token: None,
                                    };
                                    let answer_env = protocol::ControlEnvelope {
                                        version: 1,
                                        trace_id: uuid::Uuid::new_v4().to_string(),
                                        action: ActionType::TRANSFER_ANSWER,
                                        from_device: self_device_id.clone(),
                                        to_device: Some(env.from_device.clone()),
                                        timestamp: core::connection_actor::current_time_ms(),
                                        payload: serde_json::to_value(&answer).unwrap(),
                                    };
                                    let _ = outgoing_tx_actor.send(answer_env).await;
                                    let _ = app_handle.emit(
                                        "e2ee-offer-rejected",
                                        "收到一份无法解密的传输请求，已拒收：两端密钥可能不一致",
                                    );
                                    continue;
                                }

                                let answer = TransferAnswerPayload {
                                    session_id: offer.session_id.clone(),
                                    accepted: true,
                                    reject_reason: None,
                                    resumed_items: Vec::new(),
                                    token: None,
                                };

                                let answer_env = protocol::ControlEnvelope {
                                    version: 1,
                                    trace_id: uuid::Uuid::new_v4().to_string(),
                                    action: ActionType::TRANSFER_ANSWER,
                                    from_device: self_device_id.clone(),
                                    to_device: Some(env.from_device.clone()),
                                    timestamp: core::connection_actor::current_time_ms(),
                                    payload: serde_json::to_value(&answer).unwrap(),
                                };

                                pending_inbound_ref.lock().await.insert(offer.session_id.clone(), (offer.clone(), env.from_device.clone()));
                                let _ = outgoing_tx_actor.send(answer_env).await;
                                let _ = app_handle.emit("transfer-offer-received", &offer);

                                // Persist incoming transfer in history
                                {
                                    // settings 锁先取先放，不与 db_conn 锁交叠
                                    let record_account_id = {
                                        settings_ref.lock().await.account_id.clone()
                                    };
                                    let state = app_handle.state::<AppState>();
                                    let conn = state.db_conn.lock().await;
                                    let _ = HistoryRepo::record_task(&conn, &record_account_id, &offer.session_id, &env.from_device, "RECEIVE", &offer, "TRANSFERRING");
                                }
                            }
                        }
                        ActionType::TRANSFER_ANSWER => {
                            if let Ok(answer) = serde_json::from_value::<TransferAnswerPayload>(env.payload) {
                                if answer.accepted {
                                    if let Some(token) = answer.token {
                                        // Check if we are the Sender
                                        let outbound_entry = {
                                            let mut pending = pending_outbound_ref.lock().await;
                                            pending.remove(&answer.session_id)
                                        };

                                        if let Some((offer, source)) = outbound_entry {
                                            log::info!("Starting Sender task for session {}", answer.session_id);
                                            let (active_server_url, allow_insecure_tls, psk, acct) = {
                                                let s = settings_ref.lock().await;
                                                (s.server_url.clone(), s.allow_insecure_tls,
                                                 s.psk_secret.clone(), s.account_id.clone())
                                            };
                                            // offer.encrypted 是发 OFFER 时就定下的，这里按它派生。
                                            // 派生失败不能静默降级成明文：接收端已经按加密形态
                                            // 准备好了，发明文过去只会让它整批 tag 失败。
                                            let sender_key = if offer.encrypted {
                                                match derive_key_for(&answer.session_id, &psk, &acct) {
                                                    Ok(k) => Some(k),
                                                    Err(e) => {
                                                        log::error!("发送端会话密钥派生失败，放弃本次传输: {}", e);
                                                        continue;
                                                    }
                                                }
                                            } else {
                                                None
                                            };
                                            tokio::spawn(TransferEngine::start_sender_task(
                                                active_server_url,
                                                allow_insecure_tls,
                                                answer.session_id.clone(),
                                                token.clone(),
                                                self_device_id.clone(),
                                                env.from_device.clone(),
                                                source,
                                                offer,
                                                sender_key,
                                                outgoing_tx_actor.clone(),
                                                app_handle.clone(),
                                            ));
                                        } else {
                                            // Check if we are the Receiver (token echo)
                                            let inbound_entry = {
                                                let mut pending = pending_inbound_ref.lock().await;
                                                pending.remove(&answer.session_id)
                                            };

                                            if let Some((offer, sender_device)) = inbound_entry {
                                                let (auto_inject, active_server_url, allow_insecure_tls, psk, acct) = {
                                                    let s = settings_ref.lock().await;
                                                    (s.auto_inject, s.server_url.clone(), s.allow_insecure_tls,
                                                     s.psk_secret.clone(), s.account_id.clone())
                                                };
                                                let receiver_key = if offer.encrypted {
                                                    match derive_key_for(&answer.session_id, &psk, &acct) {
                                                        Ok(k) => Some(k),
                                                        Err(e) => {
                                                            log::error!("接收端会话密钥派生失败，放弃本次接收: {}", e);
                                                            continue;
                                                        }
                                                    }
                                                } else {
                                                    None
                                                };
                                                log::info!("Starting Receiver task for session {}, auto_inject={}", answer.session_id, auto_inject);
                                                tokio::spawn(TransferEngine::start_receiver_task(
                                                    active_server_url,
                                                    allow_insecure_tls,
                                                    answer.session_id.clone(),
                                                    token,
                                                    sender_device,
                                                    self_device_id.clone(),
                                                    offer,
                                                    cache_manager_ref.clone(),
                                                    auto_inject,
                                                    receiver_key,
                                                    outgoing_tx_actor.clone(),
                                                    app_handle.clone(),
                                                ));
                                            }
                                        }
                                    }
                                } else {
                                    // 接收方明确拒收（accepted=false）。
                                    //
                                    // 改造前这里没有 else：服务端原样转发拒收的 ANSWER，
                                    // 而客户端只处理 accepted 的那一支，于是发送方的
                                    // pending_outbound 永不释放、卡片停在等待状态——
                                    // 一个不需要任何异常就能稳定复现的静默挂起。
                                    //
                                    // 它与本轮服务端「授权失败双向拒绝」是同一族问题的
                                    // 两半：那边堵的是服务端不肯授权，这边堵的是对端不愿接收。
                                    // 只修一半的话「不再有静默挂起」这个说法就只对一半。
                                    let sid = answer.session_id.clone();
                                    {
                                        let state = app_handle.state::<AppState>();
                                        state.pending_outbound.lock().await.remove(&sid);
                                    }
                                    pending_inbound_ref.lock().await.remove(&sid);

                                    let brief = {
                                        let state = app_handle.state::<AppState>();
                                        let conn = state.db_conn.lock().await;
                                        let brief = HistoryRepo::get_task_brief(&conn, &sid);
                                        let _ = HistoryRepo::update_task_status(
                                            &conn, &sid, "FAILED", Some("对方拒绝了本次传输"),
                                        );
                                        brief
                                    };

                                    let (direction, summary, total_size, data_type) = match brief {
                                        Some(b) => (
                                            b.direction,
                                            format!(
                                                "{} (对方拒绝了本次传输)",
                                                b.preview_summary.unwrap_or_else(|| "传输".into())
                                            ),
                                            b.total_size,
                                            b.data_type,
                                        ),
                                        None => (
                                            "SEND".to_string(),
                                            "传输 (对方拒绝了本次传输)".to_string(),
                                            0,
                                            "FILES".to_string(),
                                        ),
                                    };

                                    let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                                        session_id: sid,
                                        preview_summary: summary,
                                        total_size,
                                        transferred_size: 0,
                                        direction,
                                        progress: 0.0,
                                        status: "FAILED".to_string(),
                                        data_type,
                                    });

                                    crate::core::history_pruner::prune_and_notify(&app_handle).await;
                                }
                            }
                        }
                        ActionType::TRANSFER_FAILURE => {
                            // Peer-reported failure: flip the corresponding card to FAILED
                            let session_id = env
                                .payload
                                .get("session_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let err_msg = env
                                .payload
                                .get("error_message")
                                .or_else(|| env.payload.get("message"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());

                            if let Some(sid) = session_id {
                                // 失败是这个会话的终点，两侧待处理队列都必须释放。
                                //
                                // 改造前两个 map 都只在 TRANSFER_ANSWER 带 token 的那条
                                // 路径上被删除，失败路径完全不碰。本轮让服务端主动拒绝成为
                                // 常规路径（超限即拒），这条泄漏随之从罕见变高频。
                                //
                                // **两个都要删，不是只删 pending_outbound。** 并发超限的
                                // 拒绝是**双向**的：服务端拒掉 ANSWER 后既不授权也不转发，
                                // 于是 token 回显——pending_inbound 唯一的删除点——永远不会
                                // 发生，接收端那份含完整 items 的 offer 会永久留在 map 里。
                                //
                                // 无条件双删而不判断本机是发送方还是接收方：
                                // HashMap::remove 对不存在的 key 是 no-op，而那个判断本身
                                // 才是出错的来源。
                                {
                                    let state = app_handle.state::<AppState>();
                                    state.pending_outbound.lock().await.remove(&sid);
                                }
                                pending_inbound_ref.lock().await.remove(&sid);

                                let brief = {
                                    let state = app_handle.state::<AppState>();
                                    let conn = state.db_conn.lock().await;
                                    let brief = HistoryRepo::get_task_brief(&conn, &sid);
                                    let _ = HistoryRepo::update_task_status(&conn, &sid, "FAILED", err_msg.as_deref());
                                    brief
                                };

                                let (direction, mut summary, total_size, data_type) = match brief {
                                    Some(b) => (b.direction, b.preview_summary.unwrap_or_else(|| "传输".into()), b.total_size, b.data_type),
                                    None => ("SEND".to_string(), "传输".into(), 0, "FILES".to_string()),
                                };
                                if let Some(m) = &err_msg {
                                    summary = format!("{} ({})", summary, m);
                                }

                                let _ = app_handle.emit("transfer-progress", ActiveTransfer {
                                    session_id: sid,
                                    preview_summary: summary,
                                    total_size,
                                    transferred_size: 0,
                                    direction,
                                    progress: 0.0,
                                    status: "FAILED".to_string(),
                                    data_type,
                                });

                                // 对端报告失败同样是终态写入点，也要修剪历史。
                                // 这里已经在上面取 conn 的 { } 块之外——那个 guard
                                // 随块结束释放，此处再取锁才不会死锁。
                                crate::core::history_pruner::prune_and_notify(&app_handle).await;
                            }
                        }
                        ActionType::TRANSFER_COMPLETE => {
                            // Sender-side completion broadcast. The receiver's own data-plane
                            // verification path is authoritative for its card, so we only log
                            // here to avoid racing an in-flight SHA-256 verification.
                            if let Some(sid) = env.payload.get("session_id").and_then(|v| v.as_str()) {
                                log::info!("Peer reports TRANSFER_COMPLETE for session {}", sid);
                            }
                        }
                        ActionType::AUTH_RESPONSE => {
                            if let Ok(resp) = serde_json::from_value::<protocol::AuthResponsePayload>(env.payload) {
                                if !resp.success {
                                    let _ = app_handle.emit("auth-failed", resp.error_message);
                                } else {
                                    // 存下服务端下发的限额，供发送前本地预检与设置面板展示。
                                    //
                                    // resp.limits 为 None（老服务端不发这个字段）时**原样存 None**，
                                    // 不要 unwrap_or_default() —— 那会变成一组全 0，而 0 在这里
                                    // 表示「不限制」，等于在老服务端上把限额整组关掉。
                                    // None 的含义是「未知」，由发送端退回兜底常量处理。
                                    {
                                        let state = app_handle.state::<AppState>();
                                        let mut slot = state.server_limits.lock().await;
                                        *slot = resp.limits.clone();
                                    }
                                    let _ = app_handle.emit("server-limits-updated", &resp.limits);
                                    let _ = app_handle.emit("auth-success", ());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            });

            // 3.5 启动收敛：先把崩溃残留的非终态行复位，再修剪一次历史（需求 4）
            //
            // 顺序不能反：修剪按约束 A 跳过非终态行，不先复位的话，上次被杀掉的
            // 进程留下的 TRANSFERRING 死行会永久占住保留额度，谁也清不掉。
            let app_handle_for_prune = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                {
                    let state = app_handle_for_prune.state::<AppState>();
                    let conn = state.db_conn.lock().await;
                    match HistoryRepo::reset_stale_in_flight(&conn) {
                        Ok(0) => {}
                        Ok(n) => log::info!("Reset {} stale in-flight transfer(s) to FAILED on startup", n),
                        Err(e) => log::warn!("Failed to reset stale in-flight transfers: {}", e),
                    }
                } // conn guard 必须在此释放，下一行会重新取同一把锁
                crate::core::history_pruner::prune_and_notify(&app_handle_for_prune).await;
            });

            // 4. Background periodic cache sweep worker (P1-9)
            //
            // 结构是**先 sweep 后 sleep**，不能反过来。改造前用的是
            // tokio::time::interval，它的首跳立即返回，因此启动时会先扫一次；
            // 若改成先 sleep，这个「启动即扫」会静默消失——默认间隔下启动后一小时
            // 内不清理，而用户若把间隔设成 1440 分钟又每天关机，sleep 永远睡不满，
            // sweep 可能一次都不执行。
            //
            // 每轮都重新读设置，所以间隔与策略都是可变的；代价是改动间隔要等
            // 当前这一轮睡完才生效（重启则立即生效），这一点写在设置项说明里。
            let cache_sweep_mgr = cache_manager_ref.clone();
            let sweep_settings = settings_ref.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let (policy, interval) = {
                        let s = sweep_settings.lock().await;
                        (
                            core::retention::RetentionPolicy::from_settings(&s),
                            core::retention::effective_sweep_interval(&s),
                        )
                    }; // guard 必须在 sweep 之前释放：sweep 内部要取 db_conn 锁，
                       // 而这里持有的是 settings 锁，两者不同但没必要交叠持有

                    // Err 必须留痕：后台清理的唯一职责就是防磁盘占满，若持续失败
                    // 而外部零信号，故障呈现形态恰是这个功能本身要防的那件事。
                    // 与 prune_and_notify 的处理对齐。
                    match cache_sweep_mgr.sweep(policy).await {
                        Ok(purged) if purged > 0 => {
                            log::info!("Cache cleaner purged {} expired or LRU entries", purged)
                        }
                        Ok(_) => {}
                        Err(e) => log::warn!("Cache sweep failed: {}", e),
                    }

                    tokio::time::sleep(interval).await;
                }
            });

            // 5. Start platform clipboard listener (P1-9)
            #[cfg(target_os = "macos")]
            {
                let (clip_tx, mut clip_rx) = mpsc::channel(32);
                crate::platform::start_clipboard_listener(clip_tx);
                let app_handle_clip = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    while let Some(event) = clip_rx.recv().await {
                        if let Some(text) = event.text_content {
                            log::info!("Clipboard changed (text len: {})", text.len());
                            let _ = app_handle_clip.emit("clipboard-updated", text);
                        }
                    }
                });
            }

            // 6. Build tray menu
            let quit_item = MenuItem::with_id(app, "quit", "退出 瞬贴 (UniDrop)", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "偏好设置...", true, None::<&str>)?;
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &settings_item, &quit_item])?;

            // 7. Setup system tray icon and click handling
            // Do NOT re-declare `trayIcon` in tauri.conf.json: Tauri would auto-create a
            // second tray (icon but no menu/handlers) alongside this one, shifting the
            // clickable area off the visible icon on Windows.
            let mut tray_builder = TrayIconBuilder::new()
                .menu(&tray_menu)
                .tooltip("瞬贴 (UniDrop) - 跨平台剪贴板与文件分发");
            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }
            let _tray = tray_builder
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        reveal_main_window(app);
                    }
                    "settings" => {
                        reveal_main_window(app);
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.emit("open-settings", ());
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            // 最小化到任务栏时窗口仍是 visible，此时应还原而不是隐藏
                            if win.is_visible().unwrap_or(false)
                                && !win.is_minimized().unwrap_or(false)
                            {
                                let _ = win.hide();
                            } else {
                                reveal_main_window(app);
                            }
                        }
                    }
                })
                .build(app)?;

            // 8. 冷启动显隐：只取决于用户配置，手动启动与开机自启一视同仁。
            // 窗口在 tauri.conf.json 里初始 visible:false，避免 minimized 时的闪窗。
            if core::startup::should_show_on_launch(start_minimized_on_launch) {
                reveal_main_window(app.handle());
            } else {
                log::info!("Starting minimized to tray per user setting");
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::cmd_get_online_devices,
            commands::cmd_get_self_info,
            commands::cmd_inject_files,
            commands::cmd_send_files,
            commands::cmd_get_settings,
            commands::cmd_save_settings,
            commands::cmd_get_autostart,
            commands::cmd_set_autostart,
            commands::cmd_hide_window,
            commands::cmd_read_clipboard_preview,
            commands::cmd_send_clipboard,
            commands::cmd_inject_session,
            commands::cmd_list_history,
            commands::cmd_save_transfer_as,
            commands::cmd_reveal_session,
            commands::cmd_get_server_limits,
        ])
        .build(tauri::generate_context!())
        .expect("error while building UniDrop application")
        .run(|app_handle, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                reveal_main_window(app_handle);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        });
}
