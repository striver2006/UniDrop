/// 排障落盘日志：鸿蒙上 Dart print 没有可靠出口（stderr 不进 hilog；fork 的
/// attach 因 listViews 为空不可用，VM service 的 Stdout 捕获也拿不到），
/// 面包屑双写到 hap 缓存目录，用 hdc file recv 拉取。
///
/// 与 Rust 侧 unidrop-debug.log（{cache_dir} 下）互补：本文件覆盖 native.start
/// 之前的 Dart 段（path_provider / device_info / dlopen）。
library;

import 'dart:io';

void dbgLog(String msg) {
  // ignore: avoid_print
  print('[unidrop] $msg');
  try {
    File('/data/storage/el2/base/haps/entry/cache/unidrop-bootstrap.log')
        .writeAsStringSync('${DateTime.now().toIso8601String()} $msg\n',
            mode: FileMode.append, flush: true);
  } catch (_) {
    // 非鸿蒙平台该路径不存在——print 已尽力，不落盘不致命
  }
}

/// 鸿蒙运行时探测。实测（HarmonyOS 6.1.1 + fork 3.7.12）：
/// Platform.operatingSystem 返回 'ohos'；但为兼容「伪装 android」的旧
/// fork/旧机型说法，双保险——ohos 沙箱的 .so 安装路径在真实 Android 上
/// 不存在（dlopen 能命中它说明一定可读）。
bool isOhosRuntime() =>
    Platform.operatingSystem == 'ohos' ||
    (Platform.isAndroid &&
        File('/data/storage/el2/base/libs/arm64/libunidrop_mobile.so')
            .existsSync());
