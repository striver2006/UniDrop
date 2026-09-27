#!/bin/zsh
# 为 git 依赖的 ohos 插件补 hvigor wrapper（hvigorw + hvigor/）。
# openharmony-sig 新版插件的 ohos 目录不带 wrapper，华为 3.7 工具的
# getHvigorwPath 直接拼 <插件ohos>/hvigorw，缺文件即崩。
# pub-cache 重新 resolve 后需重跑本脚本（build_native_ohos.sh 会自动调用）。
set -euo pipefail
cd "$(dirname "$0")/.."

WRAPPER_SRC="ohos"
count=0
for plugin_ohos in ~/.pub-cache/git/flutter_packages-*/packages/*/*_ohos/ohos \
                   ~/.pub-cache/git/flutter_plus_plugins-*/packages/*/*/ohos; do
  [[ -d "$plugin_ohos" ]] || continue
  if [[ ! -f "$plugin_ohos/hvigorw" ]]; then
    cp "$WRAPPER_SRC/hvigorw" "$plugin_ohos/hvigorw"
    cp "$WRAPPER_SRC/hvigorw.bat" "$plugin_ohos/" 2>/dev/null || true
    mkdir -p "$plugin_ohos/hvigor"
    cp "$WRAPPER_SRC/hvigor/"* "$plugin_ohos/hvigor/"
    count=$((count+1))
    echo "wrapper → $plugin_ohos"
  fi
done
echo "已补 $count 个插件的 hvigor wrapper"
