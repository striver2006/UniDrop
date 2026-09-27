/// 平台能力服务：路径、设备名、网络类型监控、剪贴板、分享、文件选择。
///
/// 鸿蒙（华为 Flutter 分支）上部分插件由 openharmony-sig 适配仓库提供同 API
/// 实现；个别插件缺位时本层做能力探测与降级（UI 隐藏对应入口），
/// 业务代码不直接感知平台差异。


library;

import 'dart:async';
import 'dart:io';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:device_info_plus/device_info_plus.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';
import 'package:share_plus/share_plus.dart';

import '../ffi/bridge.dart';

class PlatformService {
  /// 数据库与收件目录。对应 V2 计划 §4 的路径三分策略：
  /// - SQLite → 应用支撑目录（参与系统备份，绝不可放缓存区）
  /// - 收件缓存 → 文档目录下的 UniDrop（iOS 文件 App 可见；
  ///   Android/ohos 外部私有目录）
  Future<({String dbPath, String cacheDir})> resolvePaths() async {
    final support = await getApplicationSupportDirectory();
    final docs = await getApplicationDocumentsDirectory();
    return (
      dbPath: '${support.path}/unidrop.db',
      cacheDir: '${docs.path}/UniDrop',
    );
  }

  /// 平台设备名（连接层上报的 hostname）。
  Future<String> deviceName() async {
    final info = DeviceInfoPlugin();
    try {
      if (Platform.isIOS) {
        final ios = await info.iosInfo;
        return ios.name; // 用户可自定义的设备名（「张三的 iPhone」）
      }
      if (Platform.isAndroid) {
        final android = await info.androidInfo;
        return android.model;
      }
    } catch (_) {
      // 取不到就退通用名，不该挡启动
    }
    return 'UniClip Mobile';
  }

  /// 持续把网络类型上报给 Rust（WifiOnly 接收策略的数据源）。
  /// 启动即报一次，之后每次变化再报——切网那一瞬就该换策略。
  StreamSubscription<void> watchNetwork() {
    String map(List<ConnectivityResult> results) {
      if (results.contains(ConnectivityResult.ethernet)) return 'ethernet';
      if (results.contains(ConnectivityResult.wifi)) return 'wifi';
      if (results.contains(ConnectivityResult.mobile)) return 'cellular';
      return 'unknown';
    }

    Future<void> report({bool changed = false}) async {
      try {
        final results = await Connectivity().checkConnectivity();
        await NativeBridge.instance
            .invoke('set_network', {'kind': map(results)});
        // 切网即重连：旧链路在网切换后大概率已半开（TCP 无感知死亡），
        // 与 core 侧 45s read 超时构成双保险——这里把发现延迟从 45s 压到毫秒级。
        if (changed) {
          await NativeBridge.instance.invoke('reconnect');
        }
      } catch (_) {
        // 探测失败按 unknown 上报过一次兜底即可，不断流
        await NativeBridge.instance.invoke('set_network', {'kind': 'unknown'});
      }
    }

    report();
    late final StreamSubscription<void> sub;
    sub = Connectivity().onConnectivityChanged.listen((_) => report(changed: true));
    return sub;
  }

  /// 读系统剪贴板文本（发送「剪贴板文本」用）。空返回 null。
  Future<String?> readClipboardText() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text;
    if (text == null || text.isEmpty) return null;
    return text;
  }

  /// 选文件（多选）。返回可读路径——file_picker 在 iOS/Android 上都会把
  /// 选中内容落到应用可读位置，Rust 侧因此不需要感知 content://。
  Future<List<String>?> pickFiles() async {
    final result = await FilePicker.platform.pickFiles(allowMultiple: true);
    if (result == null) return null;
    return result.files
        .where((f) => f.path != null)
        .map((f) => f.path!)
        .toList();
  }

  /// 分享一组文件（接收后的「保存到… / 用其他应用打开」等价路径）。
  Future<void> shareFiles(List<String> paths, {String? subject}) async {
    await SharePlus.instance.share(
      ShareParams(files: paths.map((p) => XFile(p)).toList(), subject: subject),
    );
  }

  /// 分享纯文本。
  Future<void> shareText(String text) async {
    await SharePlus.instance.share(ShareParams(text: text));
  }

  /// 写文本到系统剪贴板（接收 TEXT 自动注入 / 手动复制用）。
  Future<void> writeClipboardText(String text) async {
    await Clipboard.setData(ClipboardData(text: text));
  }
}
