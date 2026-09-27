//! UniClip 移动端 FFI 层（iOS / Android / HarmonyOS 共享一份代码）。
//!
//! ## 协议约定
//!
//! - **命令**：`unidrop_invoke(cmd_json)` 统一入口。JSON in / JSON out，
//!   `{"cmd": "...", ...}`。命令**全部异步执行**：invoke 立即返回
//!   `{"ok":true,"call_id":"..."}`，真正结果（成功或失败）经事件回调以
//!   `invoke-result` 事件投递 `{"call_id","ok","data"|"error"}`。
//!   这样 Dart 侧的 FFI 调用永不阻塞 UI isolate，也无需 block_on
//!   （在任意宿主线程上 block_on tokio runtime 都是有风险的形态）。
//! - **事件**：`unidrop_set_event_callback` 注册 C 回调；事件为单条 JSON
//!   字符串 `{"event":"名字","payload":...}`，事件名与桌面端 Tauri 事件
//!   逐字段一致（两端共用同一套前端事件协议）。
//! - **内存**：Rust 返回的字符串由调用方经 `unidrop_free_string` 释放；
//!   入参字符串归调用方所有，Rust 只读不复用。
//!
//! ## 加载方式
//!
//! 编译为 `libunidrop_mobile.so`（Android / 鸿蒙）与
//! `libunidrop_mobile.a` + modulemap（iOS）。Dart 侧 `DynamicLibrary.open`
//! / `DynamicLibrary.process` 加载；鸿蒙 NDK 加载 .so 为官方支持路径。

use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use unidrop_core::app_state::{AppState, HostEnv};
use unidrop_core::host::{HostBridge, NetworkKind, ReceivePolicy};
use unidrop_core::protocol::OnlineDevice;
use unidrop_core::settings::AppSettings;
use unidrop_core::signal_router::SignalRouter;
use unidrop_core::storage::{self, HistoryRepo};

/// 事件回调类型：user_data 原样回传，event_json 为 NUL 结尾的 UTF-8 JSON。
pub type EventCallback = extern "C" fn(user_data: usize, event_json: *const c_char);

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// 极简 stderr logger：真机实测的 Rust 日志出口。
///
/// Android：debuggable 应用的 stderr 由 logcat 捕获（`adb logcat *:S flutter`）；
/// iOS：stderr 经 os_log 转发，Console.app / devicectl 可见。
/// 不引 env_logger / android_logger——移动端没有环境变量可配，
/// 固定 Info 级、stderr 单目的地足够排障用。
struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[unidrop {}] {}", record.level(), record.args());
            // 排障双写：ohos 的 stderr 不进 hilog，落到文件由 hdc 拉取。
            if let Some(path) = DEBUG_LOG_PATH.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(f, "[{}] {}", record.level(), record.args());
                }
            }
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

/// 排障日志文件路径（start 时指向 cache_dir 下的 unidrop-debug.log）。
static DEBUG_LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(target_os = "android")]
fn install_logger() {
    // Android：stderr 在 release 应用里被直接丢弃，日志必须进 logcat。
    // tag 固定 unidrop：`adb logcat -s unidrop` 即可只看本应用核心日志。
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("unidrop")
            .with_max_level(log::LevelFilter::Info),
    );
}

#[cfg(not(target_os = "android"))]
fn install_logger() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}
static HOST: OnceLock<Host> = OnceLock::new();
static EVENT_SINK: Mutex<Option<(EventCallback, usize)>> = Mutex::new(None);

/// 事件轮询队列（Dart 2.19 无 NativeCallable，轮询是跨版本统一通道；
/// 有回调注册时队列同样写入——轮询与回调并存，由 Dart 侧选用）。
static EVENT_QUEUE: Mutex<std::collections::VecDeque<CString>> =
    Mutex::new(std::collections::VecDeque::new());
static CALL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Dart 上报的网络类型缓存。0=Unknown 1=WiFi 2=Cellular 3=Ethernet。
static NETWORK_KIND: AtomicU8 = AtomicU8::new(0);

struct Host {
    state: std::sync::Arc<AppState>,
    router: std::sync::Arc<SignalRouter>,
}

// ---- HostBridge 的移动实现 ----

/// 移动端宿主桥：emit → C 回调推给 Dart；通知与剪贴板同样以事件形态
/// 交给 Dart（系统通知由 Flutter 插件显示；剪贴板由 Dart 调系统 API 写入）。
///
/// 剪贴板三件套是**尽力而为**：无法同步等待 Dart 执行结果（FFI 同步签名），
/// 返回 Ok 只表示「已投递给 UI 层」。移动端的「已写入剪贴板」提示由
/// Dart 侧在真正写完后自行展示，不依赖这里的返回值——这正是 V2 计划
/// §3.1 声明的移动端语义边界。
struct JsonBridge;

impl HostBridge for JsonBridge {
    fn emit(&self, event: &str, payload: &serde_json::Value) {
        push_event(event, payload.clone());
    }

    fn notify(&self, title: &str, body: &str) {
        push_event(
            "show-notification",
            serde_json::json!({ "title": title, "body": body }),
        );
    }

    fn write_clipboard_text(&self, text: String) -> Result<(), String> {
        push_event(
            "clipboard-write",
            serde_json::json!({ "kind": "text", "text": text }),
        );
        Ok(())
    }

    fn write_clipboard_image(&self, png: Vec<u8>) -> Result<(), String> {
        use base64::Engine;
        push_event(
            "clipboard-write",
            serde_json::json!({
                "kind": "image",
                "base64": base64::engine::general_purpose::STANDARD.encode(png),
            }),
        );
        Ok(())
    }

    fn inject_files_to_clipboard(&self, paths: Vec<PathBuf>) -> Result<(), String> {
        let paths: Vec<String> = paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        push_event(
            "clipboard-write",
            serde_json::json!({ "kind": "files", "paths": paths }),
        );
        Ok(())
    }

    fn network_kind(&self) -> NetworkKind {
        match NETWORK_KIND.load(Ordering::Relaxed) {
            1 => NetworkKind::Wifi,
            2 => NetworkKind::Cellular,
            3 => NetworkKind::Ethernet,
            _ => NetworkKind::Unknown,
        }
    }
}

/// 把一条事件投递给 Dart 宿主。
///
/// **内存契约：指针的所有权移交给 Dart**。`NativeCallable.listener`
/// 是异步投递——原生回调返回之后 Dart 才真正执行处理，因此字符串必须
/// 在回调返回后继续存活。这里用 `into_raw` 放弃所有权，Dart 侧读完
/// 后必须调 `unidrop_free_string` 归还；泄漏与否完全由这条约定保证。
fn push_event(event: &str, payload: serde_json::Value) {
    let json = serde_json::json!({ "event": event, "payload": payload });
    let Ok(text) = CString::new(json.to_string()) else {
        return;
    };
    // 轮询队列：所有权随 unidrop_poll_event 移交 Dart
    EVENT_QUEUE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push_back(text);
    let guard = EVENT_SINK.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((cb, user_data)) = *guard {
        // 回调路径：克隆一份（队列里那份仍由 poll 消费方释放）
        if let Ok(clone) = CString::new(json.to_string()) {
            cb(user_data, clone.into_raw());
        }
    }
}

/// 取一条待处理事件（非阻塞）。无事件返回 null；返回值所有权归调用方，
/// 须以 unidrop_free_string 归还。
#[no_mangle]
pub extern "C" fn unidrop_poll_event() -> *mut c_char {
    let front = EVENT_QUEUE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .pop_front();
    match front {
        Some(text) => text.into_raw(),
        None => std::ptr::null_mut(),
    }
}

// ---- FFI 出口 ----

fn take_string(ptr: *const c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Err("null pointer".into());
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(|s| s.to_string())
        .map_err(|e| format!("invalid UTF-8: {}", e))
}

fn give_string(s: String) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

/// 释放 Rust 返回的字符串。对同一指针重复调用是未定义行为，调用方自行保证。
#[no_mangle]
pub extern "C" fn unidrop_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe { drop(CString::from_raw(ptr)) };
    }
}

/// 注册事件回调（尽早调用，越早丢事件越少）。重复调用以后注册者为准。
#[no_mangle]
pub extern "C" fn unidrop_set_event_callback(cb: EventCallback, user_data: usize) {
    let mut guard = EVENT_SINK.lock().unwrap_or_else(|p| p.into_inner());
    *guard = Some((cb, user_data));
}

/// 启动核心。config JSON：
/// ```json
/// {
///   "db_path": "/…/unidrop.db",          // 必填：SQLite 落库位置
///   "cache_dir": "/…/UniDrop",           // 必填：收件与缓存目录
///   "device_name": "iPhone 17",          // 必填：平台设备名
///   "settings": {…AppSettings 覆盖, 可选}  // 一般不传，由持久化设置决定
/// }
/// ```
/// 返回 `{"ok":true,"data":{self_info}}` 或 `{"ok":false,"error":"…"}`。
#[no_mangle]
pub extern "C" fn unidrop_start(config_json: *const c_char) -> *mut c_char {
    let result = match start_inner(config_json) {
        Ok(info) => serde_json::json!({ "ok": true, "data": info }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    };
    give_string(result.to_string())
}

fn start_inner(config_json: *const c_char) -> Result<serde_json::Value, String> {
    install_logger();
    unidrop_core::install_crypto_provider();
    let cfg = take_string(config_json)?;
    let cfg: serde_json::Value = serde_json::from_str(&cfg).map_err(|e| e.to_string())?;

    // 排障日志双写的落点（见 StderrLogger 注释）。
    if let Some(dir) = cfg.get("cache_dir").and_then(|v| v.as_str()) {
        *DEBUG_LOG_PATH.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(PathBuf::from(dir).join("unidrop-debug.log"));
    }

    let db_path: String = cfg
        .get("db_path")
        .and_then(|v| v.as_str())
        .ok_or("config 缺少 db_path")?
        .to_string();
    let cache_dir: String = cfg
        .get("cache_dir")
        .and_then(|v| v.as_str())
        .ok_or("config 缺少 cache_dir")?
        .to_string();
    let device_name: String = cfg
        .get("device_name")
        .and_then(|v| v.as_str())
        .unwrap_or("UniClip Mobile")
        .to_string();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime 启动失败: {}", e))?;
    RUNTIME.set(runtime).map_err(|_| "已启动过，勿重复 start".to_string())?;
    let runtime = RUNTIME.get().expect("just set");

    // 路径就绪先于建库（与桌面一致的顺序，只是路径全部来自宿主注入）
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("收件目录创建失败: {}", e))?;
    let db_dir = std::path::Path::new(&db_path).parent();
    if let Some(d) = db_dir {
        std::fs::create_dir_all(d).map_err(|e| format!("数据库目录创建失败: {}", e))?;
    }

    let self_info = runtime.block_on(async {
        let db = storage::db::init_database(Some(PathBuf::from(&db_path)))
            .map_err(|e| format!("SQLite 初始化失败: {}", e))?;
        let device_id = storage::db::get_or_create_device_id(&db)
            .map_err(|e| format!("device_id 读取失败: {}", e))?;

        let mut initial_settings = match storage::db::get_persisted_settings(&db) {
            Some(json) => serde_json::from_str::<AppSettings>(&json).unwrap_or_else(|e| {
                log::warn!("Failed to parse persisted settings ({}), falling back to defaults", e);
                mobile_default_settings()
            }),
            None => mobile_default_settings(),
        };
        // 宿主显式覆盖（测试与首启向导用），不用于常规路径。
        if let Some(over) = cfg.get("settings") {
            if let Some(patched) = overlay_settings(&initial_settings, over) {
                initial_settings = patched;
            }
        }

        // 存量历史认领：与桌面同一动作，只认领 NULL 行，账号非法则跳过。
        if unidrop_core::settings::validate_account_id(&initial_settings.account_id).is_ok() {
            if let Ok(n) = storage::db::claim_unowned_history(&db, &initial_settings.account_id) {
                if n > 0 {
                    log::info!("Claimed {} legacy history rows", n);
                }
            }
        }

        let tls_trust = initial_settings.tls_trust_config().unwrap_or_else(|e| {
            log::warn!("证书指纹配置无效（{e}），本次启动按「仅信任公共 CA」处理");
            unidrop_core::core::tls_trust::TlsTrustConfig::public_ca()
        });
        let config = unidrop_core::core::connection_actor::ConnectionConfig {
            server_url: initial_settings.server_url.clone(),
            account_id: initial_settings.account_id.clone(),
            device_id: device_id.clone(),
            psk_secret: initial_settings.psk_secret.clone(),
            hostname: device_name.clone(),
            // aarch64-unknown-linux-ohos 上 consts::OS 报 "linux"，服务端
            // roster 与统计需要真实平台标识。
            os_type: if cfg!(target_os = "ohos") {
                "ohos".to_string()
            } else {
                std::env::consts::OS.to_string()
            },
            app_version: unidrop_core::app_state::APP_VERSION.to_string(),
            tls_trust,
        };

        let config_actor = std::sync::Arc::new(tokio::sync::RwLock::new(config));
        let reconnect_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let (actor, outgoing_tx) =
            unidrop_core::core::connection_actor::ConnectionActor::new(config_actor.clone(), reconnect_notify.clone());

        let app_state = AppState::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(db)),
            outgoing_tx,
            device_id,
            initial_settings,
            config_actor,
            reconnect_notify,
            HostEnv { hostname: device_name.clone(), cache_dir: Some(PathBuf::from(cache_dir)) },
        );
        let app_state = std::sync::Arc::new(app_state);

        let bridge = std::sync::Arc::new(JsonBridge);
        let router = std::sync::Arc::new(SignalRouter::new(app_state.clone(), bridge.clone()));
        router.spawn(actor);
        unidrop_core::signal_router::startup_recovery(&app_state, bridge.as_ref()).await;
        unidrop_core::signal_router::spawn_cache_sweeper(app_state.clone());

        let info = serde_json::json!({
            "device_id": app_state.device_id,
            "hostname": app_state.hostname,
            "os_type": app_state.os_type,
            "app_version": app_state.app_version,
        });
        HOST.set(Host { state: app_state, router })
            .map_err(|_| "already started".to_string())?;
        Ok::<serde_json::Value, String>(info)
    })?;

    Ok(self_info)
}

/// 移动端首启默认设置：与桌面 default_config 的唯一差异是接收策略——
/// 蜂窝流量计费下静默拉大文件不可接受（V2 计划 §4.3）。
fn mobile_default_settings() -> AppSettings {
    let mut s = AppSettings::default_config();
    s.receive_policy = ReceivePolicy::WifiOnly;
    s
}

/// 用 JSON 对象覆盖既有设置的字段（浅合并）。
fn overlay_settings(base: &AppSettings, over: &serde_json::Value) -> Option<AppSettings> {
    let mut merged = serde_json::to_value(base).ok()?;
    let obj = merged.as_object_mut()?;
    for (k, v) in over.as_object()? {
        obj.insert(k.clone(), v.clone());
    }
    serde_json::from_value(merged).ok()
}

/// 停止核心（App 退出时调用；日常切后台**不要**调，让连接自然挂起即可）。
#[no_mangle]
pub extern "C" fn unidrop_stop() {
    // OnceLock 里的 runtime 不取出（取了就无法归还），停机交由进程退出。
    // 这里只做一件事：把事件回调摘掉，避免退出路径上再往 Dart 投递。
    let mut guard = EVENT_SINK.lock().unwrap_or_else(|p| p.into_inner());
    *guard = None;
}

/// 统一命令入口。立即返回 `{"ok":true,"call_id":"…"}`（已入队）；
/// 执行结果经 `invoke-result` 事件投递 `{"call_id","ok","data"|"error"}`。
///
/// 支持的命令：
/// - `get_self_info` / `get_devices` / `get_server_limits`
/// - `get_settings` / `save_settings {settings}`
/// - `send_files {target_device, paths:[…]}`
/// - `send_text {target_device, text}` / `send_image {target_device, name, base64}`
/// - `respond_offer {session_id, accept}`
/// - `list_history` / `get_session_files {session_id}`
/// - `set_network {kind:"wifi"|"cellular"|"ethernet"|"unknown"}`
/// - `reconnect`
#[no_mangle]
pub extern "C" fn unidrop_invoke(cmd_json: *const c_char) -> *mut c_char {
    let call_id = CALL_SEQ.fetch_add(1, Ordering::Relaxed).to_string();
    let enqueue = (|| -> Result<(), String> {
        let host = HOST.get().ok_or("尚未 start")?;
        let text = take_string(cmd_json)?;
        let mut cmd: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("命令 JSON 解析失败: {}", e))?;
        let name = cmd
            .get("cmd")
            .and_then(|v| v.as_str())
            .ok_or("缺少 cmd 字段")?
            .to_string();
        if let Some(obj) = cmd.as_object_mut() {
            obj.remove("cmd");
        }
        let state = host.state.clone();
        let router = host.router.clone();
        let call_id_for_task = call_id.clone();
        RUNTIME.get().expect("runtime alive").spawn(async move {
            let result = dispatch(&name, cmd, &state, &router).await;
            let payload = match result {
                Ok(data) => serde_json::json!({ "call_id": call_id_for_task, "ok": true, "data": data }),
                Err(e) => serde_json::json!({ "call_id": call_id_for_task, "ok": false, "error": e }),
            };
            push_event("invoke-result", payload);
        });
        Ok(())
    })();

    let resp = match enqueue {
        Ok(()) => serde_json::json!({ "ok": true, "call_id": call_id }),
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    };
    give_string(resp.to_string())
}

async fn dispatch(
    name: &str,
    args: serde_json::Value,
    state: &std::sync::Arc<AppState>,
    router: &std::sync::Arc<SignalRouter>,
) -> Result<serde_json::Value, String> {
    match name {
        "get_self_info" => Ok(serde_json::to_value(OnlineDevice {
            device_id: state.device_id.clone(),
            hostname: state.hostname.clone(),
            os_type: state.os_type.clone(),
            app_version: state.app_version.clone(),
            remote_ip: None,
        })
        .map_err(|e| e.to_string())?),
        "get_devices" => {
            let devs = state.online_devices.lock().await;
            Ok(serde_json::to_value(&*devs).map_err(|e| e.to_string())?)
        }
        "get_server_limits" => {
            let limits = state.server_limits.lock().await;
            Ok(serde_json::to_value(&*limits).map_err(|e| e.to_string())?)
        }
        "get_settings" => {
            let s = state.settings.lock().await;
            Ok(serde_json::to_value(&*s).map_err(|e| e.to_string())?)
        }
        "save_settings" => {
            let new_settings: AppSettings = serde_json::from_value(
                args.get("settings").cloned().ok_or("缺少 settings")?,
            )
            .map_err(|e| format!("settings 解析失败: {}", e))?;
            unidrop_core::settings::save_settings_flow(state, &JsonBridge, new_settings).await?;
            Ok(serde_json::Value::Null)
        }
        "send_files" => {
            let target = args.get("target_device").and_then(|v| v.as_str()).ok_or("缺少 target_device")?.to_string();
            let paths: Vec<String> = args
                .get("paths")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect())
                .ok_or("缺少 paths")?;
            let sid = unidrop_core::send_flow::send_files_flow(state, &JsonBridge, target, paths).await?;
            Ok(serde_json::json!({ "session_id": sid }))
        }
        "send_text" => {
            let target = args.get("target_device").and_then(|v| v.as_str()).ok_or("缺少 target_device")?.to_string();
            let text = args.get("text").and_then(|v| v.as_str()).ok_or("缺少 text")?.to_string();
            log::info!("send_text command: target={} len={}", target, text.len());
            let sid = unidrop_core::send_flow::send_bytes_flow(
                state, &JsonBridge, target, "TEXT", "clipboard.txt", text.into_bytes(),
            )
            .await?;
            Ok(serde_json::json!({ "session_id": sid }))
        }
        "send_image" => {
            use base64::Engine;
            let target = args.get("target_device").and_then(|v| v.as_str()).ok_or("缺少 target_device")?.to_string();
            let img_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("clipboard.png").to_string();
            let b64 = args.get("base64").and_then(|v| v.as_str()).ok_or("缺少 base64")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("base64 解码失败: {}", e))?;
            let sid = unidrop_core::send_flow::send_bytes_flow(
                state, &JsonBridge, target, "IMAGE", &img_name, bytes,
            )
            .await?;
            Ok(serde_json::json!({ "session_id": sid }))
        }
        "respond_offer" => {
            let session_id = args.get("session_id").and_then(|v| v.as_str()).ok_or("缺少 session_id")?.to_string();
            let accept = args.get("accept").and_then(|v| v.as_bool()).unwrap_or(false);
            router.respond_pending_offer(&session_id, accept).await?;
            Ok(serde_json::Value::Null)
        }
        "list_history" => {
            let account_id = { state.settings.lock().await.account_id.clone() };
            let conn = state.db_conn.lock().await;
            let entries = HistoryRepo::list_history(&conn, &account_id, 200).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(entries).map_err(|e| e.to_string())?)
        }
        "get_session_files" => {
            let session_id = args.get("session_id").and_then(|v| v.as_str()).ok_or("缺少 session_id")?;
            // 归属闸门：与桌面 ensure_session_owned 同一约束——移动端拿到路径后
            // 会把文件交给分享面板，切账号后残留的卡片点下去同样会读到别人的文件。
            let account_id = { state.settings.lock().await.account_id.clone() };
            {
                let conn = state.db_conn.lock().await;
                if !HistoryRepo::session_belongs_to(&conn, session_id, &account_id) {
                    return Err("该记录不属于当前账号".to_string());
                }
            }
            let files = state.cache_manager.get_session_files(session_id).await?;
            Ok(serde_json::json!({ "paths": files }))
        }
        "set_network" => {
            let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
            let code = match kind {
                "wifi" => 1,
                "cellular" => 2,
                "ethernet" => 3,
                _ => 0,
            };
            NETWORK_KIND.store(code, Ordering::Relaxed);
            Ok(serde_json::Value::Null)
        }
        "reconnect" => {
            // 前台化快速重连：跳过退避等待（复用桌面的即时唤醒机制）
            state.reconnect_notify.notify_waiters();
            Ok(serde_json::Value::Null)
        }
        other => Err(format!("未知命令: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_kind_maps_codes() {
        assert_eq!(JsonBridge.network_kind(), NetworkKind::Unknown);
        NETWORK_KIND.store(1, Ordering::Relaxed);
        assert_eq!(JsonBridge.network_kind(), NetworkKind::Wifi);
        NETWORK_KIND.store(2, Ordering::Relaxed);
        assert_eq!(JsonBridge.network_kind(), NetworkKind::Cellular);
        NETWORK_KIND.store(3, Ordering::Relaxed);
        assert_eq!(JsonBridge.network_kind(), NetworkKind::Ethernet);
        NETWORK_KIND.store(0, Ordering::Relaxed);
    }

    #[test]
    fn mobile_default_receive_policy_is_wifi_only() {
        // 移动端首启默认必须是 wifi_only——这条钉住 V2 计划 §4.3 的决策，
        // 防止将来有人把它「统一」回 Always。
        assert_eq!(mobile_default_settings().receive_policy, ReceivePolicy::WifiOnly);
        // 桌面默认保持 Always（老行为），两端的差异是刻意的。
        assert_eq!(AppSettings::default_config().receive_policy, ReceivePolicy::Always);
    }

    #[test]
    fn overlay_merges_shallowly() {
        let base = AppSettings::default_config();
        let over = serde_json::json!({ "account_id": "mobile-user" });
        let patched = overlay_settings(&base, &over).expect("合法覆盖必须成功");
        assert_eq!(patched.account_id, "mobile-user");
        assert_eq!(patched.server_url, base.server_url, "未覆盖字段必须保留");
    }

    /// 启动链冒烟：Dart bootstrap 的挂起排查用。
    /// 复现 unidrop_start → unidrop_invoke(get_devices/get_settings) →
    /// 轮询事件拿 invoke-result 的完整链路；任何环节挂起/超时都会在这里暴露。
    /// 单独二进制跑（RUNTIME 是 OnceLock，进程内只能 start 一次）：
    /// `cargo test -p unidrop-mobile-native bootstrap_smoke -- --nocapture --test-threads=1`
    #[test]
    fn bootstrap_smoke_start_invoke_poll() {
        let dir = std::env::temp_dir().join("unidrop-bootstrap-smoke");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = format!(
            "{{\"db_path\":\"{}/db.sqlite\",\"cache_dir\":\"{}/cache\",\"device_name\":\"smoke\"}}",
            dir.display(),
            dir.display()
        );

        // 1. start（10 秒上限：Dart 侧是同步 FFI 调用，挂这里 UI 就白屏/转圈）
        let cfg_c = CString::new(cfg).unwrap();
        let resp = unsafe {
            let ptr = unidrop_start(cfg_c.as_ptr());
            assert!(!ptr.is_null());
            let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            unidrop_free_string(ptr);
            s
        };
        eprintln!("[smoke] start: {}", &resp[..resp.len().min(300)]);
        assert!(resp.contains("\"ok\":true"), "start 失败: {resp}");

        // 2. 依次 invoke get_devices / get_settings，轮询等 invoke-result
        for cmd in ["get_devices", "get_settings"] {
            let cmd_c = CString::new(format!("{{\"cmd\":\"{cmd}\"}}")).unwrap();
            let resp = unsafe {
                let ptr = unidrop_invoke(cmd_c.as_ptr());
                assert!(!ptr.is_null());
                let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
                unidrop_free_string(ptr);
                s
            };
            eprintln!("[smoke] {cmd} enqueue: {}", &resp[..resp.len().min(200)]);
            assert!(resp.contains("\"ok\":true"), "{cmd} 入队失败: {resp}");
            let call_id: String = serde_json::from_str::<serde_json::Value>(&resp)
                .unwrap()["call_id"]
                .as_str()
                .unwrap()
                .to_string();

            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut got: Option<String> = None;
            while std::time::Instant::now() < deadline {
                while let Some(ev) = poll_event_owned() {
                    if ev.contains(&call_id) {
                        got = Some(ev);
                    }
                }
                if got.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let got = got.unwrap_or_else(|| panic!("{cmd} 10 秒内无 invoke-result——链路挂起"));
            eprintln!("[smoke] {cmd} result: {}", &got[..got.len().min(400)]);
            assert!(got.contains("\"ok\":true"), "{cmd} 执行失败: {got}");
        }
    }

    fn poll_event_owned() -> Option<String> {
        unsafe {
            let ptr = unidrop_poll_event();
            if ptr.is_null() {
                return None;
            }
            let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            unidrop_free_string(ptr);
            Some(s)
        }
    }
}
