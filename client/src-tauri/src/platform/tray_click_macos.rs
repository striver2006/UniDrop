//! macOS 菜单栏托盘图标的原生点击处理。
//!
//! # 背景与机制
//!
//! 1. 跨平台托盘库（tray-icon 0.24.2）在设置了 menu 且 show_menu_on_left_click(false) 时，
//!    把 menu 常驻设在 `NSStatusItem.menu` 上，并试图用一个覆盖在 `NSStatusBarButton` 上的
//!    普通 `NSView`（`TaoTrayTarget`）来拦截鼠标事件。
//! 2. 在 macOS 11+ 以及 macOS 26 Tahoe（菜单栏项由「控制中心」托管）下：
//!    - 普通 `NSView` 默认 `acceptsFirstMouse: false`，后台/Accessory 模式下首次点击会被丢弃；
//!    - 屏幕上的菜单栏点击实际落在控制中心窗口上，控制中心把带有常驻 menu 的项视为下拉菜单专项目，
//!      直接触发系统级菜单展示或内部 Accessibility 操作，根本不会向应用进程的子视图派发 `mouseDown:`。
//! 3. 原生实现方案：
//!    - 保留由 Tauri/muda 构造好的 `NSMenu`，但通过 `status_item.setMenu(None)` 清除常驻绑定；
//!    - 清理 `NSStatusBarButton` 上 tray-icon 添加的 `TaoTrayTarget` 子视图；
//!    - 配置 `button.setTarget` 与 `button.setAction`，并监听 `LeftMouseUp | RightMouseUp`；
//!    - 当触发 action 时，通过 `currentEvent` 区分左键与右键：
//!      - 右键（或 Control + 单击）：调用 `status_item.popUpStatusItemMenu(menu)` 原生原地弹出菜单；
//!      - 左键：调用 `crate::handle_tray_left_click(&app_handle)` 唤起或切换主窗口。

use std::sync::OnceLock;

use objc2_app_kit_v06::{
    NSEventMask, NSEventModifierFlags, NSEventType, NSApplication, NSMenu, NSStatusBarButton,
    NSStatusItem,
};
use objc2_foundation_v06::{NSObject, NSObjectProtocol};
use objc2_v06::rc::Retained;
use objc2_v06::{define_class, sel, AnyThread, MainThreadMarker};

/// 允许将仅在 AppKit 主线程运行的 Objective-C 对象安全存储在静态 OnceLock 中。
struct MainThreadCell<T>(T);
unsafe impl<T> Send for MainThreadCell<T> {}
unsafe impl<T> Sync for MainThreadCell<T> {}

impl<T> MainThreadCell<T> {
    fn new(val: T) -> Self {
        Self(val)
    }
    fn get(&self) -> &T {
        &self.0
    }
}

/// 持久保存的上下文，供原生回调使用。
struct TrayContext {
    on_left_click: Box<dyn Fn() + Send + Sync + 'static>,
    status_item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
}

static TRAY_CONTEXT: OnceLock<MainThreadCell<TrayContext>> = OnceLock::new();
static TRAY_HANDLER: OnceLock<MainThreadCell<Retained<UniDropTrayClickHandler>>> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "UniDropTrayClickHandler"]
    struct UniDropTrayClickHandler;

    unsafe impl NSObjectProtocol for UniDropTrayClickHandler {}

    impl UniDropTrayClickHandler {
        #[unsafe(method(onTrayClick:))]
        fn on_tray_click(&self, sender: &NSStatusBarButton) {
            let Some(cell) = TRAY_CONTEXT.get() else {
                return;
            };
            let ctx = cell.get();

            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };

            let app = NSApplication::sharedApplication(mtm);
            let event = app.currentEvent();

            let is_right_click = match &event {
                Some(ev) => {
                    let ev_type = ev.r#type();
                    let is_right = ev_type == NSEventType::RightMouseUp;
                    let is_ctrl_left = ev_type == NSEventType::LeftMouseUp
                        && ev.modifierFlags().contains(NSEventModifierFlags::Control);
                    let is_button_two = ev.buttonNumber() == 1;
                    is_right || is_ctrl_left || is_button_two
                }
                None => false,
            };

            if is_right_click {
                log::info!("macOS native tray click: right click detected, popping up menu");
                #[allow(deprecated)]
                ctx.status_item.popUpStatusItemMenu(&ctx.menu);
                sender.highlight(false);
            } else {
                log::info!("macOS native tray click: left click detected, handling reveal/toggle");
                (ctx.on_left_click)();
            }
        }
    }
);

/// 在 macOS 上初始化原生状态栏按钮的 target-action 监听。
pub(crate) fn setup_tray_macos<F>(
    status_item: &Retained<NSStatusItem>,
    on_left_click: F,
) where
    F: Fn() + Send + Sync + 'static,
{
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("setup_tray_macos: failed to obtain MainThreadMarker");
        return;
    };

    let menu = status_item.menu(mtm);
    let Some(menu) = menu else {
        log::warn!("setup_tray_macos: status item has no menu attached");
        return;
    };

    // 1. 移除常驻 menu，使控制中心与 AppKit 将其视为普通按钮而非下拉菜单专用项
    status_item.setMenu(None);

    // 2. 初始化单例 Handler
    let handler_cell = TRAY_HANDLER.get_or_init(|| {
        let alloc = UniDropTrayClickHandler::alloc();
        let obj: Retained<UniDropTrayClickHandler> = unsafe { objc2_v06::msg_send![alloc, init] };
        MainThreadCell::new(obj)
    });
    let handler = handler_cell.get();

    // 3. 配置 button
    if let Some(button) = status_item.button(mtm) {
        // 清理 tray-icon 添加的 TaoTrayTarget 子视图，恢复原生的命中测试与 acceptsFirstMouse
        let subviews = button.subviews();
        for i in 0..subviews.count() {
            let subview = subviews.objectAtIndex(i);
            subview.removeFromSuperview();
        }

        unsafe {
            button.setTarget(Some(&**handler));
            button.setAction(Some(sel!(onTrayClick:)));
            // 将菜单挂载为按钮的上下文菜单（右键自动触发原生弹出）
            button.setMenu(Some(&menu));
        }
        button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        log::info!("macOS native tray click handler installed successfully");
    } else {
        log::warn!("setup_tray_macos: status item button is None");
    }

    // 4. 保存上下文供点击时使用
    let _ = TRAY_CONTEXT.set(MainThreadCell::new(TrayContext {
        on_left_click: Box::new(on_left_click),
        status_item: status_item.clone(),
        menu,
    }));
}
