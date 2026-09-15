//! macOS 系统通知：UNUserNotificationCenter 实现。
//!
//! 为什么不走 tauri-plugin-notification：它经 notify-rust 落到
//! mac-notification-sys 的 `NSUserNotificationCenter`（objc/notify.m），
//! 而该 API 自 macOS 11 起被 Apple 废弃，在 macOS 26 上已不再投递任何通知。
//! 失败还是静默的——ObjC 层不报错，插件内部又把 `show()` 的错误吞进
//! `spawn(async { let _ = ... })`，所以此前零信号。Windows/Linux 仍走插件，
//! 只有 macOS 换成这里的实现。
//!
//! 本模块用的是 objc2 0.6 那一代（`objc2_v06` / `objc2_foundation_v06`），
//! 与剪贴板模块的 0.5 并存。两者不交换任何类型，理由见 Cargo.toml 的注释。

use std::ptr::NonNull;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use block2::{DynBlock, RcBlock};
use objc2_foundation_v06::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSettings, UNNotificationSound,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use objc2_v06::rc::Retained;
use objc2_v06::runtime::{Bool, ProtocolObject};
// AnyThread 看着像死导入，其实不是：下面的 NotificationDelegate::alloc()
// 就是这个 trait 的方法，去掉它会 E0599（代码审查中有人据"全文没再出现过
// AnyThread 字样"提过删除建议，实测编译不过）。
use objc2_v06::{define_class, msg_send, AnyThread};
use tauri::{AppHandle, Emitter};

/// 供 delegate 回调唤起主窗口用。delegate 由 ObjC 运行时在任意线程调起，
/// 拿不到调用方传来的上下文，只能从这里取。
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// 最近一次探测到的通知授权状态：0 = 尚未探测，1 = 已授权，2 = 被拒。
///
/// 事件（notification-auth-status）是「推」，cmd_get_notification_auth_status
/// 读这里是「拉」——webview 注册 listener 可能晚于 setup 里的首次探测，
/// 只推不拉的话横幅在那种时序下永远不会出现。菜单栏放置横幅同理，
/// 见 window_cmd.rs 里 cmd_get_tray_placement 的注释。
static AUTH_STATUS: AtomicU8 = AtomicU8::new(0);

/// UNErrorCodeNotificationsNotAllowed。objc2-user-notifications 没有生成
/// 这个枚举，只能按 UserNotifications/UNError.h 的定义手写常量。
const UN_ERROR_NOTIFICATIONS_NOT_ALLOWED: isize = 1;

/// 记录并广播授权状态。
fn publish_auth_status(app: &AppHandle, granted: bool) {
    AUTH_STATUS.store(if granted { 1 } else { 2 }, Ordering::Relaxed);
    // 窗口隐藏时 webview 仍然存活、事件仍然送达，React state 会更新——
    // 用户下次打开主窗口就能看到引导横幅，不必恰好守着窗口等这一刻。
    if let Err(e) = app.emit(
        "notification-auth-status",
        serde_json::json!({ "granted": granted }),
    ) {
        log::warn!("Failed to emit notification-auth-status: {}", e);
    }
}

/// 拉取最近一次探测到的授权状态。`None` = 尚未探测或不适用
/// （dev 裸跑被 running_as_app_bundle 拦下的情形），前端据此不显示横幅。
pub fn cached_auth_status() -> Option<bool> {
    match AUTH_STATUS.load(Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// delegate 必须由我们自己长期持有：`setDelegate:` 是 **weak property**
/// （见绑定中 `UNUserNotificationCenter::setDelegate` 的文档注释），
/// 通知中心不会持有它，一旦 Retained 被 drop，点击回调就静默失效。
static DELEGATE: OnceLock<Retained<NotificationDelegate>> = OnceLock::new();

define_class!(
    // SAFETY:
    // - 父类 NSObject 无子类化要求。
    // - NotificationDelegate 不实现 Drop。
    // 刻意不标 `#[thread_kind = MainThreadOnly]`：UNUserNotificationCenterDelegate
    // 的回调线程不受保证（协议定义里没有任何 MainThreadOnly/MainThreadMarker 约束），
    // 标了反而会在非主线程被调起时触发断言。
    #[unsafe(super(NSObject))]
    #[name = "UniDropNotificationDelegate"]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        /// 应用在前台时，macOS 默认**不**展示通知。不实现这个回调，
        /// 用户在窗口开着时收不到任何提示，会被当成「又不工作了」。
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            // Banner 是 macOS 11.0+ 才有的选项（10.x 的前台展示走已废弃的 Alert）。
            // 这正是 tauri.conf.json 把 bundle.macOS.minimumSystemVersion 钉在
            // "11.0" 的原因——JSON 里写不了注释，理由记在这里：清单若仍是默认的
            // 10.13，旧系统上会收到自己不认识的 bit，前台通知被静默抑制，
            // 又变回本次要根除的那种哑故障。
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::Sound,));
        }

        /// 用户对通知作出动作。只有「点击通知本体」才唤起窗口——
        /// 划走/关闭通知是用户明确表示不想处理，此时弹窗口是打扰。
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &DynBlock<dyn Fn()>,
        ) {
            let action = response.actionIdentifier().to_string();
            let default_action = unsafe { UNNotificationDefaultActionIdentifier.to_string() };

            if action == default_action {
                if let Some(app) = APP_HANDLE.get() {
                    let app_for_main = app.clone();
                    // 本回调的线程不受保证，而 reveal_main_window 内部的
                    // is_minimized / unminimize / show / set_focus 都是 NSWindow 系
                    // API，必须在主线程执行。闭包只捕获 AppHandle（'static），
                    // 可以安全移入；completionHandler 则不行——它是借用引用，
                    // 所以下面在当前帧同步调用，不进这个闭包。
                    if let Err(e) = app.run_on_main_thread(move || {
                        crate::reveal_main_window(&app_for_main);
                    }) {
                        log::warn!("Failed to dispatch window reveal to main thread: {}", e);
                    }
                }
            }

            completion_handler.call(());
        }
    }
);

/// 当前进程是否真以本应用的身份在运行。
///
/// 判据刻意**不是**「bundleIdentifier 为 nil」：从终端跑 `tauri dev` 时，
/// 裸二进制的 mainBundle 会回落到宿主应用（如 Terminal），identifier 非空，
/// nil 判据根本不触发，于是会以 Terminal 的身份去请求通知授权，
/// 在开发机上弹出「Terminal 想要发送通知」。tauri-plugin-notification 自己在
/// dev 模式特意 `set_application("com.apple.Terminal")`，正是对这一回落的让步。
/// 比对 identifier 才能同时覆盖 nil 与回落两种情形。
fn running_as_app_bundle(app: &AppHandle) -> bool {
    let expected = &app.config().identifier;
    let bundle = NSBundle::mainBundle();
    match bundle.bundleIdentifier() {
        Some(id) => {
            let actual = id.to_string();
            if &actual != expected {
                log::warn!(
                    "Not running as the app bundle (bundle id: {}, expected: {}); \
                     skipping macOS notifications",
                    actual,
                    expected
                );
                return false;
            }
            true
        }
        None => {
            log::warn!("Process has no bundle identifier; skipping macOS notifications");
            false
        }
    }
}

/// 装 delegate 并请求通知授权。由 setup 调用一次。
///
/// 这里同样要过 `running_as_app_bundle`：`currentNotificationCenter()` 在没有
/// 合法 bundle 的进程里会抛 NSInternalInconsistencyException，
/// 而 setDelegate 与 requestAuthorization 都要先取它——不拦就是启动即崩。
pub fn init_notifications(app: &AppHandle) {
    if !running_as_app_bundle(app) {
        return;
    }

    let _ = APP_HANDLE.set(app.clone());

    let center = UNUserNotificationCenter::currentNotificationCenter();

    // 先装 delegate 再请求授权：授权弹窗期间就可能有通知回调进来，
    // 顺序反了会丢掉这段窗口内的事件。
    let delegate = DELEGATE.get_or_init(|| {
        let this = NotificationDelegate::alloc();
        unsafe { msg_send![this, init] }
    });
    let proto = ProtocolObject::from_ref(&**delegate);
    center.setDelegate(Some(proto));

    // 不要 Badge：本应用没有角标需求，多要一项权限只会让授权弹窗更可疑。
    let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
    let app_for_probe = app.clone();
    let handler = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        if !error.is_null() {
            let msg = unsafe { (*error).localizedDescription() }.to_string();
            log::warn!("Notification authorization request failed: {}", msg);
        } else if granted.as_bool() {
            log::info!("Notification authorization granted");
        } else {
            // 不是错误，但必须留痕：否则「通知不弹」又会变成无从排查的哑故障，
            // 而这正是本次修复要根除的那种形态。
            log::warn!(
                "Notification authorization denied by user; \
                 transfer notifications will not be shown until it is enabled in System Settings"
            );
        }

        // request 的 granted 只反映「本次请求」的瞬时结果，反映不了系统侧
        // 后续的翻转——替换安装后授权记录失效、用户手动改系统设置，都会让
        // 它与真实状态脱节（2026-09-14 的实机日志：请求报 error 1 的同时
        // 投递也全部失败，状态只看请求侧就永远解释不了）。getNotificationSettings
        // 是系统给的权威快照，横幅状态以它为准。
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let app_for_settings = app_for_probe.clone();
        let probe = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let granted = matches!(
                status,
                UNAuthorizationStatus::Authorized
                    | UNAuthorizationStatus::Provisional
                    | UNAuthorizationStatus::Ephemeral
            );
            log::info!(
                "Notification authorization status from settings: {:?} (granted: {})",
                status,
                granted
            );
            publish_auth_status(&app_for_settings, granted);
        });
        center.getNotificationSettingsWithCompletionHandler(&probe);
    });
    center.requestAuthorizationWithOptions_completionHandler(options, &handler);
}

/// 投递一条通知。
pub fn show_notification(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    // 与 init 各自独立判定，不依赖调用顺序：即便 init 因故没跑，这里也不会崩。
    if !running_as_app_bundle(app) {
        return Ok(());
    }

    let center = UNUserNotificationCenter::currentNotificationCenter();

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));

    // identifier 用随机 UUID：同 id 的后一条会覆盖前一条，
    // 连续收到多次传输时只会剩最后一条。
    let identifier = NSString::from_str(&uuid::Uuid::new_v4().to_string());
    // trigger 传 None 即立即投递。
    let request =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);

    let app_for_error = app.clone();
    let handler = RcBlock::new(move |error: *mut NSError| {
        if !error.is_null() {
            let msg = unsafe { (*error).localizedDescription() }.to_string();
            log::warn!("Failed to deliver notification: {}", msg);

            // 授权在启动后被翻转（重装/系统设置变更）时，最先暴露的就是这里：
            // 启动探测 granted、投递却报 NotificationsNotAllowed。只记日志的话，
            // 用户视角仍是「没提示、也没人告诉我为什么」——哑故障原样复发。
            // 只有 code==1（未授权）才广播 denied：其他错误码（如载荷无效）
            // 引导用户去开通知开关是误诊。
            if unsafe { (*error).code() } == UN_ERROR_NOTIFICATIONS_NOT_ALLOWED {
                publish_auth_status(&app_for_error, false);
            }
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&handler));

    Ok(())
}
