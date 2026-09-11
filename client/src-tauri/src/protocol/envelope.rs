use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponsePayload {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_id: Option<String>,
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
    pub preview_summary: String,
    pub encrypted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_metadata: Option<String>,
    #[serde(default)]
    pub items: Vec<TransferItemPayload>,
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
