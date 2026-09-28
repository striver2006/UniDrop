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

import '../dbg.dart';
import '../ffi/bridge.dart';

/// 路径三分配置（record 是 Dart 3 语法，2.19 兼容用小类）。
class PathConfig {
  PathConfig({required this.dbPath, required this.cacheDir});

  final String dbPath;
  final String cacheDir;
}

/// 鸿蒙 fork 的引擎编排有启动竞态：FlutterAbility.onCreate 里 await
/// onAttach（插件注册在其中），但系统的 onWindowStageCreate 不等它，并发
/// 触发 Dart entrypoint——bootstrap 早期的 channel 调用可能先于插件注册
/// 到达引擎，被静默丢弃（无 handler 无回包），Future 永不完成
/// （真机症状：设置页无限转圈）。启动期 channel 调用统一走超时重试。
Future<T> retryChannel<T>(String label, Future<T> Function() call,
    {int attempts = 8,
    Duration timeout = const Duration(seconds: 2),
    Duration interval = const Duration(milliseconds: 300)}) async {
  Object? lastError;
  for (var i = 0; i < attempts; i++) {
    try {
      return await call().timeout(timeout);
    } catch (e) {
      lastError = e;
      dbgLog('retryChannel($label): 第${i + 1}/$attempts次失败: $e');
      await Future<void>.delayed(interval);
    }
  }
  throw StateError('$label 重试 $attempts 次仍失败: $lastError');
}

class PlatformService {
  /// 数据库与收件目录。对应 V2 计划 §4 的路径三分策略：
  /// - SQLite → 应用支撑目录（参与系统备份，绝不可放缓存区）
  /// - 收件缓存 → 文档目录下的 UniDrop（iOS 文件 App 可见；
  ///   Android/ohos 外部私有目录）
  Future<PathConfig> resolvePaths() async {
    // 鸿蒙旁路：实测（Pura 70 Ultra / HarmonyOS 6.1.1 / fork 3.7.12）该设备上
    // Dart→ArkTS 的 platform channel 整体断流（连 flutter/platform 系统通道
    // 都无回包），path_provider 的 pigeon 调用永不返回。沙箱路径是固定布局，
    // 直接给出；语义对齐 path_provider_ohos（documents=filesDir/flutter，
    // support=filesDir）。与 lib/ffi/bridge.dart 的 dlopen 绝对路径同属一类
    // 「鸿蒙固定沙箱路径」事实来源。
    if (isOhosRuntime()) {
      const filesDir = '/data/storage/el2/base/haps/entry/files';
      return PathConfig(
        dbPath: '$filesDir/unidrop.db',
        cacheDir: '$filesDir/flutter/UniDrop',
      );
    }
    final support =
        await retryChannel('supportDir', () => getApplicationSupportDirectory());
    final docs =
        await retryChannel('documentsDir', () => getApplicationDocumentsDirectory());
    return PathConfig(
      dbPath: '${support.path}/unidrop.db',
      cacheDir: '${docs.path}/UniDrop',
    );
  }

  /// 平台设备名（连接层上报的 hostname）。
  Future<String> deviceName() async {
    // 鸿蒙：channel 断流，device_info 不可用，先退通用名（机型名待 channel
    // 修复后接 ohosInfo.marketName）。
    if (isOhosRuntime()) return 'UniClip Mobile';
    final info = DeviceInfoPlugin();
    try {
      if (Platform.isIOS) {
        final ios = await retryChannel('iosInfo', () => info.iosInfo);
        return ios.name; // 用户可自定义的设备名（「张三的 iPhone」）
      }
      if (Platform.isAndroid) {
        final android = await retryChannel('androidInfo', () => info.androidInfo);
        return android.model;
      }
    } catch (_) {
      // 取不到就退通用名，不该挡启动
    }
    return 'UniClip Mobile';
  }

  /// 持续把网络类型上报给 Rust（WifiOnly 接收策略的数据源）。
  /// 启动即报一次，之后每次变化再报——切网那一瞬就该换策略。
  ///
  /// 重连只在网络**类型真正变化**时触发（wifi→cellular 等）。
  /// connectivity_plus 在部分 ROM 上会对同型波动频繁回调（Wi-Fi 信号
  /// 重估、IPv6 刷新都算"变化"）；若照单全收地断开重连，正在握手的
  /// 传输（OFFER 已发、带 token 的 ANSWER 在途）会被窗口期吞掉——
  /// Mi 10 → iPhone 的图片传输失败即此（receiver 已连数据面干等，
  /// sender 的 ANSWER 随断连丢失，60s 后服务端判 idle 失败）。
  /// 半开死链的兜底仍由 core 侧 45s read 超时负责。
  StreamSubscription<void> watchNetwork() {
    // connectivity 的返回形态随大版本变化（4.x 单枚举、6.x List），
    // 鸿蒙（pubspec.ohos.yaml 钉 4.x）与 Android/iOS（主 pubspec 钉 6.x）
    // 共用本文件——运行时判别，两种形态都吃。
    String map(dynamic result) {
      if (result is List) {
        if (result.contains(ConnectivityResult.ethernet)) return 'ethernet';
        if (result.contains(ConnectivityResult.wifi)) return 'wifi';
        if (result.contains(ConnectivityResult.mobile)) return 'cellular';
        return 'unknown';
      }
      final ConnectivityResult r = result as ConnectivityResult;
      switch (r) {
        case ConnectivityResult.ethernet:
          return 'ethernet';
        case ConnectivityResult.wifi:
          return 'wifi';
        case ConnectivityResult.mobile:
          return 'cellular';
        default:
          return 'unknown';
      }
    }

    String? lastKind;

    Future<void> report({bool changed = false}) async {
      try {
        final results = await Connectivity()
            .checkConnectivity()
            .timeout(const Duration(seconds: 2));
        final kind = map(results);
        final previous = lastKind;
        lastKind = kind;
        await NativeBridge.instance.invoke('set_network', {'kind': kind});
        // 只在类型真正变化时重连：同型波动（信号重估等）断开重连
        // 会吞掉在途的传输握手。
        if (changed && previous != null && previous != kind) {
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
    try {
      // 鸿蒙 channel 断流时 Clipboard 永不回包，超时兜底防 UI 卡死
      final data = await Clipboard.getData(Clipboard.kTextPlain)
          .timeout(const Duration(seconds: 2));
      final text = data?.text;
      if (text == null || text.isEmpty) return null;
      return text;
    } catch (_) {
      return null;
    }
  }

  /// 选文件（多选）。返回可读路径——file_picker 在 iOS/Android 上都会把
  /// 选中内容落到应用可读位置，Rust 侧因此不需要感知 content://。
  ///
  /// 鸿蒙暂无 file_picker 的 ohos 实现（openharmony-sig 适配在途），且
  /// channel 断流会让调用永不返回：直接返回 null，UI 提示「暂不支持」，
  /// 不让一次能力缺失演成崩溃或挂起。
  Future<List<String>?> pickFiles() async {
    if (isOhosRuntime()) return null;
    try {
      final result = await FilePicker.platform.pickFiles(allowMultiple: true);
      if (result == null) return null;
      return result.files
          .where((f) => f.path != null)
          .map((f) => f.path!)
          .toList();
    } on MissingPluginException {
      return null;
    }
  }

  /// 分享一组文件（接收后的「保存到… / 用其他应用打开」等价路径）。
  /// 鸿蒙暂无 ohos 实现：抛出的 MissingPluginException 转成失败结果由调用方提示。
  Future<void> shareFiles(List<String> paths, {String? subject}) async {
    if (isOhosRuntime()) throw UnsupportedError('该平台暂不支持系统分享');
    try {
      // share_plus 11 的实例 API 在鸿蒙变体（7.0.2）不存在，两端共用旧静态 API。
      // ignore: deprecated_member_use
      await Share.shareXFiles(paths.map((p) => XFile(p)).toList(),
          subject: subject);
    } on MissingPluginException {
      throw UnsupportedError('该平台暂不支持系统分享');
    }
  }

  /// 分享纯文本。
  Future<void> shareText(String text) async {
    if (isOhosRuntime()) throw UnsupportedError('该平台暂不支持系统分享');
    try {
      // ignore: deprecated_member_use —— 同上，两端交集
      await Share.share(text);
    } on MissingPluginException {
      throw UnsupportedError('该平台暂不支持系统分享');
    }
  }

  /// 写文本到系统剪贴板（接收 TEXT 自动注入 / 手动复制用）。
  /// 返回是否真正写入；鸿蒙 channel 断流时超时返回 false，
  /// 调用方据此换提示文案（文本本体已在收件目录）。
  Future<bool> writeClipboardText(String text) async {
    try {
      await Clipboard.setData(ClipboardData(text: text))
          .timeout(const Duration(seconds: 2));
      return true;
    } catch (_) {
      return false;
    }
  }
}
