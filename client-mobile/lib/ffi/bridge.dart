/// Rust FFI 桥：与 `client-mobile/native`（unidrop-mobile-native）的 C ABI 对接。
///
/// 契约见 native/src/lib.rs 的模块文档：
/// - `invoke` 全部异步——立即返回 call_id，结果经 `invoke-result` 事件投递；
/// - 事件经 `NativeCallable.listener` 回调（任意线程安全）进入 Dart，
///   以 `Stream<Map>` 广播给 UI；
/// - Rust 返回的字符串必须经 `unidrop_free_string` 释放。
library;

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

import '../dbg.dart';

typedef _PollEventC = Pointer<Utf8> Function();
typedef _PollEventDart = Pointer<Utf8> Function();

typedef _StartC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _StartDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _InvokeC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _InvokeDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeStringC = Void Function(Pointer<Utf8>);
typedef _FreeStringDart = void Function(Pointer<Utf8>);


class NativeBridge {
  NativeBridge._();

  static final NativeBridge instance = NativeBridge._();

  DynamicLibrary? _lib;
  bool started = false;

  /// 待完成的 invoke 调用（call_id → completer）。
  final _pending = <String, Completer<Map<String, dynamic>>>{};

  /// Rust → Dart 的事件流：`{"event": name, "payload": …}`。
  /// UI 层订阅它刷新设备列表 / 传输卡片 / 确认弹窗。
  final events = StreamController<Map<String, dynamic>>.broadcast();

  /// 事件轮询定时器（33ms ≈ 30fps 的事件粒度，FFI 调用为纳秒级，开销可忽略）。
  /// Dart 2.19（鸿蒙 3.7 基座）无 NativeCallable，轮询是跨版本统一通道。
  Timer? _pollTimer;

  DynamicLibrary _open() {
    if (_lib != null) return _lib!;
    if (Platform.isIOS || Platform.isMacOS) {
      // 静态链接进 App 二进制，从自身进程符号表找。
      _lib = DynamicLibrary.process();
      return _lib!;
    }
    // Android 与鸿蒙共用此分支：不同 fork/机型上 Platform 标识不稳定
    // （实测 HarmonyOS 6.1.1 报 'ohos'，早期记录曾见伪装 android），
    // 不据此分支，按加载行为区分。鸿蒙的 dlopen 搜索路径不含应用 libs
    // 目录（无 classloader namespace 机制），HAP 的 native 库安装在
    // el1 bundle 目录（历史包曾落 el2/base），两处都试，最后回落裸名。
    const ohosLibPaths = [
      '/data/storage/el1/bundle/libs/arm64/libunidrop_mobile.so',
      '/data/storage/el2/base/libs/arm64/libunidrop_mobile.so',
    ];
    for (final p in ohosLibPaths) {
      try {
        _lib = DynamicLibrary.open(p);
        dbgLog('dlopen: 命中 $p');
        return _lib!;
      } on ArgumentError {
        // 该路径不存在，试下一个
      }
    }
    dbgLog('dlopen: 绝对路径均未命中，回落裸名');
    _lib = DynamicLibrary.open('libunidrop_mobile.so');
    return _lib!;
  }

  /// 注册事件回调并启动核心。必须在 UI isolate 调用（NativeCallable.listener
  /// 的投递目标是它创建时的 isolate）。settings 非空时作为启动覆盖
  /// （鸿蒙调试注入口，见 app_store._consumeOhosSetupOverride）。
  Future<Map<String, dynamic>> start({
    required String dbPath,
    required String cacheDir,
    required String deviceName,
    Map<String, dynamic>? settings,
  }) {
    final lib = _open();
    final poll = lib.lookupFunction<_PollEventC, _PollEventDart>('unidrop_poll_event');
    _pollTimer?.cancel();
    _pollTimer = Timer.periodic(const Duration(milliseconds: 33), (_) {
      // 一次 tick 排空积压（传输高峰期事件可能连发）
      while (true) {
        final ptr = poll();
        if (ptr == nullptr) break;
        final text = ptr.toDartString();
        _free(ptr);
        _onEventText(text);
      }
    });

    final config = jsonEncode({
      'db_path': dbPath,
      'cache_dir': cacheDir,
      'device_name': deviceName,
      if (settings != null) 'settings': settings,
    });
    final result = _callStart(config);
    started = result['ok'] == true;
    return Future.value(result);
  }

  Map<String, dynamic> _callStart(String config) {
    final lib = _open();
    final fn = lib.lookupFunction<_StartC, _StartDart>('unidrop_start');
    final req = config.toNativeUtf8();
    final resp = fn(req);
    malloc.free(req);
    return _takeJson(resp);
  }

  Map<String, dynamic> _takeJson(Pointer<Utf8> ptr) {
    final text = ptr.toDartString();
    _free(ptr);
    return jsonDecode(text) as Map<String, dynamic>;
  }

  void _free(Pointer<Utf8> ptr) {
    final lib = _open();
    final fn = lib.lookupFunction<_FreeStringC, _FreeStringDart>(
        'unidrop_free_string');
    fn(ptr);
  }

  void _onEventText(String text) {
    try {
      final decoded = jsonDecode(text);
      if (decoded is Map<String, dynamic>) {
        final eventName = decoded['event'];
        if (eventName == 'invoke-result') {
          final payload = decoded['payload'] as Map<String, dynamic>;
          final callId = payload['call_id'] as String?;
          if (callId != null) {
            final completer = _pending.remove(callId);
            if (completer != null && !completer.isCompleted) {
              completer.complete(payload);
            }
          }
        }
        events.add(decoded);
      }
    } catch (e) {
      // 事件解码失败不该断流：记日志继续
      // ignore: avoid_print
      print('native event decode error: $e');
    }
  }

  /// 发一条命令并等待结果。永不阻塞 UI isolate——Rust 侧立即入队返回，
  /// 真正的结果经事件回调回来。
  Future<Map<String, dynamic>> invoke(String cmd,
      [Map<String, dynamic>? args]) {
    final completer = Completer<Map<String, dynamic>>();
    final body = jsonEncode({'cmd': cmd, ...?args});
    final lib = _open();
    final fn = lib.lookupFunction<_InvokeC, _InvokeDart>('unidrop_invoke');
    final req = body.toNativeUtf8();
    final resp = _takeJson(fn(req));
    malloc.free(req);
    if (resp['ok'] != true) {
      return Future.error(StateError(resp['error']?.toString() ?? 'invoke failed'));
    }
    final callId = resp['call_id'] as String;
    _pending[callId] = completer;
    return completer.future.timeout(const Duration(seconds: 120),
        onTimeout: () {
      _pending.remove(callId);
      return {'ok': false, 'error': '命令执行超时: $cmd'};
    });
  }

  /// 便捷封装：成功返回 data，失败抛异常。
  Future<dynamic> invokeData(String cmd, [Map<String, dynamic>? args]) async {
    final resp = await invoke(cmd, args);
    if (resp['ok'] == true) {
      return resp['data'];
    }
    throw StateError(resp['error']?.toString() ?? 'unknown error');
  }

  void shutdown() {
    final lib = _open();
    lib.lookupFunction<Void Function(), void Function()>('unidrop_stop')();
  }
}
