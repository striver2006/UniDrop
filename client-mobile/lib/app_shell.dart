/// 应用壳：compact 用底部 NavigationBar，medium/expanded 用 NavigationRail。
/// 四个页签：传输 / 设备 / 历史 / 设置。

library;

import 'package:flutter/material.dart';

import 'pages/devices_page.dart';
import 'pages/history_page.dart';
import 'pages/settings_page.dart';
import 'pages/transfers_page.dart';
import 'widgets/adaptive.dart';

class AppShell extends StatelessWidget {
  const AppShell({super.key});

  static const pages = [
    _ShellEntry(
      label: '传输',
      icon: Icon(Icons.swap_vert_outlined),
      selectedIcon: Icon(Icons.swap_vert),
      page: TransfersPage(),
    ),
    _ShellEntry(
      label: '设备',
      icon: Icon(Icons.devices_outlined),
      selectedIcon: Icon(Icons.devices),
      page: DevicesPage(),
    ),
    _ShellEntry(
      label: '历史',
      icon: Icon(Icons.history_outlined),
      selectedIcon: Icon(Icons.history),
      page: HistoryPage(),
    ),
    _ShellEntry(
      label: '设置',
      icon: Icon(Icons.settings_outlined),
      selectedIcon: Icon(Icons.settings),
      page: SettingsPage(),
    ),
  ];

  @override
  Widget build(BuildContext context) {
    final class_ = sizeClass(MediaQuery.sizeOf(context).width);
    if (class_.useRail) {
      return const _RailShell();
    }
    return const _BarShell();
  }
}

class _ShellEntry {
  const _ShellEntry({
    required this.label,
    required this.icon,
    required this.selectedIcon,
    required this.page,
  });

  final String label;
  final Icon icon;
  final Icon selectedIcon;
  final Widget page;
}

class _BarShell extends StatefulWidget {
  const _BarShell();

  @override
  State<_BarShell> createState() => _BarShellState();
}

class _BarShellState extends State<_BarShell> {
  int index = 0;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: IndexedStack(
          index: index, children: AppShell.pages.map((e) => e.page).toList()),
      bottomNavigationBar: NavigationBar(
        selectedIndex: index,
        onDestinationSelected: (i) => setState(() => index = i),
        destinations: [
          for (final e in AppShell.pages)
            NavigationDestination(
              icon: e.icon,
              selectedIcon: e.selectedIcon,
              label: e.label,
            ),
        ],
      ),
    );
  }
}

class _RailShell extends StatefulWidget {
  const _RailShell();

  @override
  State<_RailShell> createState() => _RailShellState();
}

class _RailShellState extends State<_RailShell> {
  int index = 0;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Row(
        children: [
          NavigationRail(
            selectedIndex: index,
            onDestinationSelected: (i) => setState(() => index = i),
            labelType: NavigationRailLabelType.all,
            destinations: [
              for (final e in AppShell.pages)
                NavigationRailDestination(
                  icon: e.icon,
                  selectedIcon: e.selectedIcon,
                  label: Text(e.label),
                ),
            ],
          ),
          const VerticalDivider(width: 0),
          Expanded(
            child: IndexedStack(
              index: index,
              children: AppShell.pages.map((e) => e.page).toList(),
            ),
          ),
        ],
      ),
    );
  }
}
