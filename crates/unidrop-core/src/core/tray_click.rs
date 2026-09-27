//! 托盘图标左键点击的行为判定（无副作用的纯函数，便于单测）
//!
//! 这里只放判据，真正的窗口操作在 `lib.rs::build_tray` 的 `on_tray_icon_event` 里。
//! 拆出来的理由和 [`crate::core::startup`] 一样：托盘点击只能靠手点验证，
//! 每次回归都要重新构建 .app 再 `open`（见 `lib.rs::build_tray` 的文档，
//! 开发期直接跑裸二进制会被控制中心连坐拒绝放置），成本高到不适合当唯一的验证手段。

use std::time::Duration;

/// 双击去抖窗口：这段时间内的第二次左键点击会被忽略。
///
/// 托盘的左键是**切换**语义（显示 ⇄ 收起），所以一次双击会连着切两回、
/// 净效果归零——用户看到的就是「双击打不开窗口」，这正是本次修复前的实际症状之一。
/// 去抖因此不是手感优化，是切换语义下的必需品。
///
/// 刻意用常量而不去读 `NSEvent.doubleClickInterval`（系统默认 500ms，可调）：
/// 读它要多背一个 AppKit 调用和主线程约束，而这个值偏差一点只影响双击手感，
/// 单击行为完全不受影响。400ms 略小于系统默认值，避免把用户「开一下、看一眼、
/// 再关掉」的正常两次单击也吞掉。
pub const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// 左键点击时是该收起窗口（true）还是该唤起窗口（false）。
///
/// **三个判据缺一不可，尤其是 `focused`。** 只看 `visible && !minimized` 是不够的：
/// 用户打开窗口后去点了别的应用，窗口仍然是 visible、也没有 minimized，
/// 只是被盖在后面。此时点托盘图标，用户的意图显然是「把它拿到前面来」，
/// 而旧判据会把它 `hide()` 掉——屏幕上本来就看不见它，于是表现成「点了没反应」，
/// 必须再点第二次才出得来。
///
/// `minimized` 那一条则是给 Windows 留的：「最小化到任务栏」时窗口仍是 visible，
/// 当作「已经在眼前」会导致点图标反而把它藏进托盘，还原不回来。
pub fn should_hide_on_tray_click(visible: bool, minimized: bool, focused: bool) -> bool {
    visible && !minimized && focused
}

/// 距上次生效的左键动作是否近到该判为同一次双击。
///
/// `None` 表示本进程还没有过左键动作，必然不是重复。
///
/// 被判为重复的那一次**不应该**去更新「上次动作时间」（调用方保证）：
/// 去抖取的是 leading edge，连点三下四下也只该有第一下生效；
/// 若每次都刷新时间戳，快速连点会变成一直被吞，反而更难预期。
pub fn is_duplicate_left_click(elapsed: Option<Duration>) -> bool {
    matches!(elapsed, Some(d) if d < DOUBLE_CLICK_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- 显隐判据真值表：八行全覆盖 ---

    #[test]
    fn hides_only_when_window_is_actually_in_front() {
        // 唯一该收起的组合：看得见、没最小化、正是当前活动窗口
        assert!(should_hide_on_tray_click(true, false, true));
    }

    #[test]
    fn reveals_when_visible_but_not_focused() {
        // 本次修复的主场景：用户点了别的应用，窗口被盖住但仍算 visible
        assert!(!should_hide_on_tray_click(true, false, false));
    }

    #[test]
    fn reveals_when_minimized() {
        // 最小化到任务栏：窗口仍是 visible，但要还原而不是藏起来
        assert!(!should_hide_on_tray_click(true, true, false));
        assert!(!should_hide_on_tray_click(true, true, true));
    }

    #[test]
    fn reveals_when_hidden() {
        // 已经 hide() 过的窗口，四种剩余组合一律唤起
        assert!(!should_hide_on_tray_click(false, false, false));
        assert!(!should_hide_on_tray_click(false, false, true));
        assert!(!should_hide_on_tray_click(false, true, false));
        assert!(!should_hide_on_tray_click(false, true, true));
    }

    // --- 去抖判据边界 ---

    #[test]
    fn first_click_is_never_duplicate() {
        assert!(!is_duplicate_left_click(None));
    }

    #[test]
    fn second_click_within_window_is_duplicate() {
        assert!(is_duplicate_left_click(Some(Duration::ZERO)));
        assert!(is_duplicate_left_click(Some(
            DOUBLE_CLICK_WINDOW - Duration::from_millis(1)
        )));
    }

    #[test]
    fn click_at_or_after_window_is_not_duplicate() {
        // 边界取半开区间 [0, WINDOW)：正好等于窗口长度的算新动作
        assert!(!is_duplicate_left_click(Some(DOUBLE_CLICK_WINDOW)));
        assert!(!is_duplicate_left_click(Some(
            DOUBLE_CLICK_WINDOW + Duration::from_millis(1)
        )));
        assert!(!is_duplicate_left_click(Some(Duration::from_secs(10))));
    }
}
