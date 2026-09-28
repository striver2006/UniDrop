/// 与 `unidrop_core` 的 serde 结构一一对应的 Dart 模型。
///
/// 字段名与 Rust 侧 serde 序列化严格一致（snake_case）。

library;

class OnlineDevice {
  OnlineDevice({
    required this.deviceId,
    required this.hostname,
    required this.osType,
    required this.appVersion,
    this.remoteIp,
  });

  final String deviceId;
  final String hostname;
  final String osType;
  final String appVersion;
  final String? remoteIp;

  factory OnlineDevice.fromJson(Map<String, dynamic> j) => OnlineDevice(
        deviceId: j['device_id'] as String,
        hostname: j['hostname'] as String,
        osType: j['os_type'] as String,
        appVersion: j['app_version'] as String,
        remoteIp: j['remote_ip'] as String?,
      );

  /// 平台图标语义：桌面三平台 + 三移动端。
  // switch 表达式是 Dart 3 语法——鸿蒙 Flutter 基座是 3.7/Dart 2.19，
  // 全 lib/ 保持 2.19 兼容（官方 3.44 同样合法），三端一套代码。
  String get platformLabel {
    switch (osType) {
      case 'macos':
        return 'macOS';
      case 'windows':
        return 'Windows';
      case 'linux':
        return 'Linux';
      case 'ios':
        return 'iPhone/iPad';
      case 'android':
        return 'Android';
      case 'ohos':
        return '鸿蒙';
    }
    return osType;
  }

  bool get isMobile => osType == 'ios' || osType == 'android' || osType == 'ohos';
}

class ActiveTransfer {
  ActiveTransfer({
    required this.sessionId,
    required this.previewSummary,
    required this.totalSize,
    required this.transferredSize,
    required this.direction,
    required this.progress,
    required this.status,
    required this.dataType,
  });

  final String sessionId;
  final String previewSummary;
  final int totalSize;
  final int transferredSize;
  final String direction; // SEND | RECEIVE
  final double progress; // 0..100
  final String status; // TRANSFERRING | COMPLETED | FAILED
  final String dataType; // FILES | TEXT | IMAGE

  factory ActiveTransfer.fromJson(Map<String, dynamic> j) => ActiveTransfer(
        sessionId: j['session_id'] as String,
        previewSummary: j['preview_summary'] as String? ?? '',
        totalSize: (j['total_size'] as num?)?.toInt() ?? 0,
        transferredSize: (j['transferred_size'] as num?)?.toInt() ?? 0,
        direction: j['direction'] as String? ?? 'SEND',
        progress: (j['progress'] as num?)?.toDouble() ?? 0,
        status: j['status'] as String? ?? 'TRANSFERRING',
        dataType: j['data_type'] as String? ?? 'FILES',
      );

  bool get isReceive => direction == 'RECEIVE';
  bool get isTerminal => status == 'COMPLETED' || status == 'FAILED';
}

class HistoryEntry {
  HistoryEntry({
    required this.sessionId,
    required this.direction,
    required this.dataType,
    this.previewSummary,
    required this.totalSize,
    required this.totalItems,
    required this.status,
    this.errorMessage,
    this.createdAt,
    this.completedAt,
    required this.cachedCount,
  });

  final String sessionId;
  final String direction;
  final String dataType;
  final String? previewSummary;
  final int totalSize;
  final int totalItems;
  final String status;
  final String? errorMessage;
  final String? createdAt;
  final String? completedAt;
  final int cachedCount;

  factory HistoryEntry.fromJson(Map<String, dynamic> j) => HistoryEntry(
        sessionId: j['session_id'] as String,
        direction: j['direction'] as String,
        dataType: j['data_type'] as String,
        previewSummary: j['preview_summary'] as String?,
        totalSize: (j['total_size'] as num?)?.toInt() ?? 0,
        totalItems: (j['total_items'] as num?)?.toInt() ?? 0,
        status: j['status'] as String,
        errorMessage: j['error_message'] as String?,
        createdAt: j['created_at'] as String?,
        completedAt: j['completed_at'] as String?,
        cachedCount: (j['cached_count'] as num?)?.toInt() ?? 0,
      );

  bool get isReceive => direction == 'RECEIVE';
  bool get hasCachedFiles => cachedCount > 0;
}

class ServerLimits {
  ServerLimits({
    required this.maxSingleFileBytes,
    required this.maxTotalTransferBytes,
    required this.maxClipboardImageBytes,
    required this.maxClipboardTextBytes,
    required this.maxItemsPerOffer,
    required this.maxConcurrentTransfers,
  });

  final int maxSingleFileBytes;
  final int maxTotalTransferBytes;
  final int maxClipboardImageBytes;
  final int maxClipboardTextBytes;
  final int maxItemsPerOffer;
  final int maxConcurrentTransfers;

  factory ServerLimits.fromJson(Map<String, dynamic> j) => ServerLimits(
        maxSingleFileBytes: (j['max_single_file_bytes'] as num?)?.toInt() ?? 0,
        maxTotalTransferBytes:
            (j['max_total_transfer_bytes'] as num?)?.toInt() ?? 0,
        maxClipboardImageBytes:
            (j['max_clipboard_image_bytes'] as num?)?.toInt() ?? 0,
        maxClipboardTextBytes:
            (j['max_clipboard_text_bytes'] as num?)?.toInt() ?? 0,
        maxItemsPerOffer: (j['max_items_per_offer'] as num?)?.toInt() ?? 0,
        maxConcurrentTransfers:
            (j['max_concurrent_transfers'] as num?)?.toInt() ?? 0,
      );
}

class AppSettingsDto {
  AppSettingsDto({
    required this.serverUrl,
    required this.accountId,
    required this.pskSecret,
    required this.autoInject,
    required this.receivePolicy,
    required this.e2eeEnabled,
    required this.tlsTrustMode,
    required this.pinnedCertSha256,
    required this.historyMaxEntries,
    required this.transferCardRetainSecs,
    required this.cacheTtlHours,
    required this.cacheMaxSizeMb,
    this.deviceName = '',
  });

  String serverUrl;
  String accountId;
  String pskSecret;
  bool autoInject;
  String receivePolicy; // always | wifi_only | ask
  bool e2eeEnabled;
  String tlsTrustMode; // public_ca | pinned | insecure
  List<String> pinnedCertSha256;
  int historyMaxEntries;
  int transferCardRetainSecs;
  int cacheTtlHours;
  int cacheMaxSizeMb;

  /// 用户自定义设备显示名；空串 = 未设置，跟随平台探测名。
  /// iOS 16+ 系统不再提供真实设备名，设置页靠它改名（Rust 侧落库并
  /// 在启动/保存时优先于平台探测名，见 core settings.rs）。
  String deviceName;

  factory AppSettingsDto.fromJson(Map<String, dynamic> j) => AppSettingsDto(
        serverUrl: j['server_url'] as String? ?? '',
        accountId: j['account_id'] as String? ?? '',
        pskSecret: j['psk_secret'] as String? ?? '',
        autoInject: j['auto_inject'] as bool? ?? false,
        receivePolicy: (j['receive_policy'] as String?) ?? 'always',
        e2eeEnabled: j['e2ee_enabled'] as bool? ?? true,
        tlsTrustMode: _trustModeOf(j['tls_trust_mode']),
        pinnedCertSha256: (j['pinned_cert_sha256'] as List<dynamic>? ?? [])
            .map((e) => e.toString())
            .toList(),
        historyMaxEntries: (j['history_max_entries'] as num?)?.toInt() ?? 100,
        transferCardRetainSecs:
            (j['transfer_card_retain_secs'] as num?)?.toInt() ?? 30,
        cacheTtlHours: (j['cache_ttl_hours'] as num?)?.toInt() ?? 24,
        cacheMaxSizeMb: (j['cache_max_size_mb'] as num?)?.toInt() ?? 10240,
        deviceName: (j['device_name'] as String?) ?? '',
      );

  Map<String, dynamic> toJson() => {
        'server_url': serverUrl,
        'account_id': accountId,
        'psk_secret': pskSecret,
        'auto_inject': autoInject,
        'receive_policy': receivePolicy,
        'e2ee_enabled': e2eeEnabled,
        'tls_trust_mode': tlsTrustMode,
        'pinned_cert_sha256': pinnedCertSha256,
        // 桌面字段全量回填，防止保存时把未展示的设置抹掉：
        // AppSettings 是「整份落库」语义（见 core::settings 模块注释）。
        // 未在移动端暴露的字段（start_minimized / sweep 间隔）给桌面默认值。
        'start_minimized': false,
        'history_max_entries': historyMaxEntries,
        'transfer_card_retain_secs': transferCardRetainSecs,
        'cache_ttl_hours': cacheTtlHours,
        'cache_max_size_mb': cacheMaxSizeMb,
        'cache_sweep_interval_minutes': 60,
        'legacy_allow_insecure_tls': tlsTrustMode == 'insecure',
        'device_name': deviceName,
      };
}

/// Ask 档的入站确认事件载荷。
class ConfirmRequest {
  ConfirmRequest({
    required this.sessionId,
    required this.fromDevice,
    required this.dataType,
    required this.previewSummary,
    required this.totalSize,
    required this.totalItems,
    required this.timeoutSecs,
  });

  final String sessionId;
  final String fromDevice;
  final String dataType;
  final String previewSummary;
  final int totalSize;
  final int totalItems;
  final int timeoutSecs;

  factory ConfirmRequest.fromJson(Map<String, dynamic> j) => ConfirmRequest(
        sessionId: j['session_id'] as String,
        fromDevice: j['from_device'] as String? ?? '',
        dataType: j['data_type'] as String? ?? 'FILES',
        previewSummary: j['preview_summary'] as String? ?? '',
        totalSize: (j['total_size'] as num?)?.toInt() ?? 0,
        totalItems: (j['total_items'] as num?)?.toInt() ?? 0,
        timeoutSecs: (j['timeout_secs'] as num?)?.toInt() ?? 30,
      );
}

String _trustModeOf(dynamic v) {
  if (v == 'pinned') return 'pinned';
  if (v == 'insecure') return 'insecure';
  return 'public_ca';
}

String humanBytes(num n) {
  const kb = 1024.0;
  final v = n.toDouble();
  if (v < kb) return '${n.toInt()} B';
  if (v < kb * kb) return '${(v / kb).toStringAsFixed(1)} KB';
  if (v < kb * kb * kb) return '${(v / kb / kb).toStringAsFixed(1)} MB';
  return '${(v / kb / kb / kb).toStringAsFixed(2)} GB';
}
