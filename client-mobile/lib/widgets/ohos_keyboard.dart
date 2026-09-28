/// 鸿蒙内置 ASCII 键盘：fork 的 Dart→ArkTS platform channel 断流时
/// （TextInput.show 永不回包，系统键盘不弹），设置页等表单用纯 Flutter
/// 渲染的屏上键盘兜底。仅覆盖服务器/账号/密钥/指纹所需的 ASCII 字符集，
/// 支持末尾追加/退格/清空/完成；不支持光标中插与中文输入（channel 修复
/// 后应回到系统键盘）。
library;

import 'package:flutter/material.dart';

/// 当前编辑目标：控制器 + 显示属性（密文遮罩）。
class OhosKeyboardTarget {
  OhosKeyboardTarget({
    required this.controller,
    required this.label,
    this.obscure = false,
  });

  final TextEditingController controller;
  final String label;
  final bool obscure;
}

/// 表单字段的鸿蒙降级展示件：外观对齐 OutlineInputBorder 的 TextField，
/// 点击不请求系统键盘，而是把编辑权交给 [onActivate] 挂载的屏上键盘。
class OhosField extends StatelessWidget {
  const OhosField({
    super.key,
    required this.target,
    required this.active,
    required this.onActivate,
    this.hint,
    this.helper,
  });

  final OhosKeyboardTarget target;
  final bool active;
  final VoidCallback onActivate;
  final String? hint;
  final String? helper;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return InkWell(
      onTap: onActivate,
      borderRadius: BorderRadius.circular(4),
      // TextEditingController 是 ValueNotifier：屏上键盘改的是控制器，
      // 必须监听它重建，否则输入不回显（真机回归发现）。
      child: ValueListenableBuilder<TextEditingValue>(
        valueListenable: target.controller,
        builder: (context, value, _) {
          final text = value.text;
          final shown = target.obscure
              ? (text.isEmpty ? '' : '●' * text.length)
              : text;
          return InputDecorator(
            decoration: InputDecoration(
              labelText: target.label,
              hintText: hint,
              helperText: helper,
              border: const OutlineInputBorder(),
              focusedBorder: active
                  ? OutlineInputBorder(
                      borderSide:
                          BorderSide(color: theme.colorScheme.primary, width: 2))
                  : null,
            ),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    shown.isEmpty ? (hint ?? '') : shown,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: shown.isEmpty
                        ? theme.textTheme.bodyLarge
                            ?.copyWith(color: theme.hintColor)
                        : theme.textTheme.bodyLarge,
                  ),
                ),
                if (active)
                  Icon(Icons.edit,
                      size: 16, color: theme.colorScheme.primary),
              ],
            ),
          );
        },
      ),
    );
  }
}

/// 屏上 ASCII 键盘面板。布局贴近手机系统键盘的肌肉记忆：
/// 数字行 + 三行字母 + 符号/空格/完成。shift 仅切换大小写。
class OhosKeyboardPanel extends StatefulWidget {
  const OhosKeyboardPanel({super.key, required this.target, required this.onDone});

  final OhosKeyboardTarget target;
  final VoidCallback onDone;

  @override
  State<OhosKeyboardPanel> createState() => _OhosKeyboardPanelState();
}

class _OhosKeyboardPanelState extends State<OhosKeyboardPanel> {
  bool _shift = false;

  TextEditingController get _ctl => widget.target.controller;

  void _append(String ch) {
    _ctl.text = _ctl.text + ch;
    _ctl.selection = TextSelection.collapsed(offset: _ctl.text.length);
  }

  void _backspace() {
    if (_ctl.text.isEmpty) return;
    _ctl.text = _ctl.text.substring(0, _ctl.text.length - 1);
    _ctl.selection = TextSelection.collapsed(offset: _ctl.text.length);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final rows = <List<String>>[
      '1 2 3 4 5 6 7 8 9 0'.split(' '),
      'q w e r t y u i o p'.split(' '),
      'a s d f g h j k l'.split(' '),
      'z x c v b n m . / :'.split(' '),
    ];
    return Container(
      // surfaceVariant 是两端 Flutter 的交集：3.7.12 fork 无
      // surfaceContainerHighest，主线 3.44 已把 surfaceVariant 标记弃用。
      // ignore: deprecated_member_use
      color: theme.colorScheme.surfaceVariant,
      padding: const EdgeInsets.fromLTRB(4, 6, 4, 8),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
            padding: const EdgeInsets.only(bottom: 4),
            child: Text('正在编辑：${widget.target.label}',
                style: theme.textTheme.labelSmall),
          ),
          for (var i = 0; i < rows.length; i++)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Row(
                children: [
                  for (final k in rows[i])
                    Expanded(child: _key(_shift ? k.toUpperCase() : k)),
                  if (i == 2)
                    Expanded(
                      child: _action(Icons.backspace_outlined, () {
                        _backspace();
                        setState(() {});
                      }),
                    ),
                ],
              ),
            ),
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 2),
            child: Row(
              children: [
                Expanded(
                  child: _actionText('⇧', _shift, () {
                    setState(() => _shift = !_shift);
                  }),
                ),
                Expanded(child: _key('-')),
                Expanded(child: _key('_')),
                Expanded(child: _key('@')),
                Expanded(flex: 3, child: _actionText('空格', false, () {
                  _append(' ');
                  setState(() {});
                })),
                Expanded(child: _action(Icons.clear_all, () {
                  _ctl.clear();
                  setState(() {});
                })),
                Expanded(
                  flex: 2,
                  child: _actionText('完成', true, widget.onDone),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _key(String ch) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2),
      child: Material(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(6),
        child: InkWell(
          borderRadius: BorderRadius.circular(6),
          onTap: () {
            _append(ch);
            if (_shift) setState(() => _shift = false);
            setState(() {});
          },
          child: Container(
            height: 44,
            alignment: Alignment.center,
            child: Text(ch, style: const TextStyle(fontSize: 17)),
          ),
        ),
      ),
    );
  }

  Widget _action(IconData icon, VoidCallback onTap) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2),
      child: Material(
        // ignore: deprecated_member_use
        color: Theme.of(context).colorScheme.surfaceVariant,
        borderRadius: BorderRadius.circular(6),
        child: InkWell(
          borderRadius: BorderRadius.circular(6),
          onTap: onTap,
          child: SizedBox(height: 44, child: Center(child: Icon(icon, size: 20))),
        ),
      ),
    );
  }

  Widget _actionText(String text, bool filled, VoidCallback onTap) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2),
      child: Material(
        color: filled
            ? theme.colorScheme.primaryContainer
            // ignore: deprecated_member_use
            : theme.colorScheme.surfaceVariant,
        borderRadius: BorderRadius.circular(6),
        child: InkWell(
          borderRadius: BorderRadius.circular(6),
          onTap: onTap,
          child: SizedBox(
            height: 44,
            child: Center(child: Text(text)),
          ),
        ),
      ),
    );
  }
}
