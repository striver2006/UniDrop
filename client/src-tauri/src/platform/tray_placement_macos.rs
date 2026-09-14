//! macOS 26 (Tahoe) 菜单栏放置探测。
//!
//! # 背景：这里到底在解决什么
//!
//! macOS 26 起，菜单栏项不再由应用自己绘制，而是由「控制中心」进程**托管**：
//! 系统为每个获得槽位的状态项，在控制中心名下建一个 window level 25 的窗口，
//! **窗口名就是该状态项的 `autosaveName`**。应用进程内只留一个坐标不可信的影子窗口。
//!
//! 没拿到槽位时，应用侧**完全没有错误**——状态项建得出来、菜单弹得开、
//! 点击回调也照常触发，只是控制中心那边根本没有对应窗口，肉眼就是「图标没出现」。
//! 这是本模块要根除的哑故障：应用在后台好好跑着，用户既看不到图标，也得不到任何解释。
//!
//! # 实测数据（macOS 26.6.2，3440x1440 单屏 1:1）
//!
//! | 状态 | 控制中心是否有同名窗口 | 进程内影子窗口 frame |
//! |---|---|---|
//! | 正常（对照的原生应用，设了 autosaveName） | 有，名为其 autosaveName | —— |
//! | 被拒（本应用，未设 autosaveName 时） | **完全没有** | (3406, 1418, 34, 22) |
//! | 被拒（更早一次观测） | 完全没有 | (3405, -1, 35, 24) |
//!
//! 注意两次被拒的 y 完全不同（1418 与 -1），**但 x + width 都精确等于屏幕宽度**。
//! 所以判据不能按 y 写——按 y 写的第一版把 (3406, 1418) 误判成了「已放置」，
//! 而它其实正压在时钟底下。「停在屏幕最右、宽度刚好顶到边」才是没拿到槽位的特征。
//!
//! # 边界：本模块只判断，不修复
//!
//! 1. 不要去写 `group.com.apple.controlcenter` 的 plist。那是别家 App Group 容器，
//!    既无公开 API，也不该写——绕过用户授权开关正是那个机制存在的理由。
//! 2. 不要再动托盘的创建时机，也不要「没放上就重建」。setup / `RunEvent::Ready` /
//!    +800ms / +3000ms / 强制重建 五种形态实测坐标全都相同，时机不是变量
//!    （见 `lib.rs::build_tray` 与 tauri-apps/tauri#13770）。
//! 3. 不要切 activation policy。同上，已被穷举证伪。
//!
//! # 被拒的真实机制（2026-09-14 晚定案，勿再改写成别的说法）
//!
//! 控制中心把准入落在 `group.com.apple.controlcenter.plist` 的 `trackedApplications`
//! 里，每条记录是 `{ location, menuItemLocations, isAllowed }`。判定是**连坐**的：
//! 一个 bundle id 只要出现在**任一** `isAllowed=false` 记录的 `menuItemLocations`
//! 里就被挡，**它自己那条记录是 `true` 也没用**。
//!
//! 挂到别人名下的途径是**责任进程（responsible process）**：从 IDE 集成终端直接
//! 执行可执行文件时，责任进程是那个 IDE，控制中心就按 IDE 归属这个菜单栏项。
//! 实测 `com.unidrop.client` 被记在 `com.microsoft.VSCode` 与 `com.google.antigravity`
//! 名下，而这两条都是 `isAllowed=false`，于是本应用无论怎么改都上不了屏。
//! 用 `open` 启动的 .app 由 launchd 负责，不会挂到 IDE 下——**开发期构建完一律
//! `open` 启动，不要在集成终端里直接跑二进制**，已经挂错的归属不会自动清理。
//!
//! 解除只需在「系统设置 › 菜单栏 › 应用程序」里打开**那条挡着它的记录**的开关
//! （即那个 IDE 的开关，不是本应用自己的）。实测开关一开，控制中心立刻输出
//! `Unblocking host`，运行中的进程无需重启即恢复。
//!
//! 已实测**无效**、不要再试的手段：清 LaunchServices 死记录、重启控制中心、
//! `tccutil reset`、整机重启、把本应用自己那行开关关掉再打开。
//! 曾经按「LS 死记录触发粘性拉黑」写过一个启动期清理模块，该归因已被证伪，
//! 模块随之删除；别照着旧 commit（`ba6cc06`）的说法再恢复它。
//!
//! 应用侧能做的正事只有一件：给状态项设 `autosaveName`（`lib.rs::build_tray`），
//! 那是本模块主判据的前提，不是修复手段。剩下的是把哑故障翻译给用户，
//! 引导文案要指向「打开挡着它的那个应用的开关」，别再折腾状态项本身。
//!
//! 本模块用 objc2 0.6 那一代，与 `notification_macos` 对齐。
//! **只许 import `objc2_*_v06` / `objc2_core_*`**：误用剪贴板那边的 0.5 代，
//! 类型不兼容的报错会非常难读。

use std::ffi::c_void;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use objc2_app_kit_v06::{NSApplication, NSScreen, NSStatusBar, NSStatusWindowLevel};
use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CGFloat, CGRect};
use objc2_core_graphics::{
    kCGWindowLayer, kCGWindowName, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_v06::MainThreadMarker;
use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::AppState;
use crate::storage::db;

/// 状态项的稳定身份。
///
/// 控制中心托管窗口以此为名，本模块的主判据就是「这个名字在不在 layer 25 上」。
/// **`lib.rs::build_tray` 用同一个常量调 `setAutosaveName`，两边必须一致**，
/// 改名字要一起改，否则自检会把正常状态误判成被拒。
pub const TRAY_AUTOSAVE_NAME: &str = "UniDrop";

/// 控制中心托管菜单栏项所用的窗口层。
const MENU_BAR_WINDOW_LAYER: i64 = 25;

/// 前端监听的事件名。放置状态每次发生变化都会推一次。
pub const PLACEMENT_EVENT: &str = "macos-tray-placement";

/// 已就「菜单栏图标被系统拒绝」引导过用户一次。
///
/// 只按「键是否存在」判定，值里存版本与时间戳纯为排障留痕。
/// **不要按版本重新引导**：用户修好之后，没有理由因为升级再被打扰一次。
const FLAG_GUIDANCE_SHOWN: &str = "macos_menubar_guidance_shown";

/// 用户明确选择「下次不要因为托盘不可见而自动打开窗口」。
const FLAG_FORCE_REVEAL_OPTOUT: &str = "macos_menubar_force_reveal_optout";

/// 状态项在菜单栏里的放置结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayPlacement {
    /// 控制中心确实托管了我们的菜单栏项 —— 图标可见。
    Placed,
    /// 控制中心没有我们的菜单栏项 —— 图标不可见。
    Rejected,
    /// 判据本身失效，读不出结论。
    ///
    /// **刻意与 `Rejected` 分成两个态，不要合并。** 两者处置完全不同：
    /// `Rejected` 要引导用户；`Unknown` 说明判据失效（系统改了实现、
    /// 或窗口名读不到），那是我们自己的问题。合并之后就再也分不出
    /// 「判据失效」和「真被拒」——而判据失效恰恰是这套启发式最可能的失败形态，
    /// 且失效方向是「误报被拒」，会甩给用户一个查不出所以然的告警。
    Unknown,
}

const CACHE_UNSET: u8 = 0;
const CACHE_PLACED: u8 = 1;
const CACHE_REJECTED: u8 = 2;
const CACHE_UNKNOWN: u8 = 3;

/// 最近一次探测结果。
///
/// 用模块内的原子量而不是往 `AppState` 加字段：那是三平台共用的结构体，
/// 塞一个 macOS-only 的字段进去就得给它挂 `#[cfg]`，污染面远大于收益。
static LAST_PLACEMENT: AtomicU8 = AtomicU8::new(CACHE_UNSET);

impl TrayPlacement {
    fn to_cache(self) -> u8 {
        match self {
            TrayPlacement::Placed => CACHE_PLACED,
            TrayPlacement::Rejected => CACHE_REJECTED,
            TrayPlacement::Unknown => CACHE_UNKNOWN,
        }
    }

    fn from_cache(v: u8) -> Self {
        match v {
            CACHE_PLACED => TrayPlacement::Placed,
            CACHE_REJECTED => TrayPlacement::Rejected,
            // CACHE_UNSET 也落这里：尚未探测过时对外是「还不知道」，
            // 而不是乐观地报 Placed。
            _ => TrayPlacement::Unknown,
        }
    }
}

/// 最近一次探测结果，无阻塞。供 `cmd_get_tray_placement` 同步读取。
pub fn last_placement() -> TrayPlacement {
    TrayPlacement::from_cache(LAST_PLACEMENT.load(Ordering::Relaxed))
}

/// 推给前端的载荷。横幅要同时知道「被拒了」和「该显示哪几个按钮」。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TrayPlacementPayload {
    pub placement: TrayPlacement,
    /// 用户是否开了「启动即最小化到托盘」。被拒时这个设定会让他彻底没有入口，
    /// 横幅需要据此解释「窗口为什么自己弹出来了」。
    pub start_minimized: bool,
    pub guidance_shown: bool,
    pub force_reveal_optout: bool,
}

// ---------------------------------------------------------------------------
// 主判据：控制中心有没有托管我们的菜单栏项
// ---------------------------------------------------------------------------

/// 在托管窗口列表里找我们自己的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostedLookup {
    /// layer 25 上有一个名为 `TRAY_AUTOSAVE_NAME` 的窗口。
    Found,
    /// 读到了别人的菜单栏项，但没有我们的。
    NotFound,
    /// 一个 layer 25 的窗口名都读不出来——判据本身不可用，别据此下结论。
    Unreadable,
}

/// 从窗口信息字典里取一个整数字段。
///
/// # Safety
/// `dict` 必须是 `CGWindowListCopyWindowInfo` 返回的窗口信息字典。
unsafe fn dict_i64(dict: &CFDictionary, key: &CFString) -> Option<i64> {
    let raw = unsafe { dict.value(key as *const CFString as *const c_void) };
    if raw.is_null() {
        return None;
    }
    let num = unsafe { &*(raw as *const CFNumber) };
    num.as_i64()
}

/// 从窗口信息字典里取一个字符串字段。
///
/// # Safety
/// 同 `dict_i64`。
unsafe fn dict_string(dict: &CFDictionary, key: &CFString) -> Option<String> {
    let raw = unsafe { dict.value(key as *const CFString as *const c_void) };
    if raw.is_null() {
        return None;
    }
    let s = unsafe { &*(raw as *const CFString) };
    Some(s.to_string())
}

/// 查控制中心是否托管了我们的菜单栏项。任意线程可调。
///
/// 为什么按**名字**找而不是按 pid：macOS 26 上菜单栏项的窗口归「控制中心」进程所有，
/// 不归我们。第一版按自身 pid 筛 layer 25，结果永远筛不到任何东西——那条判据
/// 从一开始就不可能生效，是拿旧系统的模型写的。
///
/// `Unreadable` 这一档不可省：`kCGWindowName` 在某些权限配置下会整体返回空，
/// 那时「找不到我们的名字」并不代表被拒。分不出这两者就会误报。
fn probe_hosted_extra() -> HostedLookup {
    // OptionAll 而不是 OnScreenOnly：被拒的项很可能不算「on screen」。
    let Some(list) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionAll, 0) else {
        return HostedLookup::Unreadable;
    };

    let mut saw_any_name = false;
    for i in 0..list.count() {
        let raw = unsafe { list.value_at_index(i) };
        if raw.is_null() {
            continue;
        }
        let dict = unsafe { &*(raw as *const CFDictionary) };

        if unsafe { dict_i64(dict, &kCGWindowLayer) } != Some(MENU_BAR_WINDOW_LAYER) {
            continue;
        }
        let Some(name) = (unsafe { dict_string(dict, &kCGWindowName) }) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        saw_any_name = true;
        if name == TRAY_AUTOSAVE_NAME {
            return HostedLookup::Found;
        }
    }

    if saw_any_name {
        HostedLookup::NotFound
    } else {
        HostedLookup::Unreadable
    }
}

// ---------------------------------------------------------------------------
// 副判据：进程内影子窗口的几何
// ---------------------------------------------------------------------------

/// 影子窗口是否停在「从未获得槽位」的位置。
///
/// 判据是 **x + width 顶到屏幕右边缘**，不是看 y。实测两次被拒的 y 分别是
/// -1 和 1418（后者恰好落在菜单栏带内），按 y 写的判据把后者误判成了已放置；
/// 而两次的 x + width 都精确等于屏幕宽度。没拿到槽位的项就停在最右边，
/// 压在时钟底下——时钟永远是最右的那个，正常获得槽位的项不可能顶到边缘。
pub(crate) fn looks_parked(item: CGRect, screen: CGRect) -> bool {
    let screen_right = screen.origin.x + screen.size.width;
    let item_right = item.origin.x + item.size.width;
    // 容差 1pt：观测到过 3405+35 与 3406+34 两组，都精确等于 3440，
    // 但不同缩放下可能有半点误差。
    (item_right - screen_right).abs() <= 1.0
}

/// 从 `NSApp.windows()` 里找出状态项的影子窗口，返回 (影子 frame, 基准屏 frame, 菜单栏厚度)。
///
/// **必须在主线程调用**：NSWindow / NSScreen 系 API 都有主线程要求。
fn probe_appkit(mtm: MainThreadMarker) -> Option<(CGRect, CGRect, CGFloat)> {
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();

    // 主窗口是 NSNormalWindowLevel(0)，弹出菜单是 NSPopUpMenuWindowLevel(101)，
    // 只有状态项的影子窗口落在 NSStatusWindowLevel(25)。
    let mut status_frame = None;
    for i in 0..windows.count() {
        let win = windows.objectAtIndex(i);
        if win.level() == NSStatusWindowLevel {
            status_frame = Some(win.frame());
            break;
        }
    }
    let status_frame = status_frame?;

    // 基准屏必须取 screens()[0] 而不是 mainScreen()：后者是「key window 所在的屏」，
    // 与菜单栏所在的屏无关，多屏下会取错。
    let screens = NSScreen::screens(mtm);
    if screens.count() == 0 {
        return None;
    }
    let screen_frame = screens.objectAtIndex(0).frame();
    let thickness = NSStatusBar::systemStatusBar().thickness();

    Some((status_frame, screen_frame, thickness))
}

// ---------------------------------------------------------------------------
// 探测入口
// ---------------------------------------------------------------------------

/// 一次探测的原始观测值，只为把诊断日志一次打全。
struct Observation {
    hosted: HostedLookup,
    appkit: Option<(CGRect, CGRect, CGFloat)>,
}

fn observe(mtm: MainThreadMarker) -> Observation {
    Observation {
        hosted: probe_hosted_extra(),
        appkit: probe_appkit(mtm),
    }
}

/// 把观测值折成结论，并把完整诊断打进日志。
fn conclude(obs: &Observation) -> TrayPlacement {
    let placement = match obs.hosted {
        // 控制中心确实为我们建了窗口，这是「图标真的在菜单栏上」的直接证据。
        HostedLookup::Found => TrayPlacement::Placed,
        HostedLookup::NotFound => TrayPlacement::Rejected,
        // 主判据不可用时退到影子窗口的几何。拿不到影子窗口就只能认输报 Unknown,
        // 绝不能默认成 Rejected——那会在判据失效时向每个用户误报。
        HostedLookup::Unreadable => match obs.appkit {
            Some((item, screen, _)) => {
                if looks_parked(item, screen) {
                    TrayPlacement::Rejected
                } else {
                    TrayPlacement::Placed
                }
            }
            None => TrayPlacement::Unknown,
        },
    };

    let geom = obs.appkit.map(|(item, screen, thickness)| {
        format!(
            "shadow={:?} screen={:?} thickness={} parked={}",
            item,
            screen,
            thickness,
            looks_parked(item, screen)
        )
    });

    match placement {
        TrayPlacement::Placed => log::info!(
            "Menu bar icon is hosted by Control Center (autosave name {:?}). hosted={:?} {}",
            TRAY_AUTOSAVE_NAME,
            obs.hosted,
            geom.as_deref().unwrap_or("shadow=<none>"),
        ),
        TrayPlacement::Rejected => log::warn!(
            "Menu bar icon was NOT given a slot by the system: Control Center hosts no window \
             named {:?} at layer {}. hosted={:?} {}. \
             The status item itself was created fine (menu and click handlers work), \
             so this is a system-side placement failure, not a tray construction error. \
             Most likely some OTHER app's menu bar switch is off and this app is listed \
             under it (Control Center attributes menu bar items to the responsible \
             process; launching the binary from an IDE terminal attributes it to that IDE). \
             See docs/USER_GUIDE.md for the user-facing recovery steps.",
            TRAY_AUTOSAVE_NAME,
            MENU_BAR_WINDOW_LAYER,
            obs.hosted,
            geom.as_deref().unwrap_or("shadow=<none>"),
        ),
        TrayPlacement::Unknown => log::warn!(
            "Tray placement is indeterminate: window names at layer {} are unreadable and \
             no shadow window was found. Criteria are unusable on this system; \
             no user-facing guidance will be shown.",
            MENU_BAR_WINDOW_LAYER,
        ),
    }

    placement
}

/// 探测一次。结果同时写进进程内缓存，供 `last_placement()` 同步读取。
///
/// 必须 await：取数用的全是 NSWindow / NSScreen 系 API，只能在主线程跑，
/// 这里经 `run_on_main_thread` 派发之后等回执（与 `notification_macos` 里
/// delegate 回调派发主线程是同一套理由）。
pub async fn probe(app: &AppHandle) -> TrayPlacement {
    let (tx, rx) = tokio::sync::oneshot::channel();

    // 应用退出过程中派发会失败。这里必须容错返回 Unknown 而不是 unwrap 崩溃——
    // 一个可见性提示没有任何理由把进程带走。
    if let Err(e) = app.run_on_main_thread(move || {
        let result = match MainThreadMarker::new() {
            Some(mtm) => conclude(&observe(mtm)),
            None => {
                log::warn!("run_on_main_thread closure is not on the main thread; skipping probe");
                TrayPlacement::Unknown
            }
        };
        let _ = tx.send(result);
    }) {
        log::warn!("Failed to dispatch tray placement probe to main thread: {}", e);
        return TrayPlacement::Unknown;
    }

    let placement = rx.await.unwrap_or(TrayPlacement::Unknown);
    LAST_PLACEMENT.store(placement.to_cache(), Ordering::Relaxed);
    placement
}

// ---------------------------------------------------------------------------
// 判定的纯逻辑（可单测）
// ---------------------------------------------------------------------------

/// 被拒时是否要给用户看得见的提示。
///
/// `Unknown` 一律不提示：判据失效时宁可静默，也不要甩给用户一个查不出所以然的告警。
pub(crate) fn should_warn_user(placement: TrayPlacement) -> bool {
    matches!(placement, TrayPlacement::Rejected)
}

/// 是否要无视 `start_minimized` 强行唤起窗口。
///
/// 被拒 + 最小化启动 = 菜单栏没有、窗口没有，唯一入口只剩 Dock 图标，
/// 而那要求用户先意识到「应用其实在跑」——恰恰是他此刻没有的那条信息。
pub(crate) fn should_force_reveal(
    placement: TrayPlacement,
    start_minimized: bool,
    opted_out: bool,
) -> bool {
    matches!(placement, TrayPlacement::Rejected) && start_minimized && !opted_out
}

// ---------------------------------------------------------------------------
// 持久化标记
// ---------------------------------------------------------------------------

async fn read_flag(app: &AppHandle, key: &str) -> bool {
    let state = app.state::<AppState>();
    let conn = state.db_conn.lock().await;
    db::get_local_flag(&conn, key).is_some()
}

async fn write_flag(app: &AppHandle, key: &str, value: &str) {
    let state = app.state::<AppState>();
    let conn = state.db_conn.lock().await;
    if let Err(e) = db::set_local_flag(&conn, key, value) {
        log::warn!("Failed to persist flag {}: {}", key, e);
    }
}

/// 组装当前载荷（含两个持久化标记）。
async fn build_payload(
    app: &AppHandle,
    placement: TrayPlacement,
    start_minimized: bool,
) -> TrayPlacementPayload {
    TrayPlacementPayload {
        placement,
        start_minimized,
        guidance_shown: read_flag(app, FLAG_GUIDANCE_SHOWN).await,
        force_reveal_optout: read_flag(app, FLAG_FORCE_REVEAL_OPTOUT).await,
    }
}

/// 供前端「拉」通道用：取当前放置状态与两个标记。
///
/// 用的是缓存值而不是现场再探一次：探测要派发主线程，而这个命令会在每次
/// `fetchInitialData` 里被调用；看门狗已经在按固定节奏刷新缓存了。
pub async fn current_payload(app: &AppHandle) -> TrayPlacementPayload {
    let start_minimized = {
        let state = app.state::<AppState>();
        let settings = state.settings.lock().await;
        settings.start_minimized
    };
    build_payload(app, last_placement(), start_minimized).await
}

/// 记下「已经引导过一次」。值里带版本与时间戳纯为排障留痕。
pub async fn mark_guidance_shown(app: &AppHandle) {
    let value = format!("{} @ {}", crate::app_state::APP_VERSION, unix_timestamp());
    write_flag(app, FLAG_GUIDANCE_SHOWN, &value).await;
}

/// 记下用户不想再被自动唤起窗口。
pub async fn mark_force_reveal_optout(app: &AppHandle) {
    write_flag(app, FLAG_FORCE_REVEAL_OPTOUT, "1").await;
}

/// 一个粗粒度时间戳。
///
/// 不为一行排障日志引入 chrono：这个值除了人眼看之外没有任何消费者，
/// 精度到秒的 Unix 时间戳足够定位「哪次启动引导的」。
fn unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

// ---------------------------------------------------------------------------
// watchdog
// ---------------------------------------------------------------------------

/// 首探前的静默期。控制中心要花一点时间才把新的菜单栏项排进去，
/// 3s 同时也是给开机自启留的余量——登录瞬间 SystemUIServer 自己都还没起来。
const FIRST_PROBE_DELAY: Duration = Duration::from_secs(3);
/// 未定论时的重试间隔。
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// 定论前最多重试几次。连续这么多次都不是 Placed 才下结论——
/// 登录瞬间系统负载极高，单次探测定论必然出假阳性。
const MAX_RETRIES: u32 = 6;
/// 定论为被拒之后的复探间隔。用户会中途去系统设置改开关，横幅必须能自己消失。
const RECHECK_INTERVAL: Duration = Duration::from_secs(30);

/// 挂上菜单栏放置看门狗。在 setup 里调用一次。
pub fn spawn_placement_watchdog(app: &AppHandle, start_minimized: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_PROBE_DELAY).await;

        let mut placement = probe(&app).await;
        let mut tries = 1;
        while placement != TrayPlacement::Placed && tries < MAX_RETRIES {
            tokio::time::sleep(RETRY_INTERVAL).await;
            placement = probe(&app).await;
            tries += 1;
        }

        if placement == TrayPlacement::Placed {
            // 已放置的图标不会被系统单方面收回；用户自己拖走或关开关是有可见
            // 反馈的主动操作，不需要我们再告警。看门狗到此退出。
            let payload = build_payload(&app, placement, start_minimized).await;
            let _ = app.emit(PLACEMENT_EVENT, payload);
            return;
        }

        handle_not_placed(&app, placement, start_minimized).await;

        // 复探直到恢复。恢复即推一次「已就位」让横幅自行消失，然后退出。
        loop {
            tokio::time::sleep(RECHECK_INTERVAL).await;
            if probe(&app).await == TrayPlacement::Placed {
                log::info!("Menu bar icon has been placed; clearing the guidance banner");
                let payload = build_payload(&app, TrayPlacement::Placed, start_minimized).await;
                let _ = app.emit(PLACEMENT_EVENT, payload);
                return;
            }
        }
    });
}

/// 定论为「没被放置」之后的处置，按打扰程度递增。
async fn handle_not_placed(app: &AppHandle, placement: TrayPlacement, start_minimized: bool) {
    // 日志在 conclude() 里已经打过，这里只做用户可见的部分。
    let payload = build_payload(app, placement, start_minimized).await;
    let guidance_shown = payload.guidance_shown;
    let force_reveal_optout = payload.force_reveal_optout;

    // 永远推事件：窗口开着的话，横幅立刻就能看到。
    let _ = app.emit(PLACEMENT_EVENT, payload);

    if !should_warn_user(placement) {
        return;
    }

    let mut revealed = false;

    // 仅首次：投一条系统通知 + 唤起窗口。通知覆盖「用户此刻没在看屏幕」，
    // 唤起窗口保证横幅真的被看到。
    if !guidance_shown {
        // 非 bundle 环境（tauri dev 的裸二进制）由 show_notification 内部自行跳过，
        // 复用 running_as_app_bundle 那套判定，不另写一份。
        if let Err(e) = super::notification_macos::show_notification(
            app,
            "瞬贴 菜单栏图标被系统隐藏",
            "系统没有给瞬贴分配菜单栏位置。已为你打开主窗口，请按窗口顶部提示恢复。",
        ) {
            log::warn!("Failed to show menu bar guidance notification: {}", e);
        }
        crate::reveal_main_window(app);
        revealed = true;
        mark_guidance_shown(app).await;
    }

    // 降级保障：被拒 + 最小化启动时，无视该设定把窗口唤起来。
    // **绝不改写用户的配置**——被系统缺陷连累不是改它的理由，改了用户修好权限后
    // 会发现设置莫名变了，且无从追溯。
    if !revealed && should_force_reveal(placement, start_minimized, force_reveal_optout) {
        log::warn!(
            "Tray not placed by the system; ignoring start_minimized this launch \
             so the user keeps an entry point"
        );
        crate::reveal_main_window(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::{CGPoint, CGSize};

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
    }

    /// 本机实测屏幕：macOS 26.6.2，3440x1440 单屏 1:1 无缩放。
    fn screen() -> CGRect {
        rect(0.0, 0.0, 3440.0, 1440.0)
    }

    /// 实测的第一组被拒坐标（y 为负）。
    #[test]
    fn parked_shadow_with_negative_y_is_detected() {
        assert!(looks_parked(rect(3405.0, -1.0, 35.0, 24.0), screen()));
    }

    /// 实测的第二组被拒坐标（y 落在菜单栏带内）。
    ///
    /// **这组是第一版判据的反例**：按「上沿够不够得到菜单栏带」判，
    /// 1418 + 22 = 1440 完全齐平，会被判成已放置——而它其实压在时钟底下。
    /// 两组的共同点只有 x + width == 屏幕宽度，判据必须落在这上面。
    #[test]
    fn parked_shadow_inside_the_menu_bar_band_is_still_detected() {
        assert!(looks_parked(rect(3406.0, 1418.0, 34.0, 22.0), screen()));
    }

    /// 真正获得槽位的项不会顶到屏幕右边缘——时钟永远在最右。
    /// 坐标参照同机对照应用实测到的托管窗口位置。
    #[test]
    fn a_real_slot_is_not_parked() {
        assert!(!looks_parked(rect(2883.0, 1418.0, 79.0, 22.0), screen()));
    }

    #[test]
    fn unknown_never_warns_the_user() {
        assert!(!should_warn_user(TrayPlacement::Unknown));
        assert!(!should_warn_user(TrayPlacement::Placed));
        assert!(should_warn_user(TrayPlacement::Rejected));
    }

    /// 强制唤起的真值表。
    #[test]
    fn force_reveal_truth_table() {
        // 被拒 + 最小化 + 没退出过 → 唤起
        assert!(should_force_reveal(TrayPlacement::Rejected, true, false));
        // 用户明确退出过 → 尊重他的选择
        assert!(!should_force_reveal(TrayPlacement::Rejected, true, true));
        // 本来就会显示窗口 → 无需额外动作
        assert!(!should_force_reveal(TrayPlacement::Rejected, false, false));
        // 图标正常 → 与本机制无关
        assert!(!should_force_reveal(TrayPlacement::Placed, true, false));
        // 判据失效 → 一律不动用户的窗口
        assert!(!should_force_reveal(TrayPlacement::Unknown, true, false));
    }

    #[test]
    fn placement_cache_roundtrips() {
        for p in [
            TrayPlacement::Placed,
            TrayPlacement::Rejected,
            TrayPlacement::Unknown,
        ] {
            assert_eq!(TrayPlacement::from_cache(p.to_cache()), p);
        }
        // 从未探测过时对外是 Unknown，而不是乐观的 Placed
        assert_eq!(
            TrayPlacement::from_cache(CACHE_UNSET),
            TrayPlacement::Unknown
        );
    }

    /// autosaveName 是自检的锚点，`lib.rs::build_tray` 用同一个常量设置它。
    /// 钉住它防止有人只改一边。
    #[test]
    fn autosave_name_is_stable() {
        assert_eq!(TRAY_AUTOSAVE_NAME, "UniDrop");
        assert!(!TRAY_AUTOSAVE_NAME.is_empty());
    }
}
