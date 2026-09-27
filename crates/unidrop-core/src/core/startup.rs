//! 启动形态判定（无副作用的纯函数，便于单测）
//!
//! 自启注册项永远带 `--silent`，它只表示「本次是系统拉起的、不是用户主动点的」。
//! 主窗口是否显示由用户配置 `start_minimized` 决定，与拉起方式无关——
//! 两个开关必须能自由组合，否则 `start_minimized` 就成了自启的子选项。

/// 自启注册项携带的标记参数
pub const AUTOSTART_FLAG: &str = "--silent";

/// 启动参数中是否含自启标记
pub fn is_autostart_launch<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().any(|a| a.as_ref() == AUTOSTART_FLAG)
}

/// 冷启动时是否显示主窗口：只取决于用户配置，手动启动同样遵从
pub fn should_show_on_launch(start_minimized: bool) -> bool {
    !start_minimized
}

/// 第二实例是否应唤起并抢焦点：用户点的要抢，系统拉起的不抢
pub fn should_focus_second_instance<S: AsRef<str>>(args: &[S]) -> bool {
    !is_autostart_launch(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // --- 真值表第 1、2 行：冷启动只看配置 ---

    #[test]
    fn cold_launch_shows_window_when_not_minimized() {
        assert!(should_show_on_launch(false));
    }

    #[test]
    fn cold_launch_hides_window_when_minimized() {
        assert!(!should_show_on_launch(true));
    }

    // --- 真值表第 3、4 行：第二实例看 --silent ---

    #[test]
    fn second_instance_from_user_takes_focus() {
        assert!(should_focus_second_instance(&args(&[
            "/Applications/UniDrop.app/Contents/MacOS/UniDrop"
        ])));
    }

    #[test]
    fn second_instance_from_autostart_keeps_window_state() {
        assert!(!should_focus_second_instance(&args(&["UniDrop.exe", "--silent"])));
    }

    // --- 边界 ---

    #[test]
    fn empty_args_is_not_autostart() {
        let empty: [String; 0] = [];
        assert!(!is_autostart_launch(&empty));
        assert!(should_focus_second_instance(&empty));
    }

    #[test]
    fn flag_is_detected_anywhere_in_args() {
        assert!(is_autostart_launch(&args(&["exe", "--silent", "--other"])));
        assert!(is_autostart_launch(&args(&["exe", "--other", "--silent"])));
    }

    #[test]
    fn similar_args_are_not_mistaken_for_the_flag() {
        // 前缀/后缀/大小写/少一个横杠都不算，避免把用户参数误判成自启
        assert!(!is_autostart_launch(&args(&["exe", "--silently"])));
        assert!(!is_autostart_launch(&args(&["exe", "--no-silent"])));
        assert!(!is_autostart_launch(&args(&["exe", "--SILENT"])));
        assert!(!is_autostart_launch(&args(&["exe", "-silent"])));
        assert!(!is_autostart_launch(&args(&["exe", "silent"])));
    }

    #[test]
    fn accepts_str_slices_too() {
        assert!(is_autostart_launch(&["exe", "--silent"]));
    }
}
