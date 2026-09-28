//! 应用设置模型（从桌面端 `commands/settings_cmd.rs` 下沉，两端共享）。
//!
//! 不变量：`AppSettings` 的唯一写入方是**设置界面**（桌面 React 面板 /
//! 移动 Flutter 设置页）；后端自己写的开关必须走 `storage::db` 的
//! `get_local_flag` / `set_local_flag`，混进来会造成 read-modify-write 覆盖。

use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::host::HostBridge;
use crate::storage;

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
    /// 反序列化会整条失败并静默回落到全部默认值，
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

    /// 【迁移专用，TLS 代码一律不要读它】跳过服务器证书校验的老开关。
    ///
    /// 保留它只为两件事：读得懂老库；以及用户降级回旧版本时，旧版仍能读到
    /// 正确的值。真正的读取入口是 `effective_trust_mode()`。
    ///
    /// Rust 侧加 `legacy_` 前缀而 serde 保留原 key，是为了让"直接读这个字段"
    /// 在代码里长得就可疑——它现在只是三档枚举的一个投影，单独看会得出错误结论
    /// （Pinned 档下它是 false，但那并不意味着走的是纯公共 CA 校验）。
    ///
    /// 这里用裸 `#[serde(default)]` 是对的，与上面几个字段相反：
    /// `bool` 的 `Default` 是 `false`，而 `false` 恰好是安全值。
    /// 上面那些字段之所以要自定义 default，是因为它们的零值 `0` 被定义成
    /// 「关闭限制」——不安全的那一侧。
    #[serde(rename = "allow_insecure_tls", default)]
    pub legacy_allow_insecure_tls: bool,

    /// TLS 信任档位。`None` = 老库里根本没有这个键，交给
    /// `effective_trust_mode()` 做迁移。
    ///
    /// 刻意用 `Option` 而不是 `#[serde(default = "...")]`：「这是一个老库」
    /// 必须是个能表达、也能被测试直接断言的状态，否则迁移逻辑就没有可钉的对象。
    #[serde(default)]
    pub tls_trust_mode: Option<crate::core::tls_trust::TlsTrustMode>,

    /// 「信任指定证书」档下用户填的证书 SHA-256 指纹，每条一项。
    ///
    /// 存原样字符串而不是解析后的字节：设置面板要把用户填的内容原样显示回去，
    /// 而规范化（去冒号、转小写）会让他看到一串跟自己粘进去的不一样的东西。
    /// 解析在保存时做一次，失败当场报错。
    #[serde(default)]
    pub pinned_cert_sha256: Vec<String>,

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

    /// 接收策略：桌面端默认 `always`（保持旧行为）；移动端首启默认
    /// `wifi_only`（由移动壳构造首份设置时指定）。见 `host::ReceivePolicy`。
    #[serde(default)]
    pub receive_policy: crate::host::ReceivePolicy,

    /// 用户自定义设备显示名（连接层上报的 hostname）；空串 = 未设置，
    /// 跟随平台探测名（移动端平台 API / 桌面 whoami）。
    ///
    /// 为什么需要它：iOS 16 起 Apple 把 `UIDevice.name` 对第三方 App 脱敏成
    /// 通用型号名（"iPhone"），恢复真实名需 Apple 审批的
    /// `user-assigned-device-name` entitlement，个人开发者签名拿不到——
    /// 设置界面是 iOS 端拿到「像真名的名字」的唯一途径（业界通行做法，
    /// Home Assistant 同期同样改成手动设置）。保存后由
    /// `save_settings_flow` 更新 config 并立即重连，改名免重启即时生效。
    ///
    /// 裸 `#[serde(default)]` 在这里是安全方向：老库缺键 → 空串 → 未设置，
    /// 行为与升级前完全一致。
    #[serde(default)]
    pub device_name: String,
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
    /// 三档信任的**唯一**读取入口。
    ///
    /// 老库里 `tls_trust_mode` 这个键根本不存在，所以必须能从老的
    /// `allow_insecure_tls` 推出来——而且推的方向要对：
    /// 显式关过校验的用户必须原样保持 Insecure，否则升级之后他会突然连不上，
    /// 而界面上看不出任何东西变了。
    pub fn effective_trust_mode(&self) -> crate::core::tls_trust::TlsTrustMode {
        use crate::core::tls_trust::TlsTrustMode;
        match self.tls_trust_mode {
            Some(m) => m,
            None if self.legacy_allow_insecure_tls => TlsTrustMode::Insecure,
            // 键不存在、或存着 false：落到安全的那一侧。
            // 这一支就是 legacy_db_without_tls_fields_defaults_to_public_ca 钉的东西。
            None => TlsTrustMode::PublicCa,
        }
    }

    /// 解析出连接层真正要用的信任配置。指纹格式错误在这里暴露。
    pub fn tls_trust_config(&self) -> Result<crate::core::tls_trust::TlsTrustConfig, String> {
        crate::core::tls_trust::TlsTrustConfig::new(
            self.effective_trust_mode(),
            &self.pinned_cert_sha256,
        )
    }

    /// 把枚举回写成老 bool，保持两者一致。
    ///
    /// 为什么要回写：用户降级到旧版本时，旧版只认得 `allow_insecure_tls`。
    /// Pinned 档在旧版本上会落成 `false`（= 严格校验）而连不上——
    /// 那是**安全方向**的失败，正是降级时想要的形态。
    pub fn normalize_tls_trust(&mut self) {
        let mode = self.effective_trust_mode();
        self.tls_trust_mode = Some(mode);
        self.legacy_allow_insecure_tls =
            matches!(mode, crate::core::tls_trust::TlsTrustMode::Insecure);
    }

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
            legacy_allow_insecure_tls: false,
            tls_trust_mode: Some(crate::core::tls_trust::TlsTrustMode::PublicCa),
            pinned_cert_sha256: Vec::new(),
            e2ee_enabled: true,
            receive_policy: Default::default(),
            device_name: String::new(),
        }
    }
}

/// 账号标识的合法性校验，规则必须与服务端 `auth/identity.go` 保持一致：
/// 1..=64 字节的 `[A-Za-z0-9._@-]`。
///
/// 为什么客户端也要校验一遍：设置面板不是唯一的写入方。`default_config()`
/// 与老库里反序列化出来的 JSON 都会直接流进 `config_actor`，
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

/// 设备名清洗（保存流程用）：剥控制字符 + trim，超长报错。
///
/// hostname 会进服务端 roster 广播，控制字符（尤其换行）必须剥掉。
/// 长度按字符数（不是字节）算 64 上限：中文设备名 3 字节/字，按字节算
/// 会把合法名字误杀。超限**当场报错**而不是静默截断——静默截断会让
/// 面板显示的名字与实际上报的不一致，用户无从分辨。
fn sanitize_device_name(raw: &str) -> Result<String, String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    if cleaned.chars().count() > 64 {
        return Err("设备名最长 64 个字符".to_string());
    }
    Ok(cleaned)
}

/// 保存设置的全流程（清洗 → 落库 → 更新内存与 actor → 重连 → 认领 → 修剪）。
///
/// 从桌面 `cmd_save_settings` 下沉，桌面 tauri command 与移动 FFI 共用。
/// `app.emit` 之类宿主动作全部经 `bridge` 走。
pub async fn save_settings_flow(
    state: &AppState,
    bridge: &dyn HostBridge,
    mut clean_settings: AppSettings,
) -> Result<(), String> {
    // 账号标识：先 trim 再校验。
    // 面板那一层已经拦过一次，这里是兜底——理由见 validate_account_id。
    clean_settings.account_id = clean_settings.account_id.trim().to_string();
    validate_account_id(&clean_settings.account_id)?;
    clean_settings.device_name = sanitize_device_name(&clean_settings.device_name)?;

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

    // 间隔落库前规范化：非法值（尤其 0）不该被持久化，否则 get_settings
    // 会把它原样返回给前端，界面显示的和实际生效的对不上。
    // 与调度循环共用同一个函数，「前后端各校验一次」才名实相符。
    clean_settings.cache_sweep_interval_minutes =
        crate::core::retention::effective_sweep_interval_minutes(&clean_settings);

    // 指纹在**保存时**解析，不在重连循环里。
    //
    // 格式写错要在这里当场以精确原因回给用户（他刚粘完，还知道自己粘了什么）；
    // 留到连接时才发现，就只剩一次次查不出原因的握手失败。
    clean_settings.pinned_cert_sha256 = clean_settings
        .pinned_cert_sha256
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let trust_config = clean_settings.tls_trust_config()?;

    // 枚举与老 bool 保持一致，理由见 normalize_tls_trust。
    clean_settings.normalize_tls_trust();

    let json_str = serde_json::to_string(&clean_settings).map_err(|e| e.to_string())?;
    {
        let conn = state.db_conn.lock().await;
        storage::db::save_persisted_settings(&conn, &json_str).map_err(|e| e.to_string())?;
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
        cfg.tls_trust = trust_config;
        // 设备名：自定义非空即覆盖；清空回落平台探测名（AppState.hostname
        // 保存的就是它）。改名随步骤 4 的重连即时生效，对端 roster 立刻更新。
        cfg.hostname = if clean_settings.device_name.is_empty() {
            state.hostname.clone()
        } else {
            clean_settings.device_name.clone()
        };
    }

    // 3. Clear online devices from previous server/account and notify frontend
    {
        let mut devs = state.online_devices.lock().await;
        devs.clear();
    }
    crate::host::emit_json(bridge, "devices-updated", &Vec::<crate::protocol::OnlineDevice>::new());

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
        match storage::db::claim_unowned_history(&conn, &clean_settings.account_id) {
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
        bridge.emit("account-changed", &serde_json::Value::Null);
    }

    // 7. 立刻按新上限修剪历史：用户把条数调小后必须当场见效，
    //    否则要等到下次传输才生效，看起来就像设置没保存。
    //    此处已在上面各锁的作用域之外，取 db_conn 锁是安全的。
    crate::core::history_pruner::prune_and_notify(state, bridge).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tls_trust::TlsTrustMode;

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
    /// `#[serde(default)]`，整条反序列化会失败并走启动路径的 unwrap_or_else
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
        // device_name 同理：老库缺键 → 空串 = 未设置 → 跟随平台探测名，
        // 行为与升级前一致。这条钉住「新增字段不冲掉用户配置」的那半。
        assert!(parsed.device_name.is_empty(), "老库缺 device_name 应取空串而非报错");

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
            device_name: "陈振博的 iPhone".to_string(),
            ..AppSettings::default_config()
        };
        let json = serde_json::to_string(&settings).expect("序列化失败");
        let parsed: AppSettings = serde_json::from_str(&json).expect("反序列化失败");

        assert_eq!(parsed.server_url, settings.server_url);
        assert_eq!(parsed.account_id, settings.account_id);
        assert_eq!(parsed.psk_secret, settings.psk_secret);
        assert_eq!(parsed.device_name, settings.device_name);
        assert_eq!(parsed.auto_inject, settings.auto_inject);
        assert!(parsed.start_minimized);
        assert_eq!(parsed.history_max_entries, settings.history_max_entries);
        assert_eq!(parsed.transfer_card_retain_secs, settings.transfer_card_retain_secs);
        assert_eq!(parsed.cache_ttl_hours, settings.cache_ttl_hours);
        assert_eq!(parsed.cache_max_size_mb, settings.cache_max_size_mb);
        assert_eq!(parsed.cache_sweep_interval_minutes, settings.cache_sweep_interval_minutes);
        assert_eq!(parsed.receive_policy, settings.receive_policy);
    }

    #[test]
    fn autostart_is_not_part_of_persisted_settings() {
        // 自启状态只存在于操作系统，不得出现在落库的 JSON 里
        let json = serde_json::to_string(&AppSettings::default_config()).expect("序列化失败");
        assert!(!json.contains("autostart"));
    }

    /// 老库 JSON 没有 receive_policy 字段时必须回落到 Always（桌面旧行为），
    /// 不得因新增字段导致整条反序列化失败。
    #[test]
    fn legacy_settings_without_receive_policy_defaults_to_always() {
        let legacy = r#"{
            "server_url": "wss://example.com",
            "account_id": "acct",
            "psk_secret": "secret",
            "auto_inject": false
        }"#;
        let parsed: AppSettings = serde_json::from_str(legacy).expect("老库 JSON 必须仍能反序列化");
        assert_eq!(parsed.receive_policy, crate::host::ReceivePolicy::Always);
    }

    #[test]
    fn receive_policy_serializes_as_lowercase_string() {
        let json = serde_json::to_string(&crate::host::ReceivePolicy::WifiOnly).unwrap();
        assert_eq!(json, r#""wifi_only""#);
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
        // 启动路径的 unwrap_or_else 会回落到全部默认值，
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
    fn legacy_db_without_tls_fields_defaults_to_public_ca() {
        // 老库的 JSON 两个 TLS 字段都没有，缺省必须落在安全的一侧。
        // bool 的 Default 恰好是安全值，这也是它可以用裸 serde(default) 的原因；
        // 加了枚举之后这条结论仍然要成立，所以这条测试跟着改名留下来。
        let legacy = r#"{
            "server_url": "wss://example.com",
            "account_id": "acct",
            "psk_secret": "secret",
            "auto_inject": false
        }"#;
        let parsed: AppSettings = serde_json::from_str(legacy).expect("反序列化失败");
        assert!(!parsed.legacy_allow_insecure_tls, "老库升级后必须默认校验证书");
        assert_eq!(parsed.effective_trust_mode(), TlsTrustMode::PublicCa);
        assert_eq!(
            AppSettings::default_config().effective_trust_mode(),
            TlsTrustMode::PublicCa
        );
    }

    #[test]
    fn legacy_db_with_allow_insecure_true_migrates_to_insecure() {
        // 显式关过校验的用户必须原样保持，否则升级后突然连不上，
        // 而界面上看不出任何东西变了。
        let legacy = r#"{
            "server_url": "wss://example.com",
            "account_id": "acct",
            "psk_secret": "secret",
            "auto_inject": false,
            "allow_insecure_tls": true
        }"#;
        let parsed: AppSettings = serde_json::from_str(legacy).expect("反序列化失败");
        assert_eq!(parsed.effective_trust_mode(), TlsTrustMode::Insecure);
    }

    #[test]
    fn explicit_mode_wins_over_legacy_flag() {
        // 新键在就以新键为准——老 bool 只是它的投影，不该反过来干扰。
        let json = r#"{
            "server_url": "wss://example.com",
            "account_id": "acct",
            "psk_secret": "secret",
            "auto_inject": false,
            "allow_insecure_tls": true,
            "tls_trust_mode": "public_ca"
        }"#;
        let parsed: AppSettings = serde_json::from_str(json).expect("反序列化失败");
        assert_eq!(parsed.effective_trust_mode(), TlsTrustMode::PublicCa);
    }

    #[test]
    fn normalize_keeps_legacy_flag_in_sync() {
        let mut s = AppSettings::default_config();

        // Pinned 在旧版本上落成 false（= 严格校验）而连不上：
        // 安全方向的失败，正是降级时想要的形态。
        s.tls_trust_mode = Some(TlsTrustMode::Pinned);
        s.normalize_tls_trust();
        assert!(!s.legacy_allow_insecure_tls);

        s.tls_trust_mode = Some(TlsTrustMode::Insecure);
        s.normalize_tls_trust();
        assert!(s.legacy_allow_insecure_tls, "降级回旧版本时必须仍是关校验");

        s.tls_trust_mode = Some(TlsTrustMode::PublicCa);
        s.normalize_tls_trust();
        assert!(!s.legacy_allow_insecure_tls);
    }

    #[test]
    fn pinned_mode_requires_at_least_one_fingerprint() {
        let mut s = AppSettings::default_config();
        s.tls_trust_mode = Some(TlsTrustMode::Pinned);
        assert!(s.tls_trust_config().is_err(), "空指纹的 Pinned 档等于没有任何信任来源");
    }

    #[test]
    fn malformed_pin_is_rejected() {
        let mut s = AppSettings::default_config();
        s.tls_trust_mode = Some(TlsTrustMode::Pinned);
        s.pinned_cert_sha256 = vec!["not-a-fingerprint".to_string()];
        assert!(s.tls_trust_config().is_err());
    }

    #[test]
    fn openssl_style_fingerprint_is_accepted() {
        // 用户最可能直接粘 openssl 的整行输出，连前缀带冒号。
        let mut s = AppSettings::default_config();
        s.tls_trust_mode = Some(TlsTrustMode::Pinned);
        s.pinned_cert_sha256 = vec![
            "SHA256 Fingerprint=AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:\
             AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89"
                .to_string(),
        ];
        assert!(s.tls_trust_config().is_ok(), "openssl 原样输出必须能直接粘进来");
    }

    /// 设备名里的换行必须被剥掉：hostname 会进服务端 roster 广播，
    /// 带换行的名字在对端列表里会拆成两行。
    #[test]
    fn sanitize_device_name_strips_control_chars_and_trims() {
        assert_eq!(
            sanitize_device_name("  Bob的\u{1}iPhone\n").unwrap(),
            "Bob的iPhone"
        );
        assert_eq!(sanitize_device_name("\t\r\n").unwrap(), "");
        // 中文按字符数计：64 字远超 64 字节，不能被字节上限误杀。
        let cjk64 = "设".repeat(64);
        assert_eq!(sanitize_device_name(&cjk64).unwrap(), cjk64);
    }

    #[test]
    fn sanitize_device_name_rejects_over_length() {
        let too_long = "a".repeat(65);
        assert!(sanitize_device_name(&too_long).is_err(), "65 字符必须被拒绝");
        // 65 个中文 trim 后仍是 65 字（不是 195 字节的问题）
        let cjk65 = "名".repeat(65);
        assert!(sanitize_device_name(&cjk65).is_err(), "长度上限按字符数计");
    }
}
