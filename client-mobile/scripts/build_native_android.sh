#!/bin/zsh
# 构建 Rust 原生库并拷贝进 Android 工程（jniLibs 由 AGP 自动打包）。
# 用法：scripts/build_native_android.sh [--release]
set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE_FLAG=""
PROFILE_DIR="debug"
if [[ "${1:-}" == "--release" ]]; then PROFILE_FLAG="--release"; PROFILE_DIR="release"; fi

NDK_DIR="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/27.0.12077973}"
TOOLCHAIN="$NDK_DIR/toolchains/llvm/prebuilt/darwin-x86_64/bin"
if [[ ! -d "$TOOLCHAIN" ]]; then
  TOOLCHAIN="$NDK_DIR/toolchains/llvm/prebuilt/darwin-arm64/bin"
fi
if [[ ! -d "$TOOLCHAIN" ]]; then
  echo "找不到 NDK 工具链：$NDK_DIR（可用 ANDROID_NDK_HOME 指定）" >&2
  exit 1
fi

# arm64 真机 + x86_64 模拟器。armv7 老设备按需再加（v1 计划 API 24+）。
echo "==> cargo build aarch64-linux-android (${PROFILE_DIR})"
export CC_aarch64_linux_android="$TOOLCHAIN/aarch64-linux-android24-clang"
export CXX_aarch64_linux_android="$TOOLCHAIN/aarch64-linux-android24-clang++"
export AR_aarch64_linux_android="$TOOLCHAIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/aarch64-linux-android24-clang"
(cd .. && cargo build -p unidrop-mobile-native --target aarch64-linux-android ${PROFILE_FLAG})
mkdir -p android/app/src/main/jniLibs/arm64-v8a
cp ../target/aarch64-linux-android/${PROFILE_DIR}/libunidrop_mobile.so \
   android/app/src/main/jniLibs/arm64-v8a/

echo "==> cargo build x86_64-linux-android (${PROFILE_DIR})"
export CC_x86_64_linux_android="$TOOLCHAIN/x86_64-linux-android24-clang"
export CXX_x86_64_linux_android="$TOOLCHAIN/x86_64-linux-android24-clang++"
export AR_x86_64_linux_android="$TOOLCHAIN/llvm-ar"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$TOOLCHAIN/x86_64-linux-android24-clang"
(cd .. && cargo build -p unidrop-mobile-native --target x86_64-linux-android ${PROFILE_FLAG})
mkdir -p android/app/src/main/jniLibs/x86_64
cp ../target/x86_64-linux-android/${PROFILE_DIR}/libunidrop_mobile.so \
   android/app/src/main/jniLibs/x86_64/

echo "==> 完成：android/app/src/main/jniLibs/{arm64-v8a,x86_64}/libunidrop_mobile.so"
