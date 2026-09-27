/// 传输页：进行中与刚结束的传输卡片（对应桌面端 TransferProgress 列表）。

library;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../state/app_store.dart';
import '../widgets/transfer_card.dart';

class TransfersPage extends StatelessWidget {
  const TransfersPage({super.key});

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AppStore>();
    final transfers = store.transfers.values.toList()
      ..sort((a, b) => b.sessionId.compareTo(a.sessionId));

    return Scaffold(
      appBar: AppBar(
        title: const Text('传输'),
        actions: [
          IconButton(
            tooltip: '刷新',
            icon: const Icon(Icons.refresh),
            onPressed: () => store.refreshHistory(),
          ),
        ],
      ),
      body: transfers.isEmpty
          ? Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.swap_vert,
                      size: 56,
                      color: Theme.of(context).colorScheme.onSurfaceVariant),
                  const SizedBox(height: 12),
                  const Text('暂无进行中的传输'),
                  const SizedBox(height: 4),
                  Text(
                    '在「设备」页选择一台设备发送；接收自动出现在这里。',
                    style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: Theme.of(context).colorScheme.onSurfaceVariant),
                  ),
                ],
              ),
            )
          : ListView.builder(
              padding: const EdgeInsets.symmetric(vertical: 8),
              itemCount: transfers.length,
              itemBuilder: (context, i) =>
                  TransferCard(transfer: transfers[i]),
            ),
    );
  }
}
