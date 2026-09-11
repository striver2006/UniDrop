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
}
