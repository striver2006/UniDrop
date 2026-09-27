/// 设备页：在线设备列表，点开快捷发送（文件 / 剪贴板文本）。
/// expanded 断点右侧固定展示发送面板（Pad 双栏）。

library;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models.dart';
import '../state/app_store.dart';
import '../widgets/adaptive.dart';

class DevicesPage extends StatelessWidget {
  const DevicesPage({super.key});

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    final class_ = sizeClass(MediaQuery.sizeOf(context).width);

    final list = ListView.builder(
      padding: const EdgeInsets.symmetric(vertical: 8),
      itemCount: store.devices.length,
      itemBuilder: (context, i) => _DeviceTile(device: store.devices[i]),
    );

    final body = class_.useTwoPane
        ? Row(children: [
            SizedBox(width: 360, child: list),
            const VerticalDivider(width: 0),
            const Expanded(child: _SendPanel()),
          ])
        : list;

    return Scaffold(
      appBar: AppBar(title: const Text('设备')),
      body: store.devices.isEmpty
          ? const _EmptyDevices()
          : body,
    );
  }
}

class _EmptyDevices extends StatelessWidget {
  const _EmptyDevices();

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    final theme = Theme.of(context);
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.devices_other,
              size: 56, color: theme.colorScheme.onSurfaceVariant),
          const SizedBox(height: 12),
          switch (store.connectionState) {
            'connecting' => const Text('正在连接服务器…'),
            'auth_failed' => const Text('鉴权失败：请检查设置中的账号与密钥'),
            'cert_failed' => const Text('证书校验失败：请检查 TLS 信任配置'),
            'start_failed' => Text('启动失败：${store.connectionError ?? ''}'),
            _ => const Text('暂无其他设备在线'),
          },
        ],
      ),
    );
  }
}

class _DeviceTile extends StatelessWidget {
  const _DeviceTile({required this.device});

  final OnlineDevice device;

  @override
  Widget build(BuildContext context) {
    final store = context.read<AppStore>();
    final selected = store.sendTarget?.deviceId == device.deviceId;

    return ListTile(
      leading: _platformIcon(device),
      title: Text(device.hostname),
      subtitle: Text('${device.platformLabel} · v${device.appVersion}'),
      trailing: const Icon(Icons.chevron_right),
      selected: selected,
      onTap: () {
        final store = context.read<AppStore>();
        store.setSendTarget(device);
        final class_ = sizeClass(MediaQuery.sizeOf(context).width);
        if (!class_.useTwoPane) {
          showModalBottomSheet(
            context: context,
            showDragHandle: true,
            builder: (_) => const _SendPanel(),
          );
        }
      },
    );
  }

  Widget _platformIcon(OnlineDevice d) {
    final icon = switch (d.osType) {
      'macos' => Icons.laptop_mac,
      'windows' => Icons.laptop_windows,
      'linux' => Icons.terminal,
      'ios' => Icons.phone_iphone,
      'android' => Icons.phone_android,
      'ohos' => Icons.smartphone,
      _ => Icons.device_unknown,
    };
    return CircleAvatar(child: Icon(icon, size: 22));
  }
}

/// 快捷发送面板：发给目标设备（文件 / 剪贴板文本）。
class _SendPanel extends StatelessWidget {
  const _SendPanel();

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    final target = store.sendTarget;
    final theme = Theme.of(context);

    if (target == null) {
      return Center(
        child: Text(
          '选择左侧设备开始发送',
          style: theme.textTheme.bodyLarge
              ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
        ),
      );
    }

    return Padding(
      padding: const EdgeInsets.all(20),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              CircleAvatar(child: Text(target.hostname.characters.first)),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  '发送到 ${target.hostname}',
                  style: theme.textTheme.titleMedium,
                ),
              ),
            ],
          ),
          const SizedBox(height: 20),
          FilledButton.icon(
            icon: const Icon(Icons.folder_open),
            label: const Text('选择文件发送'),
            onPressed: () => store.pickAndSendFiles(),
          ),
          const SizedBox(height: 10),
          OutlinedButton.icon(
            icon: const Icon(Icons.content_paste),
            label: const Text('发送剪贴板文本'),
            onPressed: () => store.sendClipboardText(target.deviceId),
          ),
          const SizedBox(height: 16),
          Text(
            '目录不会被展开；大小与数量受服务端限额约束。',
            style: theme.textTheme.bodySmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}
