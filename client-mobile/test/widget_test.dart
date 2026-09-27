import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'package:unidrop_mobile/models.dart';
import 'package:unidrop_mobile/widgets/adaptive.dart';

void main() {
  test('lib/main.dart 必须包含 main 入口（防脚本误覆盖后静默绿）', () {
    final src = File('lib/main.dart').readAsStringSync();
    expect(RegExp(r'void\s+main\s*\(').hasMatch(src), isTrue,
        reason: 'main.dart 被覆盖成空壳会让 analyze/test 照常全绿，只在构建期爆炸');
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
    });
  });
}
