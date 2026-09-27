/// 设置页：服务器 / 账号 / 密钥 / 接收策略 / E2EE / TLS 信任 / 限额只读展示。

library;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models.dart';
import '../state/app_store.dart';

class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  late TextEditingController _server;
  late TextEditingController _account;
  late TextEditingController _psk;

  @override
  void initState() {
    super.initState();
    final s = context.read<AppStore>().settings;
    _server = TextEditingController(text: s?.serverUrl ?? '');
    _account = TextEditingController(text: s?.accountId ?? '');
    _psk = TextEditingController(text: s?.pskSecret ?? '');
  }

  @override
  void dispose() {
    _server.dispose();
    _account.dispose();
    _psk.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    final s = store.settings;
    final theme = Theme.of(context);

    if (s == null) {
      return Scaffold(
        appBar: AppBar(title: const Text('设置')),
        body: const Center(child: CircularProgressIndicator()),
      );
    }

    return Scaffold(
      appBar: AppBar(title: const Text('设置')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          _Section(title: '服务器'),
          TextField(
            controller: _server,
            decoration: const InputDecoration(
              labelText: '服务器地址',
              hintText: 'wss://drop.yourdomain.com:58921',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _account,
            decoration: const InputDecoration(
              labelText: '账号标识',
              helperText: '1-64 位字母、数字与 . _ @ -',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _psk,
            obscureText: true,
            decoration: const InputDecoration(
              labelText: '共享密钥（PSK）',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 24),
          _Section(title: '接收策略'),
          SegmentedButton<String>(
            segments: const [
              ButtonSegment(value: 'always', label: Text('全部自动')),
              ButtonSegment(value: 'wifi_only', label: Text('仅 Wi-Fi')),
              ButtonSegment(value: 'ask', label: Text('每次询问')),
            ],
            selected: {s.receivePolicy},
            onSelectionChanged: (v) => _saveWith(store, s, receivePolicy: v.first),
          ),
          const SizedBox(height: 8),
          Text(
            '「仅 Wi-Fi」：蜂窝网络下大文件转确认，文本与小图片仍自动接收。',
            style: theme.textTheme.bodySmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
          const SizedBox(height: 24),
          _Section(title: '安全'),
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            title: const Text('端到端加密（E2EE）'),
            subtitle: const Text('对端不支持时自动回落明文并提示'),
            value: s.e2eeEnabled,
            onChanged: (v) => _saveWith(store, s, e2eeEnabled: v),
          ),
          const SizedBox(height: 8),
          DropdownButtonFormField<String>(
            initialValue: s.tlsTrustMode,
            decoration: const InputDecoration(
              labelText: 'TLS 信任策略',
              border: OutlineInputBorder(),
            ),
            items: const [
              DropdownMenuItem(value: 'public_ca', child: Text('信任公共 CA（默认）')),
              DropdownMenuItem(value: 'pinned', child: Text('仅信任指定证书指纹')),
              DropdownMenuItem(value: 'insecure', child: Text('跳过证书校验（不安全）')),
            ],
            onChanged: (v) {
              if (v != null) _saveWith(store, s, tlsTrustMode: v);
            },
          ),
          const SizedBox(height: 24),
          _Section(title: '服务端限额（只读）'),
          _LimitsCard(limits: store.serverLimits),
          const SizedBox(height: 24),
          _Section(title: '本机'),
          ListTile(
            contentPadding: EdgeInsets.zero,
            leading: const CircleAvatar(child: Icon(Icons.smartphone)),
            title: Text(store.selfInfo?.hostname ?? '—'),
            subtitle: Text(
              '${store.selfInfo?.platformLabel ?? ''} · v${store.selfInfo?.appVersion ?? ''}\n'
              'ID：${store.selfInfo?.deviceId ?? ''}',
            ),
          ),
          const SizedBox(height: 32),
        ],
      ),
    );
  }

  void _saveWith(
    AppStore store,
    AppSettingsDto current, {
    String? receivePolicy,
    bool? e2eeEnabled,
    String? tlsTrustMode,
  }) {
    final next = AppSettingsDto(
      serverUrl: _server.text.trim(),
      accountId: _account.text.trim(),
      pskSecret: _psk.text,
      autoInject: current.autoInject,
      receivePolicy: receivePolicy ?? current.receivePolicy,
      e2eeEnabled: e2eeEnabled ?? current.e2eeEnabled,
      tlsTrustMode: tlsTrustMode ?? current.tlsTrustMode,
      pinnedCertSha256: current.pinnedCertSha256,
      historyMaxEntries: current.historyMaxEntries,
      cacheMaxSizeMb: current.cacheMaxSizeMb,
    );
    store.saveSettings(next).then((_) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(const SnackBar(content: Text('已保存并重连')));
      }
    }).catchError((Object e) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text('保存失败：$e')));
      }
    });
  }
}

class _Section extends StatelessWidget {
  const _Section({required this.title});

  final String title;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(bottom: 10),
        child: Text(
          title,
          style: Theme.of(context)
              .textTheme
              .titleSmall
              ?.copyWith(color: Theme.of(context).colorScheme.primary),
        ),
      );
}

class _LimitsCard extends StatelessWidget {
  const _LimitsCard({required this.limits});

  final ServerLimits? limits;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (limits == null) {
      return Card(
        child: Padding(
          padding: const EdgeInsets.all(14),
          child: Text(
            '未下发（未连接，或服务端为旧版本）',
            style: theme.textTheme.bodyMedium
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
        ),
      );
    }
    final l = limits!;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _row('单个文件上限', humanBytes(l.maxSingleFileBytes)),
            _row('单次总量上限', humanBytes(l.maxTotalTransferBytes)),
            _row('文本上限', humanBytes(l.maxClipboardTextBytes)),
            _row('图片上限', humanBytes(l.maxClipboardImageBytes)),
            _row('单次条目数', l.maxItemsPerOffer == 0 ? '不限' : '${l.maxItemsPerOffer}'),
          ],
        ),
      ),
    );
  }

  Widget _row(String k, String v) {
    return Builder(builder: (context) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 3),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [Text(k), Text(v, style: Theme.of(context).textTheme.bodyMedium)],
        ),
      );
    });
  }
}
