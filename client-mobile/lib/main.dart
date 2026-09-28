/// UniClip（瞬贴）移动端入口。
///
/// 生命周期约定（V2 计划 §4.3）：
/// - 前台保持连接；回前台时调 reconnect 跳过退避立即重连；
/// - 切后台不主动断开（OS 会挂起 socket，resume 时重连）。
library;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import 'app_shell.dart';
import 'dbg.dart';
import 'state/app_store.dart';
import 'widgets/transfer_card.dart';

void main() {
  dbgLog('main: ohos=${isOhosRuntime()}');
  runApp(const UniClipApp());
}

class UniClipApp extends StatelessWidget {
  const UniClipApp({super.key});

  @override
  Widget build(BuildContext context) {
    return ChangeNotifierProvider(
      create: (_) => AppStore()..bootstrap(),
      child: MaterialApp(
        title: 'UniClip 瞬贴',
        // 点空白收起键盘：iOS 没有系统返回键，不处理这个手势的话
        // 键盘会一直盖住底部导航，用户被锁死在当前页（实测卡点）。
        // GestureDetector 只在没有子组件消费 tap 时才触发，不影响列表与控件。
        builder: (context, child) => GestureDetector(
          onTap: () => FocusManager.instance.primaryFocus?.unfocus(),
          child: child,
        ),
        theme:
            ThemeData(colorSchemeSeed: const Color(0xFF3F51B5), useMaterial3: true),
        darkTheme: ThemeData(
          colorSchemeSeed: const Color(0xFF3F51B5),
          brightness: Brightness.dark,
          useMaterial3: true,
        ),
        home: const _LifecycleHost(child: AppShell()),
      ),
    );
  }
}

/// 包一层生命周期：回前台触发快速重连；Ask 确认弹窗挂载在栈顶。
class _LifecycleHost extends StatefulWidget {
  const _LifecycleHost({required this.child});

  final Widget child;

  @override
  State<_LifecycleHost> createState() => _LifecycleHostState();
}

class _LifecycleHostState extends State<_LifecycleHost>
    with WidgetsBindingObserver {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    context.read<AppStore>().onToast = _showToast;
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) {
      context.read<AppStore>().resume();
    }
  }

  void _showToast(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).hideCurrentSnackBar();
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message), duration: const Duration(seconds: 4)),
    );
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    return Stack(
      textDirection: TextDirection.ltr,
      children: [
        widget.child,
        // Ask 档确认弹窗：全局模态（任何页签收到都弹）
        if (store.pendingConfirm != null)
          Positioned.fill(
            child: Material(
              color: Colors.black54,
              child: Center(
                child: Card(
                  margin: const EdgeInsets.all(32),
                  child: ConfirmReceiveSheet(
                    request: store.pendingConfirm!,
                    onRespond: (accept) => store
                        .respondOffer(store.pendingConfirm!.sessionId, accept),
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}
