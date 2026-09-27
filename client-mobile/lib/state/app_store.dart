/// 全局应用状态：桥接 Rust 事件流到 UI 可消费的 ChangeNotifier。
///
/// 事件名与桌面端 Tauri 事件一一对应（两端共用同一套事件协议），
/// 本层只做「事件 → 状态」的翻译，不做业务决策——业务全部在 Rust core。

library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../ffi/bridge.dart';
import '../models.dart';
import '../services/platform_service.dart';

class AppStore extends ChangeNotifier {
  AppStore({PlatformService? platform}) : platform = platform ?? PlatformService();

  final PlatformService platform;
  final NativeBridge native = NativeBridge.instance;

  OnlineDevice? selfInfo;
  List<OnlineDevice> devices = [];
  final transfers = <String, ActiveTransfer>{};
  List<HistoryEntry> history = [];
  AppSettingsDto? settings;
  ServerLimits? serverLimits;
  ConfirmRequest? pendingConfirm;

  /// 设备页当前发送目标（expanded 双栏右侧面板 / 弹层）。
  OnlineDevice? sendTarget;

  /// 历史页 expanded 双栏的聚焦条目。
  String? historyFocus;

  /// 连接状态徽标：auth-success / auth-failed / tls-cert-failed 驱动。
  String connectionState = 'connecting'; // connecting | online | auth_failed | cert_failed
  String? connectionError;
  String? e2eeFallbackNotice;

  /// 一次性 toast（失败原因 / e2ee 回落提示）。
  String? toast;
  void Function(String message)? onToast;

  StreamSubscription<Map<String, dynamic>>? _sub;
  StreamSubscription<void>? _networkSub;
  Timer? _refreshDebounce;

  bool _disposed = false;

  Future<void> bootstrap() async {
    // 全程兜底：native 层的任何异常（FFI 符号缺失、路径不可写…）都必须变成
    // UI 可见的 start_failed 状态——否则 provider 构造抛异常，MaterialApp
    // 根本不会构建，用户看到的是无解释的白屏。
    try {
      final paths = await platform.resolvePaths();
      final deviceName = await platform.deviceName();
      final result = await native.start(
        dbPath: paths.dbPath,
        cacheDir: paths.cacheDir,
        deviceName: deviceName,
      );
      if (result['ok'] == true) {
        selfInfo = OnlineDevice.fromJson((result['data'] as Map).cast<String, dynamic>());
      } else {
        connectionState = 'start_failed';
        connectionError = result['error']?.toString();
        notifyListeners();
        return;
      }

      _networkSub = platform.watchNetwork();
      _sub = native.events.stream.listen(_onEvent, onError: (Object e) {
        debugPrint('event stream error: $e');
      });

      await refreshDevices();
      await refreshSettings();
      await refreshLimits();
      await refreshHistory();
    } catch (e) {
      connectionState = 'start_failed';
      connectionError = '核心初始化失败：$e';
      notifyListeners();
    }
  }

  void _onEvent(Map<String, dynamic> envelope) {
    final event = envelope['event'] as String?;
    final payload = envelope['payload'];
    switch (event) {
      case 'devices-updated':
        devices = ((payload as List?) ?? [])
            .map((e) => OnlineDevice.fromJson((e as Map).cast<String, dynamic>()))
            .where((d) => d.deviceId != selfInfo?.deviceId)
            .toList();
        notifyListeners();
        break;
      case 'transfer-progress':
        final t = ActiveTransfer.fromJson((payload as Map).cast<String, dynamic>());
        transfers[t.sessionId] = t;
        // 终态卡片按设置里的 retain 秒数自动收起；失败卡片永不自动收起
        // （与桌面端同一设计：失败原因必须有稳定入口可看）。
        if (t.status == 'COMPLETED') {
          final retain = settings?.serverUrl == null ? 30 : 30;
          Timer(Duration(seconds: retain), () {
            transfers.remove(t.sessionId);
            if (!_disposed) notifyListeners();
          });
          _scheduleHistoryRefresh();
        } else if (t.status == 'FAILED') {
          _scheduleHistoryRefresh();
        }
        notifyListeners();
        break;
      case 'transfer-offer-received':
        // WifiOnly/Always 下的自动接收：Rust 已应答，这里只刷新历史
        _scheduleHistoryRefresh();
        break;
      case 'confirm-receive':
        pendingConfirm =
            ConfirmRequest.fromJson((payload as Map).cast<String, dynamic>());
        notifyListeners();
        break;
      case 'confirm-expired':
        final sid = (payload as Map?)?['session_id'] as String?;
        if (pendingConfirm?.sessionId == sid) {
          pendingConfirm = null;
          _toast('接收确认已超时，本次传输被拒绝');
          notifyListeners();
        }
        break;
      case 'auth-success':
        connectionState = 'online';
        connectionError = null;
        notifyListeners();
        break;
      case 'auth-failed':
        connectionState = 'auth_failed';
        connectionError = payload?.toString();
        _toast('连接被服务端拒绝：请检查账号与密钥');
        notifyListeners();
        break;
      case 'tls-cert-failed':
        connectionState = 'cert_failed';
        connectionError = payload?.toString();
        _toast('服务器证书校验失败，请在设置中检查信任配置');
        notifyListeners();
        break;
      case 'server-limits-updated':
        serverLimits = payload == null
            ? null
            : ServerLimits.fromJson((payload as Map).cast<String, dynamic>());
        notifyListeners();
        break;
      case 'e2ee-fallback':
        e2eeFallbackNotice = payload?.toString();
        _toast(e2eeFallbackNotice!);
        notifyListeners();
        break;
      case 'e2ee-offer-rejected':
        _toast(payload?.toString() ?? '收到无法解密的传输，已拒收');
        notifyListeners();
        break;
      case 'history-pruned':
      case 'account-changed':
        refreshHistory();
        break;
      case 'show-notification':
        final map = (payload as Map?)?.cast<String, dynamic>() ?? {};
        _toast('${map['title'] ?? ''}\n${map['body'] ?? ''}');
        break;
      case 'clipboard-write':
        _handleClipboardWrite((payload as Map).cast<String, dynamic>());
        break;
      default:
        break;
    }
  }

  /// Rust 侧「写剪贴板」请求（接收 TEXT/IMAGE 自动注入）。
  /// 尽力而为语义：写完提示，失败也只提示。
  Future<void> _handleClipboardWrite(Map<String, dynamic> payload) async {
    final kind = payload['kind'] as String?;
    try {
      if (kind == 'text') {
        await platform.writeClipboardText(payload['text'] as String? ?? '');
        _toast('已写入系统剪贴板，可直接粘贴');
      } else if (kind == 'image') {
        // 图片剪贴板写入需要平台通道支持，Flutter Clipboard 仅文本。
        // 收到的图片已保存在收件目录，引导用户从历史卡片分享/保存。
        _toast('已接收图片（保存在收件目录），可在历史中分享');
      } else if (kind == 'files') {
        _toast('文件已保存到收件目录，可在历史中分享');
      }
    } catch (e) {
      _toast('写入剪贴板失败: $e');
    }
  }

  void _toast(String message) {
    toast = message;
    onToast?.call(message);
  }

  /// 公开的用户可见提示通道：给 sheet/页面关闭后的异步续行用
  /// （那种场景下原 context 已失效，ScaffoldMessenger 拿不到）。
  void notifyUser(String message) => _toast(message);

  void clearToast() {
    toast = null;
    notifyListeners();
  }

  /// 历史刷新去抖：一次传输完成会连发多个事件，逐个刷新只会白查。
  void _scheduleHistoryRefresh() {
    _refreshDebounce?.cancel();
    _refreshDebounce = Timer(const Duration(milliseconds: 400), () {
      if (!_disposed) refreshHistory();
    });
  }

  Future<void> refreshDevices() async {
    try {
      final data = await native.invokeData('get_devices');
      devices = ((data as List?) ?? [])
          .map((e) => OnlineDevice.fromJson((e as Map).cast<String, dynamic>()))
          .where((d) => d.deviceId != selfInfo?.deviceId)
          .toList();
      notifyListeners();
    } catch (e) {
      debugPrint('refreshDevices: $e');
    }
  }

  Future<void> refreshSettings() async {
    try {
      final data = await native.invokeData('get_settings');
      settings = AppSettingsDto.fromJson((data as Map).cast<String, dynamic>());
      notifyListeners();
    } catch (e) {
      debugPrint('refreshSettings: $e');
    }
  }

  Future<void> refreshLimits() async {
    try {
      final data = await native.invokeData('get_server_limits');
      serverLimits = data == null
          ? null
          : ServerLimits.fromJson((data as Map).cast<String, dynamic>());
      notifyListeners();
    } catch (e) {
      debugPrint('refreshLimits: $e');
    }
  }

  Future<void> refreshHistory() async {
    try {
      final data = await native.invokeData('list_history');
      history = ((data as List?) ?? [])
          .map((e) => HistoryEntry.fromJson((e as Map).cast<String, dynamic>()))
          .toList();
      notifyListeners();
    } catch (e) {
      debugPrint('refreshHistory: $e');
    }
  }

  Future<void> saveSettings(AppSettingsDto next) async {
    await native.invokeData('save_settings', {'settings': next.toJson()});
    settings = next;
    notifyListeners();
  }

  /// 发送文件（路径来自 file_picker）。
  Future<void> sendFiles(String targetDevice, List<String> paths) async {
    await native.invokeData(
        'send_files', {'target_device': targetDevice, 'paths': paths});
  }

  /// 设备页「选择文件发送」：pick → 发给当前目标。
  Future<void> pickAndSendFiles() async {
    final target = sendTarget;
    if (target == null) return;
    final paths = await platform.pickFiles();
    if (paths == null || paths.isEmpty) return;
    await sendFiles(target.deviceId, paths);
    _toast('已发起发送：${paths.length} 个文件');
  }

  void setSendTarget(OnlineDevice device) {
    sendTarget = device;
    notifyListeners();
  }

  /// 读一个 TEXT 会话的内容（历史「复制文本」用）。读不到返回 null。
  Future<String?> readSessionText(String sessionId) async {
    try {
      final files = await sessionFiles(sessionId);
      if (files.isEmpty) return null;
      final bytes = await File(files.first).readAsBytes();
      return utf8.decode(bytes);
    } catch (_) {
      return null;
    }
  }

  /// 发送剪贴板文本。
  Future<void> sendClipboardText(String targetDevice) async {
    final text = await platform.readClipboardText();
    if (text == null) {
      _toast('剪贴板没有可发送的文本');
      return;
    }
    await native.invokeData(
        'send_text', {'target_device': targetDevice, 'text': text});
  }

  /// Ask 档确认应答。
  Future<void> respondOffer(String sessionId, bool accept) async {
    pendingConfirm = null;
    notifyListeners();
    try {
      await native.invokeData(
          'respond_offer', {'session_id': sessionId, 'accept': accept});
    } on StateError catch (e) {
      // 最常见：超时看门狗已把它拒掉。界面撤弹窗即可，不重复报错。
      debugPrint('respondOffer: $e');
      _toast('该传输已超时拒收');
    }
  }

  /// 取一个会话的文件路径（分享 / 打开用）。
  Future<List<String>> sessionFiles(String sessionId) async {
    final data =
        await native.invokeData('get_session_files', {'session_id': sessionId});
    final paths = ((data as Map)['paths'] as List?) ?? [];
    return paths.map((e) => e.toString()).toList();
  }

  /// 回前台：跳过重连退避。
  void resume() {
    native.invoke('reconnect');
  }

  @override
  void dispose() {
    _disposed = true;
    _sub?.cancel();
    _networkSub?.cancel();
    _refreshDebounce?.cancel();
    super.dispose();
  }
}
