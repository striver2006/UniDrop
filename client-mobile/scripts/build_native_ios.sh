#!/bin/zsh
# 构建 iOS 静态库并放进本地 CocoaPods（ios/NativeSources）。
# 之后 flutter build ios 会经 Podfile 自动链接。
# 用法：scripts/build_native_ios.sh [--release]
set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE_FLAG=""
PROFILE_DIR="debug"
if [[ "${1:-}" == "--release" ]]; then PROFILE_FLAG="--release"; PROFILE_DIR="release"; fi

export SDKROOT_iphoneos="$(xcrun --sdk iphoneos --show-sdk-path)"
export SDKROOT_iphonesimulator="$(xcrun --sdk iphonesimulator --show-sdk-path)"

echo "==> cargo build aarch64-apple-ios ($PROFILE_DIR)"
(cd ../ && cargo build -p unidrop-mobile-native --target aarch64-apple-ios ${PROFILE_FLAG})
echo "==> cargo build aarch64-apple-ios-sim ($PROFILE_DIR)"
(cd ../ && cargo build -p unidrop-mobile-native --target aarch64-apple-ios-sim ${PROFILE_FLAG})

DEST="ios/NativeSources"
mkdir -p "$DEST/lib" "$DEST/include"

# 真机与模拟器各一份（同为 arm64，lipo 无法合并；按 SDK 条件链接，见 xcconfig）
mkdir -p "$DEST/lib/device" "$DEST/lib/sim"
cp "../target/aarch64-apple-ios/$PROFILE_DIR/libunidrop_mobile.a" "$DEST/lib/device/"
cp "../target/aarch64-apple-ios-sim/$PROFILE_DIR/libunidrop_mobile.a" "$DEST/lib/sim/"
cat > "$DEST/include/unidrop_mobile.h" <<'EOF'
// UniClip Rust 核心的 FFI 符号声明（与 native/src/lib.rs 的 #[no_mangle] 一一对应）
#ifndef UNIDROP_MOBILE_H
#define UNIDROP_MOBILE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*unidrop_event_callback)(uintptr_t user_data, const char *event_json);

void unidrop_free_string(char *ptr);
void unidrop_set_event_callback(unidrop_event_callback cb, uintptr_t user_data);
char *unidrop_start(const char *config_json);
void unidrop_stop(void);
char *unidrop_invoke(const char *cmd_json);

#ifdef __cplusplus
}
#endif

#endif // UNIDROP_MOBILE_H
EOF

echo "==> $DEST/lib/libunidrop_mobile.a"
