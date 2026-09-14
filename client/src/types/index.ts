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
  /**
   * 【迁移专用】跳过 TLS 服务器证书校验的老开关。
   *
   * 真正生效的是 tls_trust_mode，这个字段只是它的投影，单独读会得出错误结论
   * （pinned 档下它是 false，但那不等于走的是纯公共 CA 校验）。
   * 保留它是为了读得懂老库、以及用户降级回旧版本时旧版仍能读到正确的值。
   */
  allow_insecure_tls: boolean;
  /**
   * TLS 信任档位。null = 老库里没有这个键，按 allow_insecure_tls 推。
   *
   * 三档而不是一个开关：改造前「证书合法但名字对不上」和「企业内部 CA」
   * 这两类部署，唯一出路是把校验整个关掉——本该收窄到单台服务器的例外，
   * 被迫放大成对所有中间人敞开。
   */
  tls_trust_mode: TlsTrustMode | null;
  /** 「信任指定证书」档下用户填的证书 SHA-256 指纹，每条一项。 */
  pinned_cert_sha256: string[];
  /**
   * 对传输内容做端到端加密；默认 **true**。
   *
   * 与紧邻的 allow_insecure_tls 默认值相反，两者都遵循同一条规则：
   * 默认值必须落在安全的那一侧。开启后不保证每次都加密——对端版本过旧时
   * 会回落明文并推送 e2ee-fallback 事件。
   */
  e2ee_enabled: boolean;
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

/** TLS 信任档位。与 Rust 侧 `core::tls_trust::TlsTrustMode` 一一对应。 */
export type TlsTrustMode = "public_ca" | "pinned" | "insecure";

/**
 * TLS 证书校验失败的分类。与 Rust 侧 `core::tls_trust::CertFailureKind`
 * 的 `#[serde(tag = "kind")]` 一一对应，改一边必须改另一边。
 *
 * 分这么细不是为了好看：每一档的**正确处置动作都不一样**，
 * 而改造之前它们共用一句「勾选允许不安全连接」——那句话对其中几档是错的。
 */
export type CertFailureKind =
  /** 证书有效，只是签发给了别的名字（典型：用 IP 直连一台只为域名签证书的服务器）。 */
  | { kind: "name_mismatch"; expected: string; presented: string[] }
  /** 公共根证书库里没有这个签发者：自签或企业内部 CA。 */
  | { kind: "unknown_issuer" }
  | { kind: "expired" }
  | { kind: "not_yet_valid" }
  | { kind: "revoked" }
  /** X.509 v1：rustls 在解析阶段就拒绝，跳过校验也救不回来。 */
  | { kind: "unsupported_version" }
  | { kind: "other" };

/** `tls-cert-failed` 事件的载荷。 */
export type TlsCertFailure = CertFailureKind & {
  /** 横幅收起时显示的一行。 */
  title: string;
  /** 展开后逐行显示，已经是对症的处置建议。 */
  detail: string[];
  /** 原始错误串。分类错了的时候这是唯一的现场，所以界面上也要能看到。 */
  raw: string;
  /**
   * 握手中实际观察到的叶子证书指纹，供用户与服务器上那张证书核对。
   *
   * 只有控制面装了观察器，数据面为 null；后端拿不到证书时（例如 TCP 就没连上）
   * 同样为 null——所以渲染前必须判空。
   */
  observed_cert_sha256: string | null;
};

/** 各档失败对应的状态栏文案。 */
export function certFailureStatusText(kind: CertFailureKind["kind"]): string {
  switch (kind) {
    // 「证书不受信任」在这一档是**假话**：证书可能完全合法，
    // 错的是我们连的地址。说错了会把用户推去关校验。
    case "name_mismatch":
      return "证书域名不匹配";
    case "unknown_issuer":
      return "证书签发者未知";
    case "expired":
      return "证书已过期";
    case "not_yet_valid":
      return "证书尚未生效";
    case "revoked":
      return "证书已被吊销";
    case "unsupported_version":
      return "证书格式不受支持";
    default:
      return "证书校验失败";
  }
}
