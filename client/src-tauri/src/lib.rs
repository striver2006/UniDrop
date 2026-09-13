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

    let config = ConnectionConfig {
        server_url: initial_settings.server_url.clone(),
        account_id: initial_settings.account_id.clone(),
        device_id: device_id.clone(),
        psk_secret: initial_settings.psk_secret.clone(),
        hostname: app_state::whoami_hostname(),
        os_type: std::env::consts::OS.to_string(),
        app_version: "0.1.0".to_string(),
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
                tokio::spawn(actor.run(internal_tx));

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
                            if let Ok(offer) = serde_json::from_value::<TransferOfferPayload>(env.payload.clone()) {
                                log::info!("Received TRANSFER_OFFER from {} for session {}", env.from_device, offer.session_id);
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
                                    let state = app_handle.state::<AppState>();
                                    let conn = state.db_conn.lock().await;
                                    let _ = HistoryRepo::record_task(&conn, &offer.session_id, &env.from_device, "RECEIVE", &offer, "TRANSFERRING");
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
                                            let active_server_url = {
                                                let s = settings_ref.lock().await;
                                                s.server_url.clone()
                                            };
                                            tokio::spawn(TransferEngine::start_sender_task(
                                                active_server_url,
                                                answer.session_id.clone(),
                                                token.clone(),
                                                self_device_id.clone(),
                                                env.from_device.clone(),
                                                source,
                                                offer,
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
                                                let (auto_inject, active_server_url) = {
                                                    let s = settings_ref.lock().await;
                                                    (s.auto_inject, s.server_url.clone())
                                                };
                                                log::info!("Starting Receiver task for session {}, auto_inject={}", answer.session_id, auto_inject);
                                                tokio::spawn(TransferEngine::start_receiver_task(
                                                    active_server_url,
                                                    answer.session_id.clone(),
                                                    token,
                                                    sender_device,
                                                    self_device_id.clone(),
                                                    offer,
                                                    cache_manager_ref.clone(),
                                                    auto_inject,
                                                    outgoing_tx_actor.clone(),
                                                    app_handle.clone(),
                                                ));
                                            }
                                        }
                                    }
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
            let cache_sweep_mgr = cache_manager_ref.clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                loop {
                    interval.tick().await;
                    if let Ok(purged) = cache_sweep_mgr.sweep_expired_and_lru().await {
                        if purged > 0 {
                            log::info!("Cache cleaner purged {} expired or LRU entries", purged);
                        }
                    }
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
