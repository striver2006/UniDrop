/// 设置页：服务器 / 账号 / 密钥 / 接收策略 / E2EE / TLS 信任 / 限额只读展示。
///
/// 表单语义：所有可编辑字段（含档位类开关）只改**本地编辑态**，
/// 由 AppBar 右上角的「保存」统一提交——AppSettings 是整份落库的
/// read-modify-write 模型，散点即时保存会把用户改到一半的字段一起
/// 带出去；此前「切 pinned 档立即保存」更是死锁：空指纹被 core 拒绝，
/// 设置回不去，指纹输入框（条件渲染）永远不出现。

library;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../dbg.dart';
import '../models.dart';
import '../state/app_store.dart';
import '../widgets/ohos_keyboard.dart';

class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  late TextEditingController _server;
  late TextEditingController _account;
  late TextEditingController _psk;
  late TextEditingController _pin;
  late TextEditingController _historyMax;
  late TextEditingController _retainSecs;
  late TextEditingController _cacheTtl;
  late TextEditingController _cacheMaxMb;

  /// 本地编辑态：初始从持久化设置取，保存前不落库。
  late String _trustMode;
  late String _policy;
  late bool _e2ee;
  bool _autoInject = false;

  bool _saving = false;

  /// 鸿蒙屏上键盘的当前编辑目标（断流回退路径用）。非空时页面底部挂
  /// OhosKeyboardPanel。
  OhosKeyboardTarget? _kbdTarget;

  final bool _isOhos = isOhosRuntime();

  /// 表单是否已从 store 回填过。
  ///
  /// IndexedStack 常驻的页面在 App 启动瞬间就 initState，而 store.settings
  /// 要等核心启动 + get_settings 返回后才就绪——只靠 initState 取值，
  /// 拿到的是加载完成前的空值，表现为「保存后回显不对、重开配置被清空」。
  /// settings 就绪后补一次水；此后不再自动覆盖（用户编辑优先）。
  bool _hydrated = false;

  @override
  void initState() {
    super.initState();
    _server = TextEditingController();
    _account = TextEditingController();
    _psk = TextEditingController();
    _pin = TextEditingController();
    _historyMax = TextEditingController();
    _retainSecs = TextEditingController();
    _cacheTtl = TextEditingController();
    _cacheMaxMb = TextEditingController();
    _trustMode = 'public_ca';
    _policy = 'always';
    _e2ee = true;
    _autoInject = false;
    final s = context.read<AppStore>().settings;
    if (s != null) _hydrate(s);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final s = context.read<AppStore>().settings;
    if (s != null && !_hydrated) _hydrate(s);
  }

  void _hydrate(AppSettingsDto s) {
    _hydrated = true;
    _server.text = s.serverUrl;
    _account.text = s.accountId;
    _psk.text = s.pskSecret;
    _pin.text = s.pinnedCertSha256.join('\n');
    _historyMax.text = s.historyMaxEntries.toString();
    _retainSecs.text = s.transferCardRetainSecs.toString();
    _cacheTtl.text = s.cacheTtlHours.toString();
    _cacheMaxMb.text = s.cacheMaxSizeMb.toString();
    _trustMode = s.tlsTrustMode;
    _policy = s.receivePolicy;
    _e2ee = s.e2eeEnabled;
    _autoInject = s.autoInject;
  }

  @override
  void dispose() {
    _server.dispose();
    _account.dispose();
    _psk.dispose();
    _pin.dispose();
    _historyMax.dispose();
    _retainSecs.dispose();
    _cacheTtl.dispose();
    _cacheMaxMb.dispose();
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
      appBar: AppBar(
        title: const Text('设置'),
        // 保存入口放在 AppBar：键盘弹出时表单底部的按钮会被顶出视口，
        // 用户「填完没处点」正是上一版丢配置的原因。
        actions: [
          TextButton(
            onPressed: _saving ? null : () => _save(store, s),
            child: _saving
                ? const SizedBox(
                    width: 18,
                    height: 18,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                : const Text('保存'),
          ),
        ],
      ),
      body: Column(
        children: [
          Expanded(
            child: ListView(
        keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
        padding: const EdgeInsets.all(16),
        children: [
          const _Section(title: '服务器'),
          _field(_server, label: '服务器地址', hint: 'wss://drop.yourdomain.com:58921'),
          const SizedBox(height: 12),
          _field(_account, label: '账号标识', helper: '1-64 位字母、数字与 . _ @ -'),
          const SizedBox(height: 12),
          _field(_psk, label: '共享密钥（PSK）', obscure: true),
          const SizedBox(height: 8),
          FilledButton.tonal(
            onPressed: _saving ? null : () => _save(store, s),
            child: const Text('保存并重连'),
          ),
          const SizedBox(height: 16),
          const _Section(title: '接收策略'),
          Wrap(
            spacing: 8,
            children: [
              _policyChip('always', '全部自动'),
              _policyChip('wifi_only', '仅 Wi-Fi'),
              _policyChip('ask', '每次询问'),
            ],
          ),
          const SizedBox(height: 8),
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            title: const Text('接收文本/图片后自动复制'),
            subtitle: const Text('免去手动装载，收到即可直接粘贴'),
            value: _autoInject,
            onChanged: (v) => setState(() => _autoInject = v),
          ),
          const SizedBox(height: 24),
          const _Section(title: '存储与清理'),
          _field(_historyMax, label: '历史保留条数（0 = 不限）', number: true),
          const SizedBox(height: 12),
          _field(_retainSecs, label: '完成卡片保持秒数（0 = 不自动消失）', number: true),
          const SizedBox(height: 12),
          _field(_cacheTtl, label: '收件保留小时数（0 = 不按时间清理）', number: true),
          const SizedBox(height: 12),
          _field(_cacheMaxMb, label: '收件容量上限 MB（0 = 不限容量）', number: true),
          const SizedBox(height: 8),
          Text(
            '手机存储有限，建议容量 2048 MB 起步；收件目录 iOS 在「文件」App 的'
            '瞬贴目录下可见。',
            style: theme.textTheme.bodySmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
          const SizedBox(height: 24),
          const _Section(title: '安全'),
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            title: const Text('端到端加密（E2EE）'),
            subtitle: const Text('对端不支持时自动回落明文并提示'),
            value: _e2ee,
            onChanged: (v) => setState(() => _e2ee = v),
          ),
          const SizedBox(height: 8),
          DropdownButtonFormField<String>(
            value: _trustMode, // ignore: deprecated_member_use
            decoration: const InputDecoration(
              labelText: 'TLS 信任策略',
              border: OutlineInputBorder(),
            ),
            items: const [
              DropdownMenuItem(value: 'public_ca', child: Text('信任公共 CA（默认）')),
              DropdownMenuItem(value: 'pinned', child: Text('仅信任指定证书指纹')),
              DropdownMenuItem(value: 'insecure', child: Text('跳过证书校验（不安全）')),
            ],
            // 只改本地态：选 pinned 立即出指纹输入框，校验留给「保存」。
            onChanged: (v) {
              if (v != null) setState(() => _trustMode = v);
            },
          ),
          // Pinned 档必填：空指纹会被 core 拒绝（等于没有任何信任来源）。
          // 一行一条，支持直接粘 openssl 输出（core 侧解析时容错冒号与前后缀）。
          if (_trustMode == 'pinned') ...[
            const SizedBox(height: 12),
            _field(_pin, label: '证书 SHA-256 指纹（每行一条）', hint: '68e1d200…'),
          ],
          const SizedBox(height: 24),
          const _Section(title: '服务端限额（只读）'),
          _LimitsCard(limits: store.serverLimits),
          const SizedBox(height: 24),
          const _Section(title: '本机'),
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
          ),
          if (_kbdTarget != null)
            OhosKeyboardPanel(
              target: _kbdTarget!,
              onDone: () => setState(() => _kbdTarget = null),
            ),
        ],
      ),
    );
  }

  /// 鸿蒙 channel 断流期间系统键盘不可用（TextInput.show 永不回包），
  /// 表单字段降级为屏上键盘录入；其他平台保持原生 TextField。
  Widget _field(
    TextEditingController ctl, {
    required String label,
    String? hint,
    String? helper,
    bool obscure = false,
    bool number = false,
  }) {
    // channel 复活验证期（1.0.4 引擎）：鸿蒙也走原生 TextField——
    // 系统键盘弹出即证明 TextInput 通道已通，屏上键盘仅作断流回退。
    if (!_isOhos || kOhosNativeKeyboard) {
      return TextField(
        controller: ctl,
        obscureText: obscure,
        autocorrect: false,
        enableSuggestions: false,
        keyboardType: number ? TextInputType.number : null,
        decoration: InputDecoration(
          labelText: label,
          hintText: hint,
          helperText: helper,
          border: const OutlineInputBorder(),
        ),
      );
    }
    final target =
        OhosKeyboardTarget(controller: ctl, label: label, obscure: obscure);
    return OhosField(
      target: target,
      active: _kbdTarget?.controller == ctl,
      hint: hint,
      helper: helper,
      onActivate: () => setState(() => _kbdTarget = target),
    );
  }

  Widget _policyChip(String value, String label) {
    return ChoiceChip(
      label: Text(label),
      selected: _policy == value,
      onSelected: (_) => setState(() => _policy = value),
    );
  }

  Future<void> _save(AppStore store, AppSettingsDto current) async {
    final pins = _pin.text
        .split(RegExp(r'[\n,;]'))
        .map((l) => l.trim())
        .where((l) => l.isNotEmpty)
        .toList();

    if (_trustMode == 'pinned' && pins.isEmpty) {
      _toast('请填写证书指纹：Pinned 档没有指纹等于不信任任何证书');
      return;
    }

    int parseNum(TextEditingController c, int fallback) =>
        int.tryParse(c.text.trim()) ?? fallback;
    final next = AppSettingsDto(
      serverUrl: _server.text.trim(),
      accountId: _account.text.trim(),
      pskSecret: _psk.text,
      autoInject: _autoInject,
      receivePolicy: _policy,
      e2eeEnabled: _e2ee,
      tlsTrustMode: _trustMode,
      pinnedCertSha256: pins,
      historyMaxEntries: parseNum(_historyMax, current.historyMaxEntries),
      transferCardRetainSecs: parseNum(_retainSecs, current.transferCardRetainSecs),
      cacheTtlHours: parseNum(_cacheTtl, current.cacheTtlHours),
      cacheMaxSizeMb: parseNum(_cacheMaxMb, current.cacheMaxSizeMb),
    );

    setState(() => _saving = true);
    try {
      await store.saveSettings(next);
      _toast('已保存，正在重连');
    } catch (e) {
      _toast('保存失败：$e');
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  void _toast(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message)));
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
