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
                onPressed: () async {
                  final files = await store2.sessionFiles(entry.sessionId);
                  if (context.mounted && files.isNotEmpty) {
                    await store2.platform.shareFiles(files,
                        subject: entry.previewSummary);
                  }
                },
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
                      if (context.mounted) {
                        ScaffoldMessenger.of(context).showSnackBar(
                          const SnackBar(content: Text('已复制到剪贴板')),
                        );
                      }
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
