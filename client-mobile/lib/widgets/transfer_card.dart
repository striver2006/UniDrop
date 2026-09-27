import 'package:flutter/material.dart';

import '../models.dart';

/// 传输卡片：进度条 + 方向/状态 + 摘要。
/// 与桌面端 TransferProgress 卡片同一信息结构。
class TransferCard extends StatelessWidget {
  const TransferCard({super.key, required this.transfer});

  final ActiveTransfer transfer;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final t = transfer;
    final Color color;
    final String statusLabel;
    switch (t.status) {
      case 'COMPLETED':
        color = Colors.green;
        statusLabel = '已完成';
        break;
      case 'FAILED':
        color = theme.colorScheme.error;
        statusLabel = '失败';
        break;
      default:
        color = theme.colorScheme.primary;
        statusLabel = t.isReceive ? '接收中' : '发送中';
    }

    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(t.isReceive ? Icons.download : Icons.upload,
                    size: 20, color: color),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    t.previewSummary.isEmpty ? '传输' : t.previewSummary,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.bodyMedium,
                  ),
                ),
                Text(statusLabel,
                    style: theme.textTheme.labelMedium?.copyWith(color: color)),
              ],
            ),
            const SizedBox(height: 10),
            LinearProgressIndicator(
              value: t.status == 'COMPLETED'
                  ? 1
                  : (t.progress / 100).clamp(0.0, 1.0),
              minHeight: 6,
            ),
            const SizedBox(height: 6),
            Text(
              '${humanBytes(t.transferredSize)} / ${humanBytes(t.totalSize)}',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Ask 档的接收确认弹窗。
class ConfirmReceiveSheet extends StatelessWidget {
  const ConfirmReceiveSheet({super.key, required this.request, required this.onRespond});

  final ConfirmRequest request;
  final Future<void> Function(bool accept) onRespond;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.all(20),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text('接收新传输？', style: theme.textTheme.titleMedium),
          const SizedBox(height: 12),
          Text(
            request.previewSummary.isEmpty ? '（加密内容）' : request.previewSummary,
            style: theme.textTheme.bodyLarge,
          ),
          const SizedBox(height: 4),
          Text(
            '${humanBytes(request.totalSize)} · ${request.totalItems} 项 · 来自 ${request.fromDevice}',
            style: theme.textTheme.bodySmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
          const SizedBox(height: 20),
          FilledButton(
            onPressed: () => onRespond(true),
            child: const Text('接收'),
          ),
          const SizedBox(height: 8),
          OutlinedButton(
            onPressed: () => onRespond(false),
            child: const Text('拒绝'),
          ),
          const SizedBox(height: 8),
          Text(
            '${request.timeoutSecs} 秒内未应答将自动拒绝',
            textAlign: TextAlign.center,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
        ],
      ),
    );
  }
}
