pub mod app_state;
pub mod commands;
pub mod core;
pub mod platform;
pub mod protocol;
pub mod storage;

use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use tokio::sync::{mpsc, Mutex};

use app_state::AppState;
use core::connection_actor::{ConnectionActor, ConnectionConfig};
use protocol::{ActionType, DeviceListSyncPayload, DeviceOfflinePayload, DeviceOnlinePayload};
use storage::db::init_database;

pub fn run() {
    env_logger::init();

    // 1. Initialize SQLite local database
    let db = init_database(None).expect("Failed to initialize SQLite database");

    // 2. Initialize connection actor channels
    let config = ConnectionConfig {
        server_url: "ws://127.0.0.1:8080".to_string(),
        account_id: "default_user".to_string(),
        device_id: uuid::Uuid::new_v4().to_string(),
        psk_secret: "YOUR_SHARED_SECRET_KEY_HERE".to_string(),
        hostname: whoami_hostname(),
        os_type: std::env::consts::OS.to_string(),
        app_version: "0.1.0".to_string(),
    };

    let (actor, outgoing_tx, _incoming_rx) = ConnectionActor::new(config);
    let app_state = AppState::new(Arc::new(Mutex::new(db)), outgoing_tx);
    let online_devices_ref = app_state.online_devices.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .manage(app_state)
        .setup(|app| {
            let app_handle = app.handle().clone();

            // 3. Spawn Connection Actor in Tokio runtime
            tauri::async_runtime::spawn(async move {
                let (internal_tx, mut internal_rx) = mpsc::channel(128);
                tokio::spawn(actor.run(internal_tx));

                while let Some(env) = internal_rx.recv().await {
                    match env.action {
                        ActionType::DEVICE_LIST_SYNC => {
                            if let Ok(payload) = serde_json::from_value::<DeviceListSyncPayload>(env.payload) {
                                let mut devs = online_devices_ref.lock().await;
                                *devs = payload.devices;
                                let _ = app_handle.emit("devices-updated", ());
                            }
                        }
                        ActionType::DEVICE_ONLINE => {
                            if let Ok(payload) = serde_json::from_value::<DeviceOnlinePayload>(env.payload) {
                                let mut devs = online_devices_ref.lock().await;
                                if !devs.iter().any(|d| d.device_id == payload.device.device_id) {
                                    devs.push(payload.device);
                                    let _ = app_handle.emit("devices-updated", ());
                                }
                            }
                        }
                        ActionType::DEVICE_OFFLINE => {
                            if let Ok(payload) = serde_json::from_value::<DeviceOfflinePayload>(env.payload) {
                                let mut devs = online_devices_ref.lock().await;
                                devs.retain(|d| d.device_id != payload.device_id);
                                let _ = app_handle.emit("devices-updated", ());
                            }
                        }
                        ActionType::TRANSFER_OFFER => {
                            let _ = app_handle.emit("transfer-offer-received", env.payload);
                        }
                        _ => {}
                    }
                }
            });

            // 4. Build tray menu
            let quit_item = MenuItem::with_id(app, "quit", "退出 UniDrop", true, None::<&str>)?;
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            // 5. Setup system tray icon and click handling
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

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::cmd_get_online_devices,
            commands::cmd_get_self_info,
            commands::cmd_inject_files,
            commands::cmd_send_files,
            commands::cmd_get_settings,
            commands::cmd_save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running UniDrop application");
}

fn whoami_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "localhost".to_string())
}
