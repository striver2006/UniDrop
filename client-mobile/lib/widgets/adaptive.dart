/// 响应式断点（Material 3 window size classes）与 Pad 双栏支撑。
///
/// - compact（<600）：手机竖屏——底部 NavigationBar + 单栏。
/// - medium（600–840）：Pad 竖屏 / 手机横屏——NavigationRail + 双栏（若可用）。
/// - expanded（≥840）：Pad 横屏——NavigationRail + 列表-详情双栏。

library;

enum WindowSizeClass { compact, medium, expanded }

WindowSizeClass sizeClass(double width) {
  if (width >= 840) return WindowSizeClass.expanded;
  if (width >= 600) return WindowSizeClass.medium;
  return WindowSizeClass.compact;
}

extension WindowSizeClassX on WindowSizeClass {
  bool get isCompact => this == WindowSizeClass.compact;
  bool get useRail => this != WindowSizeClass.compact;
  bool get useTwoPane => this == WindowSizeClass.expanded;
}
