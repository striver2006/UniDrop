//! 缓存保留策略的唯一事实源
//!
//! 此前这些阈值散落成「常量 + SQL 字面量」两套：`DEFAULT_TTL` 与
//! `CLIPBOARD_LOCK_DURATION` 定义了却从未被引用（`pub const` 不触发 dead_code
//! 告警），真正决定行为的是 SQL 里的裸数字 `86400` / `7200`，清理间隔的 `3600`
//! 又直接写在调度循环里。谁想调 TTL，最自然的动作是改常量——改完毫无效果，
//! 且没有任何编译警告。本模块就是为消灭这个陷阱而存在的。

use std::time::Duration;

use crate::commands::settings_cmd::AppSettings;

/// 缓存文件默认保留小时数
pub const DEFAULT_CACHE_TTL_HOURS: u32 = 24;

/// 缓存总量默认上限（MB），10240 MB = 10 GB，与改造前行为一致
pub const DEFAULT_CACHE_MAX_SIZE_MB: u32 = 10 * 1024;

/// 默认清理间隔（分钟）
pub const DEFAULT_SWEEP_INTERVAL_MINUTES: u32 = 60;

/// 清理间隔下限。**不允许 0**：`tokio::time::interval` / `sleep` 收到
/// `Duration::ZERO` 会让循环退化为忙等（`interval` 更是直接 panic）。
/// 而「不清理」已经可以用 ttl=0 且 max_size=0 表达，不需要第二条通路。
pub const MIN_SWEEP_INTERVAL_MINUTES: u32 = 1;

/// 剪贴板免疫窗口（秒）。
///
/// **不开放配置**：这不是用户偏好而是正确性保障——文件还被系统剪贴板引用时删掉，
/// 粘贴就会失败。用户调小它等于给自己制造 bug。
pub const CLIPBOARD_LOCK_SECS: i64 = 7200;

/// LRU 触发后删到上限的百分之多少为止。
///
/// 留出水位差是必要的：若删到刚好等于上限就停，下一个文件写入立刻又超限，
/// 每轮 sweep 都在删——LRU 的抖动。
pub const LRU_LOW_WATERMARK_PERCENT: u64 = 80;

/// 一次清理使用的保留策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// 保留小时数；`0` = 不按时间清理
    pub ttl_hours: u32,
    /// 容量上限（字节）；`0` = 不限容量
    pub max_size_bytes: u64,
}

impl RetentionPolicy {
    /// 从设置构造，**唯一的换算入口**。
    ///
    /// `max_size_bytes` 必须是 `u64`，且换算必须**先升位再乘**。按 `u32` 相乘的话：
    ///
    /// - 默认值 10240 MB = 10,737,418,240 字节，已超 `u32::MAX`(4,294,967,295)：
    ///   debug 构建溢出 panic，release 构建回绕成 ~2 GiB，低水位随之错到 ~1.7 GiB，
    ///   LRU 会大规模删除用户本想保留的缓存——而且是在用户什么都没改的情况下；
    /// - 更糟的是 4096 MB(4 GB)、8192 MB(8 GB)、1048576 MB(1 TB) 恰好回绕成 **0**，
    ///   落入「0 = 不限容量」，语义 180 度反转。而 4 GB、8 GB 正是用户最可能填的值。
    ///
    /// 把换算收敛在这里，调用方就没有机会各自相乘。
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            ttl_hours: settings.cache_ttl_hours,
            max_size_bytes: (settings.cache_max_size_mb as u64) * 1024 * 1024,
        }
    }

    /// LRU 删除的目标水位（字节）。`max_size_bytes` 为 0 时无意义，调用方应先判空。
    ///
    /// 先除后乘是防御性的：即使日后上限被放宽到接近 `u64::MAX`，也不会在乘 80 时
    /// 溢出。代价是整数截断带来最多 99 字节的偏差（1 TB 上限时实测差 60 字节），
    /// 对「删到多少为止」这个判断毫无影响。
    pub fn low_watermark_bytes(&self) -> u64 {
        self.max_size_bytes / 100 * LRU_LOW_WATERMARK_PERCENT
    }

    /// TTL 段是否启用
    pub fn ttl_enabled(&self) -> bool {
        self.ttl_hours > 0
    }

    /// 容量段是否启用
    pub fn quota_enabled(&self) -> bool {
        self.max_size_bytes > 0
    }

    /// TTL 对应的秒数，用于 SQL 参数绑定
    pub fn ttl_seconds(&self) -> i64 {
        self.ttl_hours as i64 * 3600
    }
}

/// 本次生效的清理间隔。
///
/// **钳制的唯一入口**：保存设置时用它规范化落库值，调度循环每轮用它取间隔。
/// 两处指向同一函数，既让「后端校验」名实相符，也让这条规则可以被单测直接打——
/// 内联在 `lib.rs` 的 spawn 闭包里是测不到的。
pub fn effective_sweep_interval(settings: &AppSettings) -> Duration {
    Duration::from_secs(effective_sweep_interval_minutes(settings) as u64 * 60)
}

/// 规范化后的间隔分钟数。落库前用它，保证存进去的就是合法值。
pub fn effective_sweep_interval_minutes(settings: &AppSettings) -> u32 {
    settings
        .cache_sweep_interval_minutes
        .max(MIN_SWEEP_INTERVAL_MINUTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(ttl: u32, mb: u32, interval: u32) -> AppSettings {
        AppSettings {
            cache_ttl_hours: ttl,
            cache_max_size_mb: mb,
            cache_sweep_interval_minutes: interval,
            ..AppSettings::default_config()
        }
    }

    // ---------- 换算宽度：本轮最危险的一条 ----------

    /// 默认值就会超出 u32。按 u32 相乘 debug 会 panic、release 回绕成 ~2GiB。
    #[test]
    fn default_size_exceeds_u32_and_must_not_wrap() {
        let p = RetentionPolicy::from_settings(&settings_with(24, DEFAULT_CACHE_MAX_SIZE_MB, 60));
        assert_eq!(p.max_size_bytes, 10_737_418_240);
        assert!(p.max_size_bytes > u32::MAX as u64, "默认值本就超出 u32 范围");
    }

    /// 4096 的整数倍按 u32 相乘会回绕成 0，恰好落入「0 = 不限容量」，语义反转。
    /// 4 GB 与 8 GB 正是用户最可能填的整数值，这条守的就是它们。
    #[test]
    fn sizes_that_would_wrap_to_zero_are_preserved() {
        for (mb, expected) in [
            (4096u32, 4_294_967_296u64),
            (8192, 8_589_934_592),
            (1_048_576, 1_099_511_627_776),
        ] {
            let p = RetentionPolicy::from_settings(&settings_with(24, mb, 60));
            assert_eq!(p.max_size_bytes, expected, "{} MB 换算错误", mb);
            assert!(p.quota_enabled(), "{} MB 不该被当成「不限容量」", mb);
        }
    }

    #[test]
    fn zero_size_means_unlimited() {
        let p = RetentionPolicy::from_settings(&settings_with(24, 0, 60));
        assert_eq!(p.max_size_bytes, 0);
        assert!(!p.quota_enabled());
    }

    #[test]
    fn zero_ttl_disables_time_based_cleanup() {
        let p = RetentionPolicy::from_settings(&settings_with(0, 10240, 60));
        assert!(!p.ttl_enabled(), "0 小时表示不按时间清理，而非立即删光");
    }

    #[test]
    fn ttl_seconds_conversion() {
        assert_eq!(RetentionPolicy::from_settings(&settings_with(24, 1, 60)).ttl_seconds(), 86_400);
        assert_eq!(RetentionPolicy::from_settings(&settings_with(1, 1, 60)).ttl_seconds(), 3_600);
    }

    #[test]
    fn low_watermark_is_80_percent() {
        let p = RetentionPolicy::from_settings(&settings_with(24, 100, 60));
        assert_eq!(p.max_size_bytes, 104_857_600);
        assert_eq!(p.low_watermark_bytes(), 83_886_080);
    }

    /// 大值下低水位同样不能溢出
    #[test]
    fn low_watermark_survives_large_values() {
        let p = RetentionPolicy::from_settings(&settings_with(24, 1_048_576, 60));
        // 先除后乘的截断结果，比精确值 879_609_302_220 少 60 字节，是预期行为
        assert_eq!(p.low_watermark_bytes(), 879_609_302_160);
    }

    // ---------- 间隔钳制：内联在调度闭包里就测不到，所以提成了纯函数 ----------

    #[test]
    fn interval_zero_is_clamped_to_minimum() {
        assert_eq!(effective_sweep_interval_minutes(&settings_with(24, 10240, 0)), 1);
        assert_eq!(
            effective_sweep_interval(&settings_with(24, 10240, 0)),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn interval_zero_never_yields_zero_duration() {
        // Duration::ZERO 会让调度循环退化成忙等
        assert!(!effective_sweep_interval(&settings_with(24, 10240, 0)).is_zero());
    }

    #[test]
    fn normal_interval_passes_through() {
        assert_eq!(effective_sweep_interval_minutes(&settings_with(24, 10240, 60)), 60);
        assert_eq!(
            effective_sweep_interval(&settings_with(24, 10240, 60)),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn interval_one_is_allowed() {
        assert_eq!(effective_sweep_interval_minutes(&settings_with(24, 10240, 1)), 1);
    }
}
