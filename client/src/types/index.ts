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
  /** 启动时不弹出主窗口，仅托盘常驻。对手动启动与开机自启同样生效。 */
  start_minimized: boolean;
  /** 传输历史最多保留的条数，超出的最旧记录连同缓存文件一起删除；0 = 不限制。 */
  history_max_entries: number;
  /** 传输**完成**的卡片在界面保持的秒数；0 = 不自动消失。失败卡片不受此设置影响。 */
  transfer_card_retain_secs: number;
  /** 磁盘缓存文件保留小时数；0 = 不按时间清理。 */
  cache_ttl_hours: number;
  /** 磁盘缓存总量上限（MB）；0 = 不限容量。 */
  cache_max_size_mb: number;
  /** 后台清理间隔（分钟），最小 1。 */
  cache_sweep_interval_minutes: number;
}

/**
 * 服务端下发的传输限额（只读）。由部署者在服务端环境变量配置，客户端改不了。
 * 任一项为 0 表示该项不限制。
 */
export interface ServerLimits {
  max_single_file_bytes: number;
  max_total_transfer_bytes: number;
  max_clipboard_image_bytes: number;
  max_clipboard_text_bytes: number;
  max_items_per_offer: number;
  max_concurrent_transfers: number;
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
