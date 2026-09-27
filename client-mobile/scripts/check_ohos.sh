#!/bin/zsh
# 鸿蒙（HarmonyOS NEXT）target 的交叉编译检查。
# 依赖 DevEco Studio 自带的 OpenHarmony native llvm（本脚本自动定位）。
#
# CI runner 无 DevEco，此检查在本机或 self-hosted runner 执行：
#   client-mobile/scripts/check_ohos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

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

cd ..
cargo check -p unidrop-mobile-native --target aarch64-unknown-linux-ohos
echo "==> OHOS aarch64 check 通过"
