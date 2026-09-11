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

pub fn run() {
    env_logger::init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 1. Initialize SQLite local database and persistent identity (P1-4, P1-5)
    let db = init_database(None).expect("Failed to initialize SQLite database");
    let device_id = storage::db::get_or_create_device_id(&db).expect("Failed to get/create device_id");

    let initial_settings = if let Some(json_str) = storage::db::get_persisted_settings(&db) {
        let mut loaded = serde_json::from_str::<commands::settings_cmd::AppSettings>(&json_str).unwrap_or_else(|_| {
            commands::settings_cmd::AppSettings {
                server_url: "wss://drop.yourdomain.com:58921".to_string(),
                account_id: "default_user".to_string(),
                psk_secret: "dev-insecure-psk-secret".to_string(),
                auto_inject: false,
                rate_limit_mb: 10,
            }
        });
        if loaded.server_url == "ws://127.0.0.1:8080" {
            loaded.server_url = "wss://drop.yourdomain.com:58921".to_string();
        }
        loaded
    } else {
        commands::settings_cmd::AppSettings {
            server_url: "wss://drop.yourdomain.com:58921".to_string(),
            account_id: "default_user".to_string(),
            psk_secret: "dev-insecure-psk-secret".to_string(),
            auto_inject: false,
            rate_limit_mb: 10,
        }
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

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log::info!("Another instance attempted to start, focusing existing window");
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
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
            let _tray = TrayIconBuilder::new()
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "settings" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
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
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
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
            commands::cmd_hide_window,
            commands::cmd_start_drag,
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
                if let Some(win) = app_handle.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        });
}
