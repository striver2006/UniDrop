/// 历史页：传输历史列表 + 每条记录的操作（分享 / 查看文件）。
/// expanded 断点：列表-详情双栏；窄屏：bottom sheet 详情。

library;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models.dart';
import '../state/app_store.dart';
import '../widgets/adaptive.dart';

class HistoryPage extends StatelessWidget {
  const HistoryPage({super.key});

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    final class_ = sizeClass(MediaQuery.sizeOf(context).width);

    final list = ListView.builder(
      padding: const EdgeInsets.symmetric(vertical: 8),
      itemCount: store.history.length,
      itemBuilder: (context, i) {
        final entry = store.history[i];
        return _HistoryTile(
          entry: entry,
          onTap: () {
            if (class_.useTwoPane) {
              context.read<AppStore>().historyFocus = entry.sessionId;
            } else {
              showModalBottomSheet(
                context: context,
                showDragHandle: true,
                isScrollControlled: true,
                builder: (_) => _HistoryDetail(sessionId: entry.sessionId),
              );
            }
          },
        );
      },
    );

    final body = class_.useTwoPane
        ? Row(children: [
            SizedBox(width: 400, child: list),
            const VerticalDivider(width: 0),
            Expanded(
              child: store.historyFocus == null
                  ? const Center(child: Text('选择一条记录查看详情'))
                  : _HistoryDetail(sessionId: store.historyFocus!),
            ),
          ])
        : list;

    return Scaffold(
      appBar: AppBar(
        title: const Text('历史'),
        actions: [
          IconButton(
            icon: const Icon(Icons.refresh),
            tooltip: '刷新',
            onPressed: () => store.refreshHistory(),
          ),
        ],
      ),
      body: store.history.isEmpty
          ? const Center(child: Text('暂无传输历史'))
          : body,
    );
  }
}

class _HistoryTile extends StatelessWidget {
  const _HistoryTile({required this.entry, required this.onTap});

  final HistoryEntry entry;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = switch (entry.status) {
      'COMPLETED' => Colors.green,
      'FAILED' => theme.colorScheme.error,
      _ => theme.colorScheme.primary,
    };

    return ListTile(
      leading: Icon(entry.isReceive ? Icons.download : Icons.upload,
          color: color),
      title: Text(
        entry.previewSummary ?? '（加密内容）',
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: Text(
        [
          entry.isReceive ? '接收' : '发送',
          humanBytes(entry.totalSize),
          if (entry.totalItems > 1) '${entry.totalItems} 项',
          _briefTime(entry),
        ].join(' · '),
      ),
      trailing: entry.status == 'FAILED'
          ? const Icon(Icons.error_outline, color: Colors.red)
          : entry.hasCachedFiles
              ? const Icon(Icons.chevron_right)
              : null,
      onTap: onTap,
    );
  }

  String _briefTime(HistoryEntry e) {
    final raw = e.createdAt ?? e.completedAt;
    if (raw == null) return '';
    return raw.length >= 16 ? raw.substring(0, 16) : raw;
  }
}

class _HistoryDetail extends StatelessWidget {
  const _HistoryDetail({required this.sessionId});

  final String sessionId;

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    final entry = store.history
        .where((e) => e.sessionId == sessionId)
        .firstOrNull;
    if (entry == null) {
      return const Center(child: Text('记录不存在（可能已被清理）'));
    }
    final theme = Theme.of(context);
    final store2 = context.read<AppStore>();

    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(20),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(entry.previewSummary ?? '（加密内容）',
                style: theme.textTheme.titleMedium),
            const SizedBox(height: 8),
            Text(
              '${entry.isReceive ? '接收' : '发送'} · ${humanBytes(entry.totalSize)} · ${entry.totalItems} 项\n'
              '状态：${entry.status}${entry.errorMessage != null ? '\n失败原因：${entry.errorMessage}' : ''}\n'
              '本地缓存文件：${entry.cachedCount} 个',
              style: theme.textTheme.bodyMedium,
            ),
            const SizedBox(height: 20),
            if (entry.hasCachedFiles) ...[
              FilledButton.icon(
                icon: const Icon(Icons.share),
                label: const Text('分享 / 保存到…'),
                // 先收起 bottom sheet 再拉系统分享面板：iOS 上 rootViewController
                // 已被 sheet 占用时，再 present UIActivityViewController 会被
                // 系统拒绝（"already presenting"），异常若被吞即「点了没反应」。
                onPressed: () => _shareAfterSheetDismissed(context, store, entry),
              ),
              const SizedBox(height: 8),
              if (entry.dataType == 'TEXT')
                OutlinedButton.icon(
                  icon: const Icon(Icons.content_copy),
                  label: const Text('复制文本'),
                  onPressed: () async {
                    final text = await store2.readSessionText(entry.sessionId);
                    if (text != null) {
                      await store2.platform.writeClipboardText(text);
                      store.notifyUser('已复制到剪贴板');
                    } else {
                      store.notifyUser('文本读取失败（缓存文件可能已被清理）');
                    }
                  },
                ),
            ] else
              Text(
                '缓存文件已被清理（超出保留策略）',
                style: theme.textTheme.bodySmall
                    ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
              ),
          ],
        ),
      ),
    );
  }
}

/// 收起详情 sheet 后再执行分享，失败一律可见。
/// 手机端详情是 modal bottom sheet；Pad 双栏详情没有 sheet，pop 是 no-op
/// 的风险由调用侧保证（双栏不经过此路径时 context 无 Navigator 可 pop
/// 会抛错——用 maybePop 兜底）。
Future<void> _shareAfterSheetDismissed(
    BuildContext context, AppStore store, HistoryEntry entry) async {
  Navigator.of(context).maybePop();
  // 等 sheet 完全收起（iOS 上立刻 present 仍可能撞上过渡动画）
  await Future<void>.delayed(const Duration(milliseconds: 350));

  final List<String> files;
  try {
    files = await store.sessionFiles(entry.sessionId);
  } catch (e) {
    store.notifyUser('读取会话文件失败：$e');
    return;
  }
  if (files.isEmpty) {
    store.notifyUser('缓存文件已被清理，无法分享');
    return;
  }
  try {
    await store.platform.shareFiles(files, subject: entry.previewSummary);
  } catch (e) {
    store.notifyUser('分享失败：$e');
  }
}
