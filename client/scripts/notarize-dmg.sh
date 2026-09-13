#!/bin/sh
# 补公证 .dmg
#
# Tauri 的 macOS 流程是「公证 .app → 把已公证的 .app 打进 .dmg → 给 .dmg 签名」，
# 它**不会单独公证 .dmg**。结果是从 dmg 里拖出来的 app 带票据可以正常用，
# 但 dmg 本身仍被判为 Unnotarized Developer ID，别人下载后打开就会被拦。
#
# 本脚本由 package.json 的 tauri:build 在构建之后调用，环境变量已由该脚本
# 从 .env 载入。凭证不全时**跳过而不失败**：本机自用的构建不该因为没配公证而报错。
set -e

BUNDLE_DIR="src-tauri/target/release/bundle/dmg"

# 仅 macOS 有意义；其他平台直接跳过
if [ "$(uname -s)" != "Darwin" ]; then
  exit 0
fi

if [ ! -d "$BUNDLE_DIR" ]; then
  exit 0
fi

DMG=$(ls -t "$BUNDLE_DIR"/*.dmg 2>/dev/null | head -1)
if [ -z "$DMG" ]; then
  echo "notarize-dmg: 未找到 dmg 产物，跳过"
  exit 0
fi

# 两种凭证任选其一，与 .env.example 中的说明一致
if [ -n "$APPLE_API_KEY" ] && [ -n "$APPLE_API_ISSUER" ] && [ -n "$APPLE_API_KEY_PATH" ]; then
  set -- --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER"
elif [ -n "$APPLE_ID" ] && [ -n "$APPLE_PASSWORD" ] && [ -n "$APPLE_TEAM_ID" ]; then
  set -- --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID"
else
  echo "notarize-dmg: 未配置公证凭证，跳过 dmg 公证（app 的公证由 tauri 自行处理）"
  exit 0
fi

# app 已在 tauri build 阶段公证过；若那一步被跳过，这里公证 dmg 也没有意义，
# 但不阻断——让 notarytool 自己去判定并给出准确错误。
echo "notarize-dmg: 提交 $(basename "$DMG") 公证…"
xcrun notarytool submit "$DMG" "$@" --wait

echo "notarize-dmg: 装订票据…"
xcrun stapler staple "$DMG"

# 装订后复验：让「脚本成功」严格等于「Gatekeeper 真的放行」
xcrun stapler validate "$DMG"
echo "notarize-dmg: 完成"
