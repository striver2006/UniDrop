export interface OnlineDevice {
  device_id: string;
  hostname: string;
  os_type: "windows" | "macos" | "linux" | string;
  app_version: string;
  remote_ip?: string;
}

export interface AppSettings {
  server_url: string;
  account_id: string;
  psk_secret: string;
  auto_inject: boolean;
  rate_limit_mb: number;
}

export interface ActiveTransfer {
  session_id: string;
  preview_summary: string;
  total_size: number;
  transferred_size: number;
  direction: "SEND" | "RECEIVE";
  progress: number; // 0..100
  status: "TRANSFERRING" | "COMPLETED" | "FAILED";
  data_type?: "FILES" | "TEXT" | "IMAGE" | string;
}

export interface ClipboardPreview {
  kind: "TEXT" | "IMAGE" | "FILES" | "EMPTY" | string;
  summary: string;
  count: number;
  size_bytes: number;
}

export interface TransferHistoryEntry {
  session_id: string;
  direction: "SEND" | "RECEIVE" | string;
  data_type: "FILES" | "TEXT" | "IMAGE" | string;
  preview_summary: string | null;
  total_size: number;
  total_items: number;
  status: string;
  error_message: string | null;
  created_at: string | null;
  completed_at: string | null;
  cached_count: number;
}
