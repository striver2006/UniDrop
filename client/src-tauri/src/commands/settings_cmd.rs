use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt;
use crate::app_state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub server_url: String,
    pub account_id: String,
    pub psk_secret: String,
    pub auto_inject: bool,
    /// 启动时不弹出主窗口，仅在托盘常驻。对手动启动与开机自启同样生效。
    ///
    /// `#[serde(default)]` 不可删除：整份设置以一条 JSON 存在 SQLite 的
    /// `local_config` 表里，老库的 JSON 没有本字段。缺了 default，
    /// 反序列化会整条失败并走 `lib.rs` 的 `unwrap_or_else` 静默回落到全部默认值，
    /// 把用户已配置的 server_url / account_id / psk_secret 一起冲掉。
    #[serde(default)]
    pub start_minimized: bool,

    /// 传输历史最多保留的条数，超出的最旧记录连同缓存文件一起删除；`0` = 不限制。
    ///
    /// **不能只写 `#[serde(default)]`**：`u32` 的 `Default` 是 `0`，而 `0` 在这里
    /// 被定义成「不限制」。老库的 JSON 没有本字段，裸 default 会让升级后的用户
    /// 静默变成「历史无上限」——恰好是需求没生效的形态，而设置面板里显示的 `0`
    /// 看起来又像是用户自己设的，无从分辨。
    #[serde(default = "default_history_max_entries")]
    pub history_max_entries: u32,

    /// 传输**完成**的卡片在界面上保持的秒数；`0` = 不自动消失。
    ///
    /// 只管 COMPLETED。FAILED 卡片永不自动消失——失败原因的 toast 只显示 4 秒，
    /// 卡片再自动消失就没有任何入口能看到为什么失败了。
    ///
    /// 同样需要自定义 default，理由见 `history_max_entries`。
    #[serde(default = "default_transfer_card_retain_secs")]
    pub transfer_card_retain_secs: u32,

    /// 磁盘缓存文件保留小时数；`0` = 不按时间清理。
    ///
    /// 同样必须用自定义 default：`0` 在这里是「不清理」，裸 `#[serde(default)]`
    /// 会让老库升级后静默变成永不按时间清理，磁盘被无声占满。
    #[serde(default = "default_cache_ttl_hours")]
    pub cache_ttl_hours: u32,

    /// 磁盘缓存总量上限（MB）；`0` = 不限容量。
    #[serde(default = "default_cache_max_size_mb")]
    pub cache_max_size_mb: u32,

    /// 允许不安全的 TLS 连接：跳过服务器证书校验。
    ///
    /// 需要它的部署有三类：自签证书、直连 IP（证书上没有对应名字）、
    /// 以及证书由**非公共 CA**（企业内部 CA）签发——webpki 根存储不含后者，
    /// 所以合法的内部证书同样过不了默认校验。
    ///
    /// 这里用裸 `#[serde(default)]` 是对的，与上面几个字段相反：
    /// `bool` 的 `Default` 是 `false`，而 `false` 恰好是安全值，
    /// 老库升级后默认变成「校验证书」。上面那些字段之所以要自定义 default，
    /// 是因为它们的零值 `0` 被定义成「关闭限制」——不安全的那一侧。
    #[serde(default)]
    pub allow_insecure_tls: bool,

    /// 是否对传输内容做端到端加密。
    ///
    /// **这里必须写自定义 default，不能用裸 `#[serde(default)]`——与紧邻的
    /// `allow_insecure_tls` 恰好相反。** 那个字段的安全值是 `false`，正好等于
    /// `bool::default()`；而这个字段的安全值是 `true`，裸 default 会让所有
    /// 老库升级后静默关闭加密，且没有任何迹象。两个相邻的 bool 取了相反的默认，
    /// 看起来像风格不一致，实际都是「默认值必须落在安全的那一侧」。
    ///
    /// 开启后并不保证每次传输都加密：对端版本过旧时会回落明文并给出可见提示
    /// （见 `core::e2ee::peer_supports_e2ee`）。
    #[serde(default = "default_e2ee_enabled")]
    pub e2ee_enabled: bool,

    /// 后台清理间隔（分钟），最小 1。
    ///
    /// 这个字段的裸 `#[serde(default)]` 后果最严重：`0` 会让调度循环拿到
    /// `Duration::ZERO` 而退化成忙等。取值经 `effective_sweep_interval` 钳制，
    /// 保存时也会规范化，但 default 仍必须给出合法值而非 0。
    #[serde(default = "default_cache_sweep_interval_minutes")]
    pub cache_sweep_interval_minutes: u32,
}

/// E2EE 默认开启。回落机制保证了对端老版本不会因此失败，
/// 而默认关闭等于绝大多数用户永远不会用上它。
fn default_e2ee_enabled() -> bool {
    true
}

/// 历史保留条数的默认值。沿用改造前 `cmd_list_history` 硬编码的 100，
/// 使未改过设置的老用户看到的列表长度保持不变。
pub const DEFAULT_HISTORY_MAX_ENTRIES: u32 = 100;

/// 完成卡片保持秒数的默认值。既有 toast 是 4 秒，够读完但不够点卡片上的
/// 「重新复制 / 装载」按钮；30 秒是「看完并来得及点」的下限。
pub const DEFAULT_TRANSFER_CARD_RETAIN_SECS: u32 = 30;

fn default_history_max_entries() -> u32 {
    DEFAULT_HISTORY_MAX_ENTRIES
}

fn default_transfer_card_retain_secs() -> u32 {
    DEFAULT_TRANSFER_CARD_RETAIN_SECS
}

fn default_cache_ttl_hours() -> u32 {
    crate::core::retention::DEFAULT_CACHE_TTL_HOURS
}

fn default_cache_max_size_mb() -> u32 {
    crate::core::retention::DEFAULT_CACHE_MAX_SIZE_MB
}

fn default_cache_sweep_interval_minutes() -> u32 {
    crate::core::retention::DEFAULT_SWEEP_INTERVAL_MINUTES
}

impl AppSettings {
    /// 首次启动与反序列化失败时的兜底配置（唯一定义点，避免多处字面量漏改）
    pub fn default_config() -> Self {
        Self {
            server_url: "wss://drop.yourdomain.com:58921".to_string(),
            account_id: "default_user".to_string(),
            psk_secret: "dev-insecure-psk-secret".to_string(),
            auto_inject: false,
            start_minimized: false,
            history_max_entries: DEFAULT_HISTORY_MAX_ENTRIES,
            transfer_card_retain_secs: DEFAULT_TRANSFER_CARD_RETAIN_SECS,
            cache_ttl_hours: crate::core::retention::DEFAULT_CACHE_TTL_HOURS,
            cache_max_size_mb: crate::core::retention::DEFAULT_CACHE_MAX_SIZE_MB,
            cache_sweep_interval_minutes: crate::core::retention::DEFAULT_SWEEP_INTERVAL_MINUTES,
            allow_insecure_tls: false,
            e2ee_enabled: true,
        }
    }
}

/// 账号标识的合法性校验，规则必须与服务端 `auth/identity.go` 保持一致：
/// 1..=64 字节的 `[A-Za-z0-9._@-]`。
///
/// 为什么客户端也要校验一遍：设置面板不是唯一的写入方。`default_config()`
/// 与老库里反序列化出来的 JSON 都会直接流进 `config_actor`（见 lib.rs），
/// 不经过面板那一层。仓库里已有同形态的先例——`cache_sweep_interval_minutes`
/// 就是前后端各校验一次。
///
/// 两边不一致时的表现是：客户端放行、服务端拒绝，用户看到顶部红色横幅。
/// 也就是说这一层是体验优化，服务端那一层才是约束。
pub fn validate_account_id(s: &str) -> Result<(), String> {
    if s.is_empty() || s.len() > 64 {
        return Err("账号标识不能为空，长度需在 1-64 之间".to_string());
    }
    if !s.bytes().all(|c| {
        c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.' || c == b'@'
    }) {
        return Err("账号标识只能包含字母、数字与 . _ @ -".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn cmd_get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    let s = state.settings.lock().await;
    Ok(s.clone())
}

#[tauri::command]
pub async fn cmd_save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    let mut clean_settings = new_settings;

    // 账号标识：先 trim 再校验。
    // 面板那一层已经拦过一次，这里是兜底——理由见 validate_account_id。
    clean_settings.account_id = clean_settings.account_id.trim().to_string();
    validate_account_id(&clean_settings.account_id)?;

    clean_settings.server_url = clean_settings
        .server_url
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .trim_end_matches('/')
        .to_string();
    if !clean_settings.server_url.is_empty()
        && !clean_settings.server_url.starts_with("ws://")
        && !clean_settings.server_url.starts_with("wss://")
    {
        clean_settings.server_url = format!("wss://{}", clean_settings.server_url);
    }

    // 间隔落库前规范化：非法值（尤其 0）不该被持久化，否则 cmd_get_settings
    // 会把它原样返回给前端，界面显示的和实际生效的对不上。
    // 与调度循环共用同一个函数，「前后端各校验一次」才名实相符。
    clean_settings.cache_sweep_interval_minutes =
        crate::core::retention::effective_sweep_interval_minutes(&clean_settings);

    let json_str = serde_json::to_string(&clean_settings).map_err(|e| e.to_string())?;
    {
        let conn = state.db_conn.lock().await;
        crate::storage::db::save_persisted_settings(&conn, &json_str).map_err(|e| e.to_string())?;
    }

    // 1. Update in-memory settings，顺带记下账号是否真的变了
    let account_changed = {
        let mut s = state.settings.lock().await;
        let changed = s.account_id != clean_settings.account_id;
        *s = clean_settings.clone();
        changed
    };

    // 2. Update dynamic actor config
    {
        let mut cfg = state.config_actor.write().await;
        cfg.server_url = clean_settings.server_url.clone();
        cfg.account_id = clean_settings.account_id.clone();
        cfg.psk_secret = clean_settings.psk_secret.clone();
        cfg.allow_insecure_tls = clean_settings.allow_insecure_tls;
    }

    // 3. Clear online devices from previous server/account and notify frontend
    {
        let mut devs = state.online_devices.lock().await;
        devs.clear();
    }
    let _ = app.emit("devices-updated", Vec::<crate::protocol::OnlineDevice>::new());

    // 4. Trigger immediate actor reconnection with new configuration
    state.reconnect_notify.notify_waiters();
    log::info!("Settings saved and reconnected immediately with new config");

    // 5. 认领无归属的历史行。
    //
    //    与启动时那次是同一个动作，这里再做一次是为了覆盖「首次启动时账号
    //    非法、认领被跳过」的情形：用户改正账号的地方就是这个面板，若只在
    //    启动认领，他改对之后还得重启一次才能看见老历史。
    //
    //    不会把上一个账号的历史搬过来——认领只触 account_id IS NULL 的行，
    //    已有归属的行动不了（claim_is_idempotent_and_does_not_resteal 钉住了
    //    这一点）。账号到这里必然合法：函数开头已经 validate 过并提前返回。
    {
        let conn = state.db_conn.lock().await;
        match crate::storage::db::claim_unowned_history(&conn, &clean_settings.account_id) {
            Ok(0) => {}
            Ok(n) => log::info!("Claimed {} unowned history rows for account {}", n, clean_settings.account_id),
            Err(e) => log::warn!("Failed to claim unowned history rows: {}", e),
        }
    }

    // 6. 账号变了就让界面重拉历史。
    //
    //    不能指望 `history-pruned` 代劳：那是「修剪删掉了东西」的语义，而切账号
    //    时很可能一条都不用删（prune_and_notify 在 Ok(0) 时本就不广播）。
    //    缺了这条事件，历史面板会继续显示上一个账号的卡片——而那些卡片上的
    //    按钮是真的能点的，装载 / 另存为 / 定位三条路径都直接读文件。
    //    后端已有归属闸门兜底（见 history_cmd::ensure_session_owned），
    //    但让用户点到一个必然报错的按钮本身就是缺陷。
    if account_changed {
        let _ = app.emit("account-changed", ());
    }

    // 7. 立刻按新上限修剪历史：用户把条数调小后必须当场见效，
    //    否则要等到下次传输才生效，看起来就像设置没保存。
    //    此处已在上面各锁的作用域之外，取 db_conn 锁是安全的。
    crate::core::history_pruner::prune_and_notify(&app).await;

    Ok(())
}

/// 读取开机自启的**操作系统实时状态**。
///
/// 自启状态的唯一事实源是操作系统（Windows 注册表 Run 键 / macOS LaunchAgent /
/// Linux ~/.config/autostart），不在 AppSettings 里保留副本——用户完全可能绕过
/// 本应用、在系统设置里改它，有副本就必然漂移。
///
/// **注意「存在」不等于「会生效」**：底层 auto-launch 的 `is_enabled()` 只检查
/// 注册项 / plist 是否存在，**不校验其中的路径是否仍指向当前可执行文件**
/// （auto-launch 0.5.0 `windows.rs:73-83`、`macos.rs:161-176`）。因此把应用移到
/// 别处、或先在 dev 二进制上开启自启后再安装正式版，本函数仍会返回 true，
/// 而开机时拉起的是一个失效路径。返回 true 只保证「注册项在」，
/// 不保证「开机真的能起来」。
#[tauri::command]
pub async fn cmd_get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("读取开机自启状态失败: {}", e))
}

/// 设置开机自启。幂等：目标态与 OS 现值一致时直接返回，不触碰系统。
///
/// 幂等是刻意的：macOS 13+ 每次重写 LaunchAgent 都会弹一次「后台项已添加」横幅，
/// 无条件重写会在用户保存任何无关配置时反复打扰。
///
/// 代价是写入后的复查同样只能确认「注册项存在」而非「路径有效」
/// （见 `cmd_get_autostart` 的说明）——要覆盖路径漂移就得强制 disable + enable
/// 重写，而那正好破坏上面这条幂等。本轮选择保幂等。
#[tauri::command]
pub async fn cmd_set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();

    // 比对基准必须是 OS 实时值而不是前端提交时的旧值：用户可能在本次会话中途
    // 于系统设置里改过它，按旧值写回会把用户的操作悄悄覆盖掉。
    let current = manager
        .is_enabled()
        .map_err(|e| format!("读取开机自启状态失败: {}", e))?;
    if current == enabled {
        log::info!("Autostart already {}, skipping OS write", enabled);
        return Ok(());
    }

    if enabled {
        manager
            .enable()
            .map_err(|e| format!("开启开机自启失败: {}", e))?;
    } else {
        manager
            .disable()
            .map_err(|e| format!("关闭开机自启失败: {}", e))?;
    }

    // 复查：让「返回成功」严格等于「OS 里确实是这个状态」
    let applied = manager
        .is_enabled()
        .map_err(|e| format!("校验开机自启状态失败: {}", e))?;
    if applied != enabled {
        return Err(format!(
            "开机自启设置未生效：期望 {}，实际 {}",
            enabled, applied
        ));
    }

    log::info!("Autostart set to {}", enabled);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 老库里的 JSON 没有 start_minimized 字段，却**有**已被删除的
    /// rate_limit_mb 字段。本测试同时守护两个方向：
    ///
    /// - 缺字段：`#[serde(default)]` 删掉的话这里会红，而不是等到用户配置被静默冲掉；
    /// - 多字段：serde 默认忽略未知键，所以删掉 rate_limit_mb 之后老 JSON 仍能读。
    ///
    /// **fixture 里的 rate_limit_mb 是故意留着的，不要"顺手清理"**——
    /// 删掉它这条测试就不再覆盖「多出一个已删字段」这一半了。
    ///
    /// 注意「删字段安全」只对**升级**方向成立。反过来，用户若回滚到旧版客户端，
    /// 新客户端写出的 JSON 里没有 rate_limit_mb，而旧结构体那个字段没有
    /// `#[serde(default)]`，整条反序列化会失败并走 lib.rs 的 unwrap_or_else
    /// 静默回落全部默认值，把 server_url / account_id / psk_secret 一起冲掉。
    /// 本项目发签名 dmg，版本回退是现实场景——降级前应导出设置。
    #[test]
    fn legacy_settings_json_deserializes_without_data_loss() {
        let legacy = r#"{
            "server_url": "wss://relay.example.com:58921",
            "account_id": "alice",
            "psk_secret": "user-configured-secret",
            "auto_inject": true,
            "rate_limit_mb": 42
        }"#;

        let parsed: AppSettings =
            serde_json::from_str(legacy).expect("老格式设置必须能反序列化，否则用户配置会被冲掉");

        assert_eq!(parsed.server_url, "wss://relay.example.com:58921");
        assert_eq!(parsed.account_id, "alice");
        assert_eq!(parsed.psk_secret, "user-configured-secret");
        assert!(parsed.auto_inject);
        assert!(!parsed.start_minimized, "缺失的新字段应取默认值 false");

        // 这两条守护的是 #[serde(default = "...")] 而不是裸 #[serde(default)]：
        // u32 的 Default 是 0，而 0 在本功能里表示「不限制 / 不自动消失」。
        // 换成裸 default 后这里会得到 0，两个需求对老用户静默失效，且面板上
        // 显示的 0 看起来像是用户自己设的，无从分辨。
        assert_eq!(
            parsed.history_max_entries, DEFAULT_HISTORY_MAX_ENTRIES,
            "老库缺字段时必须取 100，不能是 u32 的 Default 0（那表示不限制）"
        );
        assert_eq!(
            parsed.transfer_card_retain_secs, DEFAULT_TRANSFER_CARD_RETAIN_SECS,
            "老库缺字段时必须取 30，不能是 u32 的 Default 0（那表示不自动消失）"
        );

        // 三个缓存清理字段同理。retention.rs 的单测都经 default_config() 构造，
        // 走不到 serde 缺字段这条路径，所以这道闸只能建在这里。
        // 间隔那条后果最严重：0 会让调度循环拿到零间隔而退化成忙等。
        assert_eq!(
            parsed.cache_ttl_hours,
            crate::core::retention::DEFAULT_CACHE_TTL_HOURS,
            "老库缺字段时必须取 24，0 表示永不按时间清理、磁盘会被无声占满"
        );
        assert_eq!(
            parsed.cache_max_size_mb,
            crate::core::retention::DEFAULT_CACHE_MAX_SIZE_MB,
            "老库缺字段时必须取 10240，0 表示不限容量"
        );
        assert_eq!(
            parsed.cache_sweep_interval_minutes,
            crate::core::retention::DEFAULT_SWEEP_INTERVAL_MINUTES,
            "老库缺字段时必须取 60，0 会让清理循环退化成忙等"
        );
    }

    /// 0 是合法取值（不限制 / 不自动消失），不能被 default 逻辑改写成 100 / 30。
    #[test]
    fn explicit_zero_is_preserved_not_defaulted() {
        let json = r#"{
            "server_url": "wss://relay.example.com:58921",
            "account_id": "alice",
            "psk_secret": "s",
            "auto_inject": false,
            "rate_limit_mb": 10,
            "start_minimized": false,
            "history_max_entries": 0,
            "transfer_card_retain_secs": 0
        }"#;

        let parsed: AppSettings = serde_json::from_str(json).expect("反序列化失败");
        assert_eq!(parsed.history_max_entries, 0);
        assert_eq!(parsed.transfer_card_retain_secs, 0);
    }

    /// TTL 与容量的显式 0（= 关闭该段清理）同样不能被 default 改写。
    /// 间隔不在此列：0 对它非法，由 effective_sweep_interval 钳制。
    #[test]
    fn explicit_zero_cache_policy_is_preserved() {
        let json = r#"{
            "server_url": "wss://relay.example.com:58921",
            "account_id": "alice",
            "psk_secret": "s",
            "auto_inject": false,
            "rate_limit_mb": 10,
            "start_minimized": false,
            "history_max_entries": 100,
            "transfer_card_retain_secs": 30,
            "cache_ttl_hours": 0,
            "cache_max_size_mb": 0,
            "cache_sweep_interval_minutes": 5
        }"#;

        let parsed: AppSettings = serde_json::from_str(json).expect("反序列化失败");
        assert_eq!(parsed.cache_ttl_hours, 0, "0 = 不按时间清理，不得被 default 覆写成 24");
        assert_eq!(parsed.cache_max_size_mb, 0, "0 = 不限容量，不得被 default 覆写成 10240");
        assert_eq!(parsed.cache_sweep_interval_minutes, 5);
    }

    #[test]
    fn current_settings_json_round_trips() {
        let settings = AppSettings {
            start_minimized: true,
            ..AppSettings::default_config()
        };
        let json = serde_json::to_string(&settings).expect("序列化失败");
        let parsed: AppSettings = serde_json::from_str(&json).expect("反序列化失败");

        assert_eq!(parsed.server_url, settings.server_url);
        assert_eq!(parsed.account_id, settings.account_id);
        assert_eq!(parsed.psk_secret, settings.psk_secret);
        assert_eq!(parsed.auto_inject, settings.auto_inject);
        assert!(parsed.start_minimized);
        assert_eq!(parsed.history_max_entries, settings.history_max_entries);
        assert_eq!(parsed.transfer_card_retain_secs, settings.transfer_card_retain_secs);
        assert_eq!(parsed.cache_ttl_hours, settings.cache_ttl_hours);
        assert_eq!(parsed.cache_max_size_mb, settings.cache_max_size_mb);
        assert_eq!(parsed.cache_sweep_interval_minutes, settings.cache_sweep_interval_minutes);
    }

    #[test]
    fn autostart_is_not_part_of_persisted_settings() {
        // 自启状态只存在于操作系统，不得出现在落库的 JSON 里
        let json = serde_json::to_string(&AppSettings::default_config()).expect("序列化失败");
        assert!(!json.contains("autostart"));
    }

    #[test]
    fn account_id_empty_is_rejected() {
        assert!(validate_account_id("").is_err(), "空账号标识必须被拒绝");
    }

    #[test]
    fn account_id_whitespace_only_is_rejected() {
        // 面板的原生 required 放行纯空格，而 handleSubmit 的 trim 会把它变成空串。
        // 这里校验的是 trim 之后的值，所以等价于空串。
        assert!(validate_account_id("   ".trim()).is_err(), "纯空格账号标识必须被拒绝");
    }

    #[test]
    fn account_id_with_newline_is_rejected() {
        // canonical string 以换行分隔字段，放行换行就等于放行分隔符注入
        assert!(validate_account_id("acct\nUNIDROP_V1").is_err());
    }

    #[test]
    fn account_id_non_ascii_is_rejected() {
        // NFC / NFD 两种字节形式肉眼相同，会静默分成两个账号桶
        assert!(validate_account_id("团队").is_err());
    }

    #[test]
    fn default_config_account_id_passes_validation() {
        // 钉死「新规则没有把随发的默认值打死」——它红了就说明默认安装连不上
        let cfg = AppSettings::default_config();
        assert!(
            validate_account_id(&cfg.account_id).is_ok(),
            "默认账号标识 {} 必须通过校验",
            cfg.account_id
        );
    }

    #[test]
    fn account_id_accepts_documented_shapes() {
        for s in ["default_user", "my_team_sync", "user@example.com", "a.b", "A-1_2"] {
            assert!(validate_account_id(s).is_ok(), "合法账号标识被误拒: {}", s);
        }
    }

    #[test]
    fn legacy_settings_json_with_invalid_account_id_still_deserializes() {
        // 反序列化**不得**因账号非法而失败。
        // 校验只发生在保存与连接时；若读库这一步就整条失败，
        // lib.rs 的 unwrap_or_else 会回落到全部默认值，
        // 把用户已配置的 server_url / psk_secret 一起冲掉 ——
        // 那正是本文件其他注释反复强调要避免的形态。
        let legacy = r#"{
            "server_url": "wss://example.com",
            "account_id": "",
            "psk_secret": "secret",
            "auto_inject": false
        }"#;
        let parsed: AppSettings = serde_json::from_str(legacy).expect("老库 JSON 必须仍能反序列化");
        assert_eq!(parsed.server_url, "wss://example.com");
        assert_eq!(parsed.psk_secret, "secret");
        assert!(validate_account_id(&parsed.account_id).is_err(), "但它的账号标识确实非法");
    }

    #[test]
    fn allow_insecure_tls_defaults_to_secure_on_legacy_db() {
        // 老库的 JSON 没有这个字段，缺省必须是 false（= 校验证书）。
        // bool 的 Default 恰好是安全值，这也是它可以用裸 serde(default) 的原因。
        let legacy = r#"{
            "server_url": "wss://example.com",
            "account_id": "acct",
            "psk_secret": "secret",
            "auto_inject": false
        }"#;
        let parsed: AppSettings = serde_json::from_str(legacy).expect("反序列化失败");
        assert!(!parsed.allow_insecure_tls, "老库升级后必须默认校验证书");
        assert!(!AppSettings::default_config().allow_insecure_tls);
    }
}
