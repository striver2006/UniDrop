#!/bin/sh
set -e

# 确保在 client 目录下运行
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CLIENT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$CLIENT_DIR"

echo "==> 1. 检查代码签名配置..."

# 读取 .env 配置（若存在）
if [ -f .env ]; then
  set -a
  . ./.env
  set +a
fi

# 若环境变量中未配置 APPLE_SIGNING_IDENTITY，自动从系统钥匙串中探测 Developer ID 证书
if [ -z "$APPLE_SIGNING_IDENTITY" ]; then
  DETECTED_IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null | grep "Developer ID Application:" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
  if [ -n "$DETECTED_IDENTITY" ]; then
    echo "自动探测到开发者签名证书: $DETECTED_IDENTITY"
    export APPLE_SIGNING_IDENTITY="$DETECTED_IDENTITY"
  else
    echo "警告: 未找到 Developer ID 证书。未签名的应用在 macOS 上将被系统拒绝通知权限。"
  fi
else
  echo "使用配置的签名身份: $APPLE_SIGNING_IDENTITY"
fi

echo "==> 2. 编译并打包应用 (.app)..."
pnpm tauri build --bundles app

APP_SOURCE="src-tauri/target/release/bundle/macos/UniDrop.app"
if [ ! -d "$APP_SOURCE" ]; then
  echo "错误: 未找到构建产物 $APP_SOURCE"
  exit 1
fi

# 二次复核签名
if [ -n "$APPLE_SIGNING_IDENTITY" ]; then
  echo "==> 3. 校验并强化代码签名..."
  codesign --force --options runtime --deep --sign "$APPLE_SIGNING_IDENTITY" "$APP_SOURCE"
  codesign -dvvv "$APP_SOURCE" 2>&1 | grep -E "Authority=|Identifier=|TeamIdentifier=" || true
fi

echo "==> 4. 安装应用至 /Applications..."
# 若已有实例正在运行，先优雅退出
pkill -f "/Applications/UniDrop.app/Contents/MacOS/unidrop-client" 2>/dev/null || true
sleep 1

rm -rf /Applications/UniDrop.app
cp -R "$APP_SOURCE" /Applications/UniDrop.app

echo "==> 5. 安装完成！"
echo "已成功安装至 /Applications/UniDrop.app (具备开发者账号签名)"
