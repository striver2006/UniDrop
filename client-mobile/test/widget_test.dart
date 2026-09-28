import 'dart:async';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'package:unidrop_mobile/models.dart';
import 'package:unidrop_mobile/services/platform_service.dart';
import 'package:unidrop_mobile/widgets/adaptive.dart';

void main() {
  test('lib/main.dart 必须包含 main 入口（防脚本误覆盖后静默绿）', () {
    final src = File('lib/main.dart').readAsStringSync();
    expect(RegExp(r'void\s+main\s*\(').hasMatch(src), isTrue,
        reason: 'main.dart 被覆盖成空壳会让 analyze/test 照常全绿，只在构建期爆炸');
  });

  group('retryChannel 启动竞态容错（鸿蒙 fork 插件注册晚于 Dart 启动）', () {
    test('前几次超时后成功——返回最终结果', () async {
      var calls = 0;
      final v = await retryChannel('t', () {
        calls++;
        // 前两次模拟 channel 无回包（永不完成的 Future），第三次成功
        if (calls < 3) return Completer<int>().future;
        return Future.value(7);
      }, timeout: const Duration(milliseconds: 50), interval: Duration.zero);
      expect(v, 7);
      expect(calls, 3);
    });

    test('全部失败——抛 StateError 而不是无限挂起', () async {
      Future<Object> never() => Completer<Object>().future;
      await expectLater(
        retryChannel('t', never,
            attempts: 3,
            timeout: const Duration(milliseconds: 30),
            interval: Duration.zero),
        throwsStateError,
      );
    });
  });

  group('WindowSizeClass 断点（V2 计划 §5.2 的 Pad 适配约定）', () {
    test('compact < 600', () {
      expect(sizeClass(360), WindowSizeClass.compact);
      expect(sizeClass(599.9), WindowSizeClass.compact);
    });

    test('medium 600–840（Pad 竖屏）', () {
      expect(sizeClass(600), WindowSizeClass.medium);
      expect(sizeClass(839.9), WindowSizeClass.medium);
    });

    test('expanded >= 840（Pad 横屏，双栏）', () {
      expect(sizeClass(840), WindowSizeClass.expanded);
      expect(sizeClass(1280), WindowSizeClass.expanded);
    });
  });

  group('模型反序列化（字段名与 Rust serde 严格一致）', () {
    test('OnlineDevice', () {
      final d = OnlineDevice.fromJson({
        'device_id': 'id-1',
        'hostname': 'MacBook Pro',
        'os_type': 'macos',
        'app_version': '0.4.2',
      });
      expect(d.platformLabel, 'macOS');
    });

    test('ActiveTransfer 终态判定', () {
      final t = ActiveTransfer.fromJson({
        'session_id': 's1',
        'preview_summary': 'x',
        'total_size': 10,
        'transferred_size': 10,
        'direction': 'RECEIVE',
        'progress': 100.0,
        'status': 'COMPLETED',
        'data_type': 'TEXT',
      });
      expect(t.isTerminal, isTrue);
      expect(t.isReceive, isTrue);
    });

    test('AppSettingsDto 往返保留关键字段', () {
      final s = AppSettingsDto.fromJson({
        'server_url': 'wss://x',
        'account_id': 'a',
        'psk_secret': 'p',
        'receive_policy': 'wifi_only',
        'e2ee_enabled': true,
      });
      final j = s.toJson();
      expect(j['receive_policy'], 'wifi_only');
      expect(j['e2ee_enabled'], isTrue);
      // 桌面字段全量回填，防止保存时抹掉未展示的设置
      expect(j.containsKey('history_max_entries'), isTrue);
      expect(j.containsKey('transfer_card_retain_secs'), isTrue);

      // 移动端已暴露的存储字段必须透传用户值（曾硬编码 30/24/2048，
      // 用户改完保存会被静默抹回默认）
      final s2 = AppSettingsDto.fromJson({
        ...j,
        'history_max_entries': 50,
        'transfer_card_retain_secs': 10,
        'cache_ttl_hours': 12,
        'cache_max_size_mb': 512,
        'auto_inject': true,
      });
      final j2 = s2.toJson();
      expect(j2['history_max_entries'], 50);
      expect(j2['transfer_card_retain_secs'], 10);
      expect(j2['cache_ttl_hours'], 12);
      expect(j2['cache_max_size_mb'], 512);
      expect(j2['auto_inject'], isTrue);
    });
  });
}
