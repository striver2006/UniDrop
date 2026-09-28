#!/bin/zsh
# 主 pubspec 变体 ↔ 鸿蒙变体切换（pubspec.ohos.yaml 头注释引用的入口）。
#
# 用法：
#   scripts/use_ohos_deps.sh ohos   # 切到鸿蒙变体（华为 Flutter 3.7.12/Dart 2.19 分支用）
#   scripts/use_ohos_deps.sh main   # 切回主变体（官方 Flutter 3.44+ / CI 用）
#
# 约定：pubspec.yaml / pubspec.lock 是 git 跟踪的主变体；鸿蒙变体文件为
# pubspec.ohos.yaml（已入库）与 pubspec.ohos.lock（gitignore，首次 pub get 生成）。
# 切换时主变体文件备份为 *.main.bak，切回时恢复；鸿蒙 lock 保存回 pubspec.ohos.lock。
set -euo pipefail
cd "$(dirname "$0")/.."

direction="${1:-}"
if [[ "$direction" != "ohos" && "$direction" != "main" ]]; then
  echo "用法：scripts/use_ohos_deps.sh [ohos|main]" >&2
  exit 1
fi

is_ohos_active() { grep -q '鸿蒙构建专用依赖变体' pubspec.yaml; }

case "$direction" in
  ohos)
    if is_ohos_active; then
      echo "==> 已是鸿蒙变体，无需切换"
      exit 0
    fi
    cp pubspec.yaml pubspec.main.yaml.bak
    [[ -f pubspec.lock ]] && cp pubspec.lock pubspec.main.lock.bak
    cp pubspec.ohos.yaml pubspec.yaml
    if [[ -f pubspec.ohos.lock ]]; then
      cp pubspec.ohos.lock pubspec.lock
      echo "==> 已切到鸿蒙变体（复用 pubspec.ohos.lock）"
    else
      rm -f pubspec.lock
      echo "==> 已切到鸿蒙变体（无 pubspec.ohos.lock，需 pub get 全新解析）"
    fi
    cat <<'EOF'
后续步骤（用华为 Flutter 分支，不要用官方 flutter）：
  export PATH="/Users/chenzhenbo/DevLib/Flutter-Ohos/bin:$PATH"
  flutter pub get
  ./scripts/prepare_ohos_plugin_wrappers.sh   # pub-cache 重新 resolve 后必须重跑
  ./scripts/standardize_ohos_plugins.sh
注意：3.7.12 工具对纯 Dart 测试也做 android embedding 检查（本工程为误报），
跑测试要加 --no-pub：flutter test --no-pub
切回主变体：scripts/use_ohos_deps.sh main
EOF
    ;;
  main)
    if ! is_ohos_active; then
      echo "==> 当前不是鸿蒙变体，无需切回"
      exit 0
    fi
    [[ -f pubspec.lock ]] && cp pubspec.lock pubspec.ohos.lock
    if [[ -f pubspec.main.yaml.bak ]]; then
      mv pubspec.main.yaml.bak pubspec.yaml
      [[ -f pubspec.main.lock.bak ]] && mv pubspec.main.lock.bak pubspec.lock
      echo "==> 已切回主变体（备份恢复，鸿蒙 lock 存为 pubspec.ohos.lock）"
    else
      # 备份缺失（如手工改过）：从 git 恢复已入库的主变体
      git checkout -- pubspec.yaml pubspec.lock
      echo "==> 备份缺失，已从 git 恢复主变体（鸿蒙 lock 存为 pubspec.ohos.lock）"
    fi
    echo "切回后用官方 Flutter 重新解析：flutter pub get"
    ;;
esac
