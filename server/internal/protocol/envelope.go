package protocol

import "encoding/json"

// ActionType represents the type of control signaling message.
type ActionType string

const (
	// Authentication
	ActionAuthChallenge ActionType = "AUTH_CHALLENGE"
	ActionAuthRequest   ActionType = "AUTH_REQUEST"
	ActionAuthResponse  ActionType = "AUTH_RESPONSE"

	// Topology & Keepalive
	ActionHeartbeatPing  ActionType = "HEARTBEAT_PING"
	ActionHeartbeatPong  ActionType = "HEARTBEAT_PONG"
	ActionDeviceOnline   ActionType = "DEVICE_ONLINE"
	ActionDeviceOffline  ActionType = "DEVICE_OFFLINE"
	ActionDeviceListSync ActionType = "DEVICE_LIST_SYNC"

	// Transfer Negotiation
	ActionTransferOffer    ActionType = "TRANSFER_OFFER"
	ActionTransferAnswer   ActionType = "TRANSFER_ANSWER"
	ActionTransferCancel   ActionType = "TRANSFER_CANCEL"
	ActionTransferFailure  ActionType = "TRANSFER_FAILURE"
	ActionTransferComplete ActionType = "TRANSFER_COMPLETE"

	// Clipboard Feedback
	ActionClipboardInjected ActionType = "CLIPBOARD_INJECTED"
)

// ControlEnvelope is the universal outer JSON envelope for all control plane messages.
type ControlEnvelope struct {
	Version    int             `json:"version"`             // Currently 1
	TraceID    string          `json:"trace_id"`            // UUIDv4 trace id
	Action     ActionType      `json:"action"`              // ActionType
	FromDevice string          `json:"from_device"`         // Sender device ID
	ToDevice   string          `json:"to_device,omitempty"` // Target device ID (if unicast)
	Timestamp  int64           `json:"timestamp"`           // Millisecond timestamp
	Payload    json.RawMessage `json:"payload,omitempty"`   // Raw JSON payload
}

// AuthChallengePayload is sent by server to client upon connection.
type AuthChallengePayload struct {
	NonceSalt  string `json:"nonce_salt"`
	ServerTime int64  `json:"server_time"`
}

// AuthRequestPayload is sent by client to authenticate.
type AuthRequestPayload struct {
	AccountID  string `json:"account_id"`
	DeviceID   string `json:"device_id"`
	Hostname   string `json:"hostname"`
	OSType     string `json:"os_type"` // "windows" | "macos" | "linux"
	AppVersion string `json:"app_version"`
	Signature  string `json:"signature"`
	Nonce      string `json:"nonce"`
	Timestamp  int64  `json:"timestamp"`
}

// ServerLimits is the transfer limit set the server advertises to a client
// right after authentication. It is configured by the operator through
// environment variables and is read-only from the client's point of view.
//
// A zero on any field means that limit is switched off. Defaults and parsing
// live in internal/limits, which is the sole source of truth for the values;
// only the wire shape belongs here.
type ServerLimits struct {
	MaxSingleFileBytes     int64 `json:"max_single_file_bytes"`
	MaxTotalTransferBytes  int64 `json:"max_total_transfer_bytes"`
	MaxClipboardImageBytes int64 `json:"max_clipboard_image_bytes"`
	MaxClipboardTextBytes  int64 `json:"max_clipboard_text_bytes"`
	MaxItemsPerOffer       int   `json:"max_items_per_offer"`
	MaxConcurrentTransfers int   `json:"max_concurrent_transfers"`
}

// AuthResponsePayload is returned to client with auth outcome.
//
// Limits is a pointer with omitempty so that an older server — which simply
// does not send the field — is distinguishable on the client from a server
// that sent an all-zero set. The two must not be conflated: absent means
// "unknown, fall back to the client's own constants", whereas all-zero would
// mean "every limit switched off".
type AuthResponsePayload struct {
	Success      bool          `json:"success"`
	ErrorCode    string        `json:"error_code,omitempty"`
	ErrorMessage string        `json:"error_message,omitempty"`
	AssignedID   string        `json:"assigned_id,omitempty"`
	Limits       *ServerLimits `json:"limits,omitempty"`
}

// OnlineDevice represents an active peer device.
type OnlineDevice struct {
	DeviceID   string `json:"device_id"`
	Hostname   string `json:"hostname"`
	OSType     string `json:"os_type"`
	AppVersion string `json:"app_version"`
	RemoteIP   string `json:"remote_ip,omitempty"`
}

// DeviceListSyncPayload is sent to client upon successful auth.
type DeviceListSyncPayload struct {
	Devices []OnlineDevice `json:"devices"`
}

// DeviceOnlinePayload is broadcast when a peer connects.
type DeviceOnlinePayload struct {
	Device OnlineDevice `json:"device"`
}

// DeviceOfflinePayload is broadcast when a peer disconnects.
type DeviceOfflinePayload struct {
	DeviceID string `json:"device_id"`
	Reason   string `json:"reason,omitempty"`
}

// TransferItemPayload describes a single item in a transfer offer.
type TransferItemPayload struct {
	ItemIndex    uint32 `json:"item_index"`
	RelativePath string `json:"relative_path"`
	Size         int64  `json:"size"`
	IsDir        bool   `json:"is_dir"`
	SHA256       string `json:"sha256"`
	TotalChunks  uint32 `json:"total_chunks"`
}

// TransferOfferPayload is sent by sender to propose a transfer.
type TransferOfferPayload struct {
	SessionID         string                `json:"session_id"` // UUID string
	DataType          string                `json:"data_type"`  // "TEXT" | "IMAGE" | "FILES"
	TotalSize         int64                 `json:"total_size"`
	TotalItems        int                   `json:"total_items"`
	PreviewSummary    string                `json:"preview_summary"`
	Encrypted         bool                  `json:"encrypted"`
	EncryptedMetadata string                `json:"encrypted_metadata,omitempty"`
	Items             []TransferItemPayload `json:"items,omitempty"`
}

// ResumedItemPayload describes chunks already present locally.
type ResumedItemPayload struct {
	ItemIndex      uint32   `json:"item_index"`
	ExistingChunks []uint32 `json:"existing_chunks"`
}

// TransferAnswerPayload is sent by receiver in response to an offer.
type TransferAnswerPayload struct {
	SessionID    string               `json:"session_id"`
	Accepted     bool                 `json:"accepted"`
	RejectReason string               `json:"reject_reason,omitempty"`
	ResumedItems []ResumedItemPayload `json:"resumed_items,omitempty"`
	Token        string               `json:"token,omitempty"`
}

// TransferFailurePayload reports an unrecoverable failure.
type TransferFailurePayload struct {
	SessionID       string `json:"session_id"`
	ErrorCode       string `json:"error_code"`
	ErrorMessage    string `json:"error_message"`
	FailedItemIndex uint32 `json:"failed_item_index,omitempty"`
}

// ClipboardInjectedPayload is sent when the receiver has loaded the files into clipboard.
type ClipboardInjectedPayload struct {
	SessionID  string `json:"session_id"`
	InjectedAt int64  `json:"injected_at"`
	ItemCount  int    `json:"item_count"`
}
