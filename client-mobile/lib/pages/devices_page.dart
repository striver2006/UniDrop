/// 设备页：在线设备列表，点开快捷发送（文件 / 剪贴板文本）。
/// expanded 断点右侧固定展示发送面板（Pad 双栏）。

library;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../dbg.dart';
import '../models.dart';
import '../state/app_store.dart';
import '../widgets/adaptive.dart';
import '../widgets/ohos_keyboard.dart';

class DevicesPage extends StatelessWidget {
  const DevicesPage({super.key});

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    final class_ = sizeClass(MediaQuery.of(context).size.width);

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

  // Dart 2.19 兼容（鸿蒙 3.7 基座）：switch 表达式降级为 if 链
  Widget _connectionStateText(
      String state, String? error, ThemeData theme) {
    switch (state) {
      case 'connecting':
        return const Text('正在连接服务器…');
      case 'auth_failed':
        return const Text('鉴权失败：请检查设置中的账号与密钥');
      case 'cert_failed':
        return const Text('证书校验失败：请检查 TLS 信任配置');
      case 'start_failed':
        return Text('启动失败：${error ?? ''}');
    }
    return const Text('暂无其他设备在线');
  }

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
          _connectionStateText(store.connectionState, store.connectionError, theme),
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
        final class_ = sizeClass(MediaQuery.of(context).size.width);
        if (!class_.useTwoPane) {
          showModalBottomSheet(
            context: context,
            // 鸿蒙手动输入会带屏上键盘，面板超高需可滚动+占满高度
            isScrollControlled: true,
            builder: (_) => const _SendPanel(),
          );
        }
      },
    );
  }

  Widget _platformIcon(OnlineDevice d) {
    final IconData icon;
    switch (d.osType) {
      case 'macos':
        icon = Icons.laptop_mac;
        break;
      case 'windows':
        icon = Icons.laptop_windows;
        break;
      case 'linux':
        icon = Icons.terminal;
        break;
      case 'ios':
        icon = Icons.phone_iphone;
        break;
      case 'android':
        icon = Icons.phone_android;
        break;
      case 'ohos':
        icon = Icons.smartphone;
        break;
      default:
        icon = Icons.device_unknown;
    }
    return CircleAvatar(child: Icon(icon, size: 22));
  }
}

/// 快捷发送面板：发给目标设备（文件 / 剪贴板文本 / 鸿蒙手动输入）。
class _SendPanel extends StatefulWidget {
  const _SendPanel();

  @override
  State<_SendPanel> createState() => _SendPanelState();
}

class _SendPanelState extends State<_SendPanel> {
  final _manualCtl = TextEditingController();
  OhosKeyboardTarget? _kbdTarget;
  final bool _isOhos = isOhosRuntime();

  @override
  void dispose() {
    _manualCtl.dispose();
    super.dispose();
  }

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

    return SingleChildScrollView(
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
            onPressed: () async {
              // 发起成功即收面板——传输卡在「传输」页，面板留着只会挡住反馈
              final sent = await store.pickAndSendFiles();
              if (sent && context.mounted) Navigator.of(context).maybePop();
            },
          ),
          const SizedBox(height: 10),
          OutlinedButton.icon(
            icon: const Icon(Icons.content_paste),
            label: const Text('发送剪贴板文本'),
            onPressed: () async {
              final sent = await store.sendClipboardText(target.deviceId);
              if (sent && context.mounted) Navigator.of(context).maybePop();
            },
          ),
          // 鸿蒙 channel 断流期間剪贴板/系统键盘都不可用：手动输入是主路径。
          // 1.0.4 引擎后 channel 已修复——kOhosNativeKeyboard 时用原生
          // TextField（系统键盘，含中文输入法），断流回退保留屏上键盘。
          if (_isOhos) ...[
            const SizedBox(height: 16),
            if (kOhosNativeKeyboard)
              TextField(
                controller: _manualCtl,
                autocorrect: false,
                enableSuggestions: false,
                decoration: const InputDecoration(
                  labelText: '手动输入文本',
                  hintText: '剪贴板不可读时的发送入口',
                  border: OutlineInputBorder(),
                ),
              )
            else
              OhosField(
                target: OhosKeyboardTarget(
                    controller: _manualCtl, label: '手动输入文本'),
                active: _kbdTarget != null,
                hint: '剪贴板不可读时的发送入口',
                onActivate: () => setState(() {
                  _kbdTarget = OhosKeyboardTarget(
                      controller: _manualCtl, label: '手动输入文本');
                }),
              ),
            const SizedBox(height: 10),
            FilledButton.tonalIcon(
              icon: const Icon(Icons.send),
              label: const Text('发送该文本'),
              onPressed: () async {
                final sent =
                    await store.sendText(target.deviceId, _manualCtl.text);
                if (sent) {
                  _manualCtl.clear();
                  if (context.mounted) Navigator.of(context).maybePop();
                }
              },
            ),
            if (!kOhosNativeKeyboard && _kbdTarget != null)
              Padding(
                padding: const EdgeInsets.only(top: 10),
                child: OhosKeyboardPanel(
                  target: _kbdTarget!,
                  onDone: () => setState(() => _kbdTarget = null),
                ),
              ),
          ],
          // 鸿蒙 channel 断流期間剪贴板/键盘都不可用，debug 构建给一个
          // 固定文本探针打通发送链验收；release 构建不含此入口。
          if (kDebugMode && _isOhos) ...[
            const SizedBox(height: 10),
            OutlinedButton.icon(
              icon: const Icon(Icons.send),
              label: const Text('发送测试文本（鸿蒙调试）'),
              onPressed: () async {
                final sent = await store.sendProbeText(target.deviceId);
                if (sent && context.mounted) Navigator.of(context).maybePop();
              },
            ),
          ],
          const SizedBox(height: 16),
          // 面板是 modal sheet，会盖住 ScaffoldMessenger 的 SnackBar——
          // 面板内的操作反馈必须就地展示，否则用户看到的就是「没反应」。
          if (store.toast != null) ...[
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
              decoration: BoxDecoration(
                color: theme.colorScheme.secondaryContainer,
                borderRadius: BorderRadius.circular(8),
              ),
              child: Text(store.toast!, style: theme.textTheme.bodySmall),
            ),
            const SizedBox(height: 10),
          ],
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
