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

/// 上一次**生效**的左键动作发生的时刻，用于双击去抖。
///
/// 与 `tray_placement_macos` 里的 `CURRENT_POLICY` 同样是模块内静态量而不是
/// `AppState` 的字段：它只服务于托盘回调这一处，进不了前端也进不了持久化，
/// 挂到共享状态上只会让那个结构体多背一个跟它无关的概念。
///
/// 用 `std::sync::Mutex` 而非本文件里 `use` 进来的 `tokio::sync::Mutex`：
/// 托盘回调是同步的（`TrayIconEvent::send` 直接调 handler，
/// tray-icon-0.24.2/src/lib.rs:693-699），里面 await 不了。
static LAST_LEFT_CLICK: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

/// 构建菜单栏 / 任务栏托盘图标。
///
/// **注意：macOS 26 (Tahoe) 起，菜单栏项由控制中心进程托管并有准入控制。**
/// 状态项被拒时不会报任何错误——创建成功、菜单弹得开、回调照常触发，只是
/// 控制中心不给它菜单栏位置，肉眼看就是「图标没出现」。这套哑故障的检测在
/// [`platform::tray_placement_macos`]，其模块头记着被拒的完整机制。
///
/// 一句话版本：控制中心按**责任进程**归属菜单项，并且是**连坐**判定——本应用的
/// bundle id 只要出现在任一 `isAllowed=false` 记录的 `menuItemLocations` 里就被挡，
/// 自己那条记录是允许也没用。开发期从 IDE 集成终端直接跑可执行文件，责任进程
/// 就是那个 IDE，本应用于是挂到它名下；**构建后一律 `open` 启动 .app**。
///
/// 排查时**不要再往创建时机上找原因**：setup 内创建、推迟到 `RunEvent::Ready`、
/// 再叠加数百毫秒到数秒的延迟、乃至创建后强制重建，五种形态在生产构建下
/// 实测坐标全都相同——时机不是变量（见 tauri-apps/tauri#13770 与
/// `tray_placement_macos` 的模块头）。与托盘的建法（tray-icon 还是原生 AppKit）
/// 同样无关：同 bundle id 的 /tmp 裸探针照样被挡，因为挡它的是责任进程那条记录。
fn build_tray<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<tauri::tray::TrayIcon<R>> {
    let quit_item = MenuItem::with_id(app, "quit", "退出 瞬贴 (UniDrop)", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "偏好设置...", true, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let tray_menu = Menu::with_items(app, &[&show_item, &settings_item, &quit_item])?;

    // Do NOT re-declare `trayIcon` in tauri.conf.json: Tauri would auto-create a
    // second tray (icon but no menu/handlers) alongside this one, shifting the
    // clickable area off the visible icon on Windows.
    let mut tray_builder = TrayIconBuilder::new()
        .menu(&tray_menu)
        .tooltip("瞬贴 (UniDrop) - 跨平台剪贴板与文件分发");

    #[cfg(target_os = "macos")]
    {
        // 菜单栏图标必须是模板图（纯黑 + alpha，形状只由 alpha 表达），
        // 由系统按深浅色自动反色。若沿用 default_window_icon()——也就是彩色的
        // icons/32x32.png——深色主体会在深色菜单栏上与背景融为一体。
        let tray_icon =
            tauri::image::Image::from_bytes(include_bytes!("../icons/tray-macos.png"))?;
        tray_builder = tray_builder
            .icon(tray_icon)
            .icon_as_template(true)
            // 默认是 true，左键会被系统拿去弹菜单，把下面 on_tray_icon_event
            // 里的左键分支彻底挡死（那段逻辑在 macOS 上从未生效过）。
            // 关掉之后：左键切换显隐、右键弹菜单，与 Windows 行为一致。
            .show_menu_on_left_click(false);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray_builder = tray_builder.icon(icon.clone());
    }

    let tray_built = tray_builder
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
            let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            else {
                return;
            };

            // 这条链路此前一行日志都没有，「点了没反应」时无从分辨事件压根没来，
            // 还是来了但判据走错——本次故障就卡在这个盲点上排查了很久。留着它。
            // 回调是被 `TrayIconEvent::send` 同步调用的，直接跑在 AppKit 的鼠标
            // 事件处理里（macOS 上就是 `mouseUp:`），所以这里只能做轻量的事。
            log::info!(
                "Tray icon clicked: button={:?} state={:?}",
                button,
                button_state
            );

            // 只认抬起。按下与抬起各会发一次 Click（macOS 见 tray-icon-0.24.2/
            // src/platform_impl/macos/mod.rs:336-365，Windows 见 windows/mod.rs:409-449，
            // Tauri 两种都原样转发、不过滤，tauri-2.11.5/src/tray/mod.rs:31-36），
            // 两种都接就是一次点击触发两次动作，正好互相抵消。
            if button != MouseButton::Left || button_state != MouseButtonState::Up {
                return;
            }

            handle_tray_left_click(tray.app_handle());
        })
        .build(app)?;

    // macOS 26 起，菜单栏项由「控制中心」进程托管，应用这边只留一个坐标不可信的
    // 影子窗口。托管窗口以状态项的 `autosaveName` 为名，未设则是匿名的 `Item-0`。
    //
    // **设它不是修复手段，别指望补上就能让图标出现**——实测补设之后控制中心日志
    // 确认名字已生效，图标依旧不出现（被挡的真正原因见 build_tray 的文档）。
    // 它的作用是给状态项一个稳定身份，让 layer-25 托管窗口有确定的名字可认，
    // 这是运行期放置探测主判据的前提（见 platform::tray_placement_macos）。
    // tray-icon 0.24.2 自己不设这个属性（全仓库无 setAutosaveName 调用），
    // 所以必须在这里补，否则探测只能退到几何副判据上。
    //
    // **改名字要两边一起改**，否则自检会把正常状态误判成被拒。
    //
    // 在 setup（主线程）里调 with_inner_tray_icon 不会自锁：它内部是
    // run_on_main_thread + 阻塞收回执，而 tauri-runtime-wry 的 send_user_message
    // 对「调用方已在主线程」做了特判，直接同步就地执行（lib.rs:239）。
    // 此时事件循环还没 run 起来，靠的正是这条特判——别改成先 spawn 再等。
    #[cfg(target_os = "macos")]
    {
        let app_handle_for_tray = app.clone();
        if let Err(e) = tray_built.with_inner_tray_icon(move |inner| {
            if let Some(item) = inner.ns_status_item() {
                item.setAutosaveName(Some(&objc2_foundation_v06::NSString::from_str(
                    platform::tray_placement_macos::TRAY_AUTOSAVE_NAME,
                )));

                // 诊断：把状态项自身的状态打出来。
                // macOS 26 下系统侧日志对「能显示」和「不能显示」的应用完全一致
                // （实测对照过一个同机正常工作的原生应用，托管序列逐行相同），
                // 所以差异只可能在我们送过去的内容上——图标是否真的设上、按钮多宽、
                // 状态项是否 visible。这几个值只能在本进程内读到。
                if let Some(mtm) = objc2_v06::MainThreadMarker::new() {
                    let button = item.button(mtm);
                    let (has_image, image_size, title, button_frame) = match &button {
                        Some(b) => {
                            let img = b.image();
                            (
                                img.is_some(),
                                img.as_ref().map(|i| i.size()),
                                b.title().to_string(),
                                Some(b.frame()),
                            )
                        }
                        None => (false, None, String::new(), None),
                    };
                    log::info!(
                        "Status item diagnostics: visible={} length={} has_button={} has_image={} \
                         image_size={:?} title={:?} button_frame={:?}",
                        item.isVisible(),
                        item.length(),
                        button.is_some(),
                        has_image,
                        image_size,
                        title,
                        button_frame,
                    );
                }

                // 安装 macOS 原生 target-action 点击处理器（左键唤起窗口，右键呼出菜单）
                let app_handle = app_handle_for_tray.clone();
                platform::tray_click_macos::setup_tray_macos(&item, move || {
                    handle_tray_left_click(&app_handle);
                });
            }
        }) {
            // 设不上不致命：图标可能仍然显示（系统会退回匿名身份），只是位置不再被
            // 记住、自检也会失去依据。所以留痕但不阻断启动。
            log::warn!("Failed to set up macOS tray native click handler or autosave name: {}", e);
        }
    }

    Ok(tray_built)
}

/// 唤起主窗口的唯一入口。
///
/// 「隐藏到托盘」与「最小化到任务栏」是两种不同的形态：后者窗口仍是 visible，
/// 只调 show() + set_focus() 在 Windows 上无法还原，必须先 unminimize()。
///
/// 三步顺序在 macOS 上同样是必须品，而且自从 Dock 图标被藏起来（Accessory，
/// 见 `run()` 里的 set_activation_policy）之后更要命：tao 的 `Window::set_focus`
/// 有一道前置守卫 `if !is_minimized && is_visible`
/// （tao-0.35.3/src/platform_impl/macos/window.rs:677），不满足就整个空操作，
/// 连里面那句 `activateIgnoringOtherApps: YES`（util/async.rs:234）都不会执行——
/// 处理托盘图标左键点击（唤出或收起主窗口，包含双击去抖与焦点判定）。
///
/// 供 Windows/Linux 的 on_tray_icon_event 以及 macOS 原生 target-action 回调共享调用。
pub(crate) fn handle_tray_left_click<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    // 双击去抖：一次双击会发两次抬起，不拦的话变成「唤起 + 收起」，
    // 窗口闪一下就没了——旧代码双击打不开窗口就是这么来的。
    // 被吞掉的那次不刷新时间戳，理由见 is_duplicate_left_click。
    let now = std::time::Instant::now();
    let duplicate = {
        let mut last = LAST_LEFT_CLICK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if crate::core::tray_click::is_duplicate_left_click(
            last.map(|t| now.saturating_duration_since(t)),
        ) {
            true
        } else {
            *last = Some(now);
            false
        }
    };
    if duplicate {
        log::debug!("Ignoring the second click of a tray double-click");
        return;
    }

    if let Some(win) = app.get_webview_window("main") {
        if crate::core::tray_click::should_hide_on_tray_click(
            win.is_visible().unwrap_or(false),
            win.is_minimized().unwrap_or(false),
            win.is_focused().unwrap_or(false),
        ) {
            let _ = win.hide();
        } else {
            reveal_main_window(app);
        }
    }
}

/// 唤起主窗口的唯一入口。
///
/// 「隐藏到托盘」与「最小化到任务栏」是两种不同的形态：后者窗口仍是 visible，
/// 只调 show() + set_focus() 在 Windows 上无法还原，必须先 unminimize()。
///
/// 三步顺序在 macOS 上同样是必须品，而且自从 Dock 图标被藏起来（Accessory，
/// 见 `run()` 里的 set_activation_policy）之后更要命：tao 的 `Window::set_focus`
/// 有一道前置守卫 `if !is_minimized && is_visible`
/// （tao-0.35.3/src/platform_impl/macos/window.rs:677），不满足就整个空操作，
/// 连里面那句 `activateIgnoringOtherApps: YES`（util/async.rs:234）都不会执行——
/// 而没有 Dock 图标时，那一句是应用唯一还能把自己拉到前台的手段。
/// 不要把 show() 挪到 set_focus() 后面，也不要以「反正已经 visible 了」删掉它。
///
/// 托盘菜单、托盘点击、第二实例、macOS Reopen、macOS 通知点击全部走这里，
/// 避免各处行为不一致。pub(crate) 是为了让 platform::notification_macos
/// 的 delegate 回调也能复用它，而不是另造一条唤起路径。
pub(crate) fn reveal_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
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
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 启动期日志缓冲。
    //
    // tauri-plugin-log 要到 `tauri::Builder::run()` 才把 logger 挂上去，而下面的
    // 建库、设置反序列化、存量历史认领全发生在那之前——直接 log! 会写进一个
    // 还没人接管的 logger，整条丢掉。其中就包括「设置解析失败、用户配置被全量
    // 重置」那一条，恰恰是本文件里后果最严重、也最需要事后排查的一条。
    //
    // 所以这一段先攒着，进 `.setup()` 后原样重放。用缓冲而不是把 init_database /
    // AppState::new 整体搬进 .setup()，是因为后者要动 .manage() 的构造顺序，
    // 风险与收益不成比例。
    let mut startup_log: Vec<(log::Level, String)> = Vec::new();

    // 1. Initialize SQLite local database and persistent identity (P1-4, P1-5)
    let db = init_database(None).expect("Failed to initialize SQLite database");
    let device_id = storage::db::get_or_create_device_id(&db).expect("Failed to get/create device_id");

    let initial_settings = if let Some(json_str) = storage::db::get_persisted_settings(&db) {
        let mut loaded = serde_json::from_str::<commands::settings_cmd::AppSettings>(&json_str)
            .unwrap_or_else(|e| {
                // 走到这里意味着用户已存的配置会被全量丢弃，必须留下痕迹
                startup_log.push((
                    log::Level::Warn,
                    format!("Failed to parse persisted settings ({}), falling back to defaults", e),
                ));
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
            Ok(n) => startup_log.push((
                log::Level::Info,
                format!("Claimed {} legacy history rows for account {}", n, initial_settings.account_id),
            )),
            Err(e) => startup_log.push((
                log::Level::Warn,
                format!("Failed to claim legacy history rows: {}", e),
            )),
        }
    } else {
        startup_log.push((
            log::Level::Warn,
            "Skipped claiming legacy history: current account id is invalid".to_string(),
        ));
    }

    let config = ConnectionConfig {
        server_url: initial_settings.server_url.clone(),
        account_id: initial_settings.account_id.clone(),
        device_id: device_id.clone(),
        psk_secret: initial_settings.psk_secret.clone(),
        hostname: app_state::whoami_hostname(),
        os_type: std::env::consts::OS.to_string(),
        app_version: app_state::APP_VERSION.to_string(),
        // 指纹解析失败不能让应用起不来：回落到最安全的一档，
        // 用户进设置页改指纹时会再看到一次精确的报错。
        tls_trust: initial_settings.tls_trust_config().unwrap_or_else(|e| {
            startup_log.push((
                log::Level::Warn,
                format!("证书指纹配置无效（{e}），本次启动按「仅信任公共 CA」处理"),
            ));
            core::tls_trust::TlsTrustConfig::public_ca()
        }),
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

    // 拆成 `let app = ...build()` 再单独 `app.run()`，而不是一路链下去：
    // 中间要插一次 `set_activation_policy`，且**必须插在 run() 之前**，理由见那里。
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut app = tauri::Builder::default()
        // 日志插件放在最前：它之后注册的插件若有启动期日志，才有人接得住。
        //
        // 级别压到 Info；tungstenite 两个 crate 单独压到 Warn——它们在 Debug/Info
        // 级别会把每一帧 WebSocket 都打出来，落盘后正常传一次文件就能刷掉几 MB，
        // 把真正要看的那几行冲走。
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("tokio_tungstenite", log::LevelFilter::Warn)
                .level_for("tungstenite", log::LevelFilter::Warn)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir { file_name: Some("unidrop".into()) },
                ))
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ))
                // 上限与 KeepOne 是刻意的：日志和缓存争同一块磁盘，而本仓库对
                // 缓存已经立了「按量按时清理」的规矩（见 cache_max_size_mb），
                // 日志不该是那条规矩之外的例外。最坏占用 2 个文件 4 MB。
                .max_file_size(2 * 1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .build(),
        )
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
            // logger 此时才真正就绪，把启动期攒下的日志原样放出来。
            for (level, msg) in startup_log.drain(..) {
                log::log!(level, "{}", msg);
            }

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
                                            let (active_server_url, tls_trust, psk, acct) = {
                                                let s = settings_ref.lock().await;
                                                // 数据面必须与控制面用同一套信任策略。
                                                // 解析失败时同样回落到最严的一档——绝不能
                                                // 因为指纹写错就让这条连接比控制面更宽松。
                                                (s.server_url.clone(),
                                                 s.tls_trust_config().unwrap_or_default(),
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
                                                tls_trust,
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
                                                let (auto_inject, active_server_url, tls_trust, psk, acct) = {
                                                    let s = settings_ref.lock().await;
                                                    (s.auto_inject, s.server_url.clone(),
                                                     s.tls_trust_config().unwrap_or_default(),
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
                                                    tls_trust,
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

            // 6/7. 托盘图标（全平台一致，在此创建）。
            // 返回的 TrayIcon 可以就地丢弃：TrayIconBuilder::build 内部已经把它
            // 注册进 app 的资源表与 manager.tray.icons，引用计数不会归零。
            // 丢弃它与「图标不可见」无关，别再往这上面怀疑。
            build_tray(app.handle())?;

            // 7.5 macOS 通知初始化：装 delegate 并请求授权。
            // 必须在这里做一次，而不是等到第一条通知才初始化——授权弹窗应在启动期
            // 出现，且 delegate 要先于任何通知到达就位，否则点击回调会丢。
            // 非 bundle 环境（如 tauri dev 的裸二进制）由函数内部自行跳过。
            #[cfg(target_os = "macos")]
            crate::platform::notification_macos::init_notifications(app.handle());

            // 7.6 菜单栏放置看门狗：托盘建好之后，确认系统是否真给了它位置。
            // macOS 26 会静默拒绝（状态项创建成功但拿不到菜单栏位置），
            // 不探测的话这就是个彻底的哑故障——应用在后台跑着，用户既看不到图标，
            // 也收不到任何解释。探测本身是延迟异步的，不阻塞 setup。
            #[cfg(target_os = "macos")]
            crate::platform::tray_placement_macos::spawn_placement_watchdog(app.handle());

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
            commands::cmd_get_tray_placement,
            commands::cmd_dismiss_tray_guidance,
            commands::cmd_open_menu_bar_settings,
            commands::cmd_get_notification_auth_status,
            commands::cmd_open_notification_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building UniDrop application");

    // macOS：启动即 Accessory —— 只在菜单栏出现，Dock 里不要图标。
    //
    // **必须在 run() 之前，不能挪进 setup()。** 两条硬约束：
    //
    // 1. setup 闭包不是在 build() 里跑的，而是在 `RuntimeRunEvent::Ready` 里跑的
    //    （tauri-2.11.5/src/app.rs:1423）。那时 tao 的 applicationDidFinishLaunching
    //    早已执行完 `apply_activation_policy`（tao-0.35.3 app_state.rs:285），
    //    按 aux state 的默认值 Regular 落过一次——每个用户都会看见 Dock 图标
    //    闪一下再消失。
    // 2. 只有 run() 之前 `App::runtime` 还是 Some，此时
    //    `App::set_activation_policy`（app.rs:1286）走的是 tao 的
    //    `EventLoopExtMacOS::set_activation_policy`，只写 aux state、由 tao 在
    //    didFinishLaunching 统一 apply，零闪烁。run() 之后就只剩运行时那条路了。
    //
    // 与 `src-tauri/Info.plist` 里的 LSUIElement 是**互补而非冗余**，两边都要留：
    // LSUIElement 只管 exec → didFinishLaunching 那几百毫秒（且只在 .app bundle 里
    // 有效），这一行管其余全部时间，并且是 `tauri dev`（裸二进制、没有 Info.plist）
    // 下唯一生效的手段。反过来单靠 LSUIElement 也不行——上面第 1 条那次 apply
    // 是无条件的，会拿默认的 Regular 把它直接覆盖掉。
    //
    // dev 与 release 行为刻意保持一致，不加 cfg(debug_assertions) 分支。
    // 被系统拒绝时切回 Regular 放出 Dock 图标当兜底，那是看门狗的职责，
    // 见 platform::tray_placement_macos::desired_activation_policy。
    #[cfg(target_os = "macos")]
    app.set_activation_policy(platform::tray_placement_macos::launch_activation_policy());

    app.run(|app_handle, event| {
        // macOS「再次打开一个已在运行的应用」→ 唤回窗口。
        //
        // 触发面比字面上的「点 Dock 图标」宽得多：tao 注册的是
        // applicationShouldHandleReopen:hasVisibleWindows:
        // （tao-0.35.3/src/platform_impl/macos/app_delegate.rs:79），
        // `open -a`、访达双击 .app、Spotlight 打开已运行的实例都会走这里。
        // 本应用常态是 Accessory（Dock 里没有图标，见上面那次 set_activation_policy），
        // 「点 Dock 图标」这一条确实不会再发生了，但**其余几条恰恰是此时最自然的
        // 唤回手势**——不要因为「反正没有 Dock 图标了」就把这个分支删掉。
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            reveal_main_window(app_handle);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app_handle, event);
    });
}
