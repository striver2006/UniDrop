#!/bin/zsh
# 把 openharmony-sig fork 插件的扁平 ohos/ 布局重排为标准 entry 结构。
# 扁平布局（ohos/src/main 直挂）华为 3.7 工具 patch 后能吃，但 hvigor 6
# 要求工程级 build-profile 带 modules 数组——标准结构两者通吃：
# 3.7 工具的模块扫描（已 patch：扫 <ohos>/<name>/src/main/module.json5）
# 会找到 entry/，hvigor 6 拿到合法的 modules 声明。
# pub-cache 重新 resolve 后需重跑（build 前手动或接入流水线）。
set -euo pipefail

ROOT_OHOS="$(cd "$(dirname "$0")/.." && pwd)/ohos"

for plugin_ohos in ~/.pub-cache/git/flutter_packages-*/packages/*/*_ohos/ohos \
                   ~/.pub-cache/git/flutter_plus_plugins-*/packages/*/*/ohos; do
  [[ -d "$plugin_ohos" ]] || continue
  # 只处理扁平布局（src/main 直挂且无 entry/）
  [[ -f "$plugin_ohos/src/main/module.json5" && ! -d "$plugin_ohos/entry" ]] || continue

  # wrapper / package.json（若缺则从工程补）
  if [[ ! -f "$plugin_ohos/hvigorw" ]]; then
    cp "$ROOT_OHOS/hvigorw" "$plugin_ohos/hvigorw"
    cp "$ROOT_OHOS/hvigorw.bat" "$plugin_ohos/" 2>/dev/null || true
    mkdir -p "$plugin_ohos/hvigor"
    cp "$ROOT_OHOS/hvigor/"* "$plugin_ohos/hvigor/"
  fi

  mkdir -p "$plugin_ohos/entry"
  mv "$plugin_ohos/src" "$plugin_ohos/entry/src"
  mv "$plugin_ohos/oh-package.json5" "$plugin_ohos/entry/oh-package.json5"
  mv "$plugin_ohos/build-profile.json5" "$plugin_ohos/entry/build-profile.json5"
  [[ -f "$plugin_ohos/index.ets" ]] && mv "$plugin_ohos/index.ets" "$plugin_ohos/entry/index.ets"

  # 模块依赖里的相对路径修正：file:libs/ → file:../libs/
  /usr/bin/sed -i '' 's|"file:libs/|"file:../libs/|g' "$plugin_ohos/entry/oh-package.json5" 2>/dev/null || true

  # 工程级 build-profile.json5（hvigor 6 必需的 modules 数组）
  cat > "$plugin_ohos/build-profile.json5" <<'JSON5'
{
  "app": {
    "signingConfigs": [],
    "products": [
      {
        "name": "default",
        "signingConfig": "default",
        "compatibleSdkVersion": "5.0.0(12)",
        "runtimeOS": "HarmonyOS"
      }
    ]
  },
  "modules": [
    {
      "name": "entry",
      "srcPath": "./entry",
      "targets": [
        {
          "name": "default",
          "applyToProducts": [ "default" ]
        }
      ]
    }
  ]
}
JSON5

  # package.json（hvigor 依赖声明，pnpm 8）
  cat > "$plugin_ohos/package.json" <<'PKGJSON'
{
  "name": "ohos_plugin_build",
  "version": "1.0.0",
  "devDependencies": {
    "@ohos/hvigor": "6.26.8",
    "@ohos/hvigor-ohos-plugin": "6.26.8"
  }
}
PKGJSON

  # hvigor wrapper 配置对齐 6.26.8
  cat > "$plugin_ohos/hvigor/hvigor-config.json5" <<'HVCFG'
{
  "hvigorVersion": "6.26.8",
  "dependencies": {
    "@ohos/hvigor-ohos-plugin": "6.26.8"
  }
}
HVCFG

  # 工程根 oh-package.json5（3.7 工具的存在性判据）
  if [[ ! -f "$plugin_ohos/oh-package.json5" ]]; then
    printf '{
  "name": "ohos_plugin_root",
  "version": "1.0.0",
  "dependencies": {}
}
' > "$plugin_ohos/oh-package.json5"
  fi

  rm -rf "$plugin_ohos/oh_modules" "$plugin_ohos/oh-package-lock.json5" "$plugin_ohos/package-lock.json" "$plugin_ohos/.hvigor"
  echo "standardized: $plugin_ohos"
done
