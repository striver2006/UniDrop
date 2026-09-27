#!/bin/zsh
# 构建 Rust 核心为鸿蒙 .so 并拷入 ohos 工程的 libs 目录。
# 用法：scripts/build_native_ohos.sh [--release]
set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE_FLAG=""
PROFILE_DIR="debug"
if [[ "${1:-}" == "--release" ]]; then PROFILE_FLAG="--release"; PROFILE_DIR="release"; fi

DEVECO="${DEVECO_STUDIO_DIR:-/Applications/DevEco-Studio.app}"
OHOS_LLVM="$DEVECO/Contents/sdk/default/openharmony/native/llvm/bin"
if [[ ! -d "$OHOS_LLVM" ]]; then
  echo "找不到 OHOS llvm：$OHOS_LLVM（安装 DevEco Studio 或用 DEVECO_STUDIO_DIR 指定）" >&2
  exit 1
fi

export CC_aarch64_unknown_linux_ohos="$OHOS_LLVM/aarch64-unknown-linux-ohos-clang"
export CXX_aarch64_unknown_linux_ohos="$OHOS_LLVM/aarch64-unknown-linux-ohos-clang++"
export AR_aarch64_unknown_linux_ohos="$OHOS_LLVM/llvm-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="$OHOS_LLVM/aarch64-unknown-linux-ohos-clang"

echo "==> cargo build aarch64-unknown-linux-ohos ($PROFILE_DIR)"
(cd .. && cargo build -p unidrop-mobile-native --target aarch64-unknown-linux-ohos ${PROFILE_FLAG})

# ohos 工程的 libs 目录（flutter create --platforms ohos 生成后存在）
DEST="ohos/libs/arm64-v8a"
if [[ ! -d ohos ]]; then
  echo "ohos/ 工程尚未生成（见 README 鸿蒙章节），先拷到暂存目录"
  DEST="build/ohos-libs/arm64-v8a"
fi
mkdir -p "$DEST"
cp "../target/aarch64-unknown-linux-ohos/$PROFILE_DIR/libunidrop_mobile.so" "$DEST/"
echo "==> $DEST/libunidrop_mobile.so"
