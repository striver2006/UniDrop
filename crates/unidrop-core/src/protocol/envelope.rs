use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ActionType {
    AUTH_CHALLENGE,
    AUTH_REQUEST,
    AUTH_RESPONSE,

    HEARTBEAT_PING,
    HEARTBEAT_PONG,
    DEVICE_ONLINE,
    DEVICE_OFFLINE,
    DEVICE_LIST_SYNC,

    TRANSFER_OFFER,
    TRANSFER_ANSWER,
    TRANSFER_CANCEL,
    TRANSFER_FAILURE,
    TRANSFER_COMPLETE,

    CLIPBOARD_INJECTED,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlEnvelope<T = serde_json::Value> {
    pub version: i32,
    pub trace_id: String,
    pub action: ActionType,
    pub from_device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_device: Option<String>,
    pub timestamp: i64,
    #[serde(default)]
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthChallengePayload {
    pub nonce_salt: String,
    pub server_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequestPayload {
    pub account_id: String,
    pub device_id: String,
    pub hostname: String,
    pub os_type: String,
    pub app_version: String,
    pub signature: String,
    pub nonce: String,
    pub timestamp: i64,
}

/// 服务端在鉴权成功后下发的传输限额。只读——由部署者在服务端 env 配置，
/// 客户端改不了（这条边界是刻意的，理由见 `.reviews` 下 server-transfer-limits
/// 主题计划的 §3）。
///
/// 任一项为 `0` 表示该项不限制。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerLimits {
    pub max_single_file_bytes: i64,
    pub max_total_transfer_bytes: i64,
    pub max_clipboard_image_bytes: i64,
    pub max_clipboard_text_bytes: i64,
    pub max_items_per_offer: i32,
    pub max_concurrent_transfers: i32,
}

impl ServerLimits {
    /// `limit <= 0` 视为不限制。三处校验共用这一个判定，避免各写各的
    /// `if x > 0 &&`，漏掉一处就是一项限额在某条路径上静默失效。
    pub fn exceeds(limit: i64, actual: i64) -> bool {
        limit > 0 && actual > limit
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponsePayload {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_id: Option<String>,

    /// 老服务端不发这个字段，于是这里是 `None`。
    ///
    /// **`None` 必须退回客户端自己的兜底常量，不得 `unwrap_or_default()`。**
    /// 三种处理在代码上只差一个调用，后果却完全不同：
    /// 沿用兜底 = 老服务器上一切照旧；全零 = 老服务器上限额全部失效；
    /// 若把全零当"都为 0 即不限制"以外的含义用，还可能变成什么都发不出去。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ServerLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineDevice {
    pub device_id: String,
    pub hostname: String,
    pub os_type: String,
    pub app_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceListSyncPayload {
    pub devices: Vec<OnlineDevice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceOnlinePayload {
    pub device: OnlineDevice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceOfflinePayload {
    pub device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferItemPayload {
    pub item_index: u32,
    pub relative_path: String,
    pub size: i64,
    pub is_dir: bool,
    pub sha256: String,
    pub total_chunks: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOfferPayload {
    pub session_id: String,
    pub data_type: String, // "TEXT" | "IMAGE" | "FILES"
    pub total_size: i64,
    pub total_items: usize,
    /// 预览摘要。`encrypted = true` 时这里是空串，真值在 `encrypted_metadata` 里。
    pub preview_summary: String,
    /// 本次传输是否加密。此前是一直硬编码 `false` 的协议占位，现已启用。
    pub encrypted: bool,
    /// `encrypted = true` 时装 base64(AEAD(条目元数据 JSON))，见 `EncryptedMetadata`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_metadata: Option<String>,
    /// E2EE 协议版本。`None` = 明文传输（老客户端或协商回落）。
    ///
    /// 它**不承担能力协商职责**——协商在发 OFFER 之前就由对端 `app_version` 定了
    /// （见 `core::e2ee::peer_supports_e2ee`）。这里只是接收端的自描述。
    /// `skip_serializing_if` 让明文 OFFER 与老版本保持字节级一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2ee_version: Option<u8>,
    #[serde(default)]
    pub items: Vec<TransferItemPayload>,
}

/// 加密元数据的明文形态：`encrypted = true` 时从 `items[]` 里摘出来的敏感字段。
///
/// 为什么这几个字段必须加密，而 `size` / `total_chunks` 不能加密：
/// 服务端限额只读 `total_size` / `data_type` / `items[].size` 与条目数
/// （server/internal/limits/limits.go），**从不读文件名与哈希**。
/// 所以能藏的就藏，藏不了的如实留明文——这条切分线是限额机制的必然代价，
/// 不是可以再优化的空间。
///
/// `sha256` 尤其要藏：它是内容的密码学指纹，服务端拿它**查表**就能精确确认
/// 你传了哪个已知文件，不需要暴力枚举。加密了正文却附赠内容指纹，
/// 与把 CRC32 算在明文上是同一类错误。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedMetadata {
    pub preview_summary: String,
    pub items: Vec<EncryptedItemMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedItemMeta {
    pub item_index: u32,
    pub relative_path: String,
    pub sha256: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumedItemPayload {
    pub item_index: u32,
    pub existing_chunks: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferAnswerPayload {
    pub session_id: String,
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
    #[serde(default)]
    pub resumed_items: Vec<ResumedItemPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferFailurePayload {
    pub session_id: String,
    pub error_code: String,
    pub error_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_item_index: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardInjectedPayload {
    pub session_id: String,
    pub injected_at: i64,
    pub item_count: usize,
}
