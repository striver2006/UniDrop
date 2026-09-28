#!/bin/bash
# 重启鸿蒙 App 并抓取本次启动的 Dart stdout（经 VM service，绕过 fork 的
# attach listViews 缺陷）。用法：scripts/ohos_dart_logs.sh [秒数]
set -e
HDC=/Users/chenzhenbo/Library/OpenHarmony/Sdk/26.0.0/toolchains/hdc
DART=/Users/chenzhenbo/DevLib/Flutter/flutter/bin/dart
DURATION="${1:-30}"
BUNDLE=com.unidrop.unidrop_mobile

$HDC shell aa force-stop $BUNDLE >/dev/null 2>&1 || true
$HDC shell hilog -r >/dev/null 2>&1 || true
$HDC shell aa start -a EntryAbility -b $BUNDLE >/dev/null 2>&1

TOKEN=""
PORT=""
for i in $(seq 1 40); do
  LINE=$($HDC shell "hilog -x 2>/dev/null | grep 'listening on' | tail -1" | tr -d '\r')
  if [[ "$LINE" =~ http://127.0.0.1:([0-9]+)/([^/]+)/ ]]; then
    PORT="${BASH_REMATCH[1]}"
    TOKEN="${BASH_REMATCH[2]}"
    break
  fi
  sleep 0.5
done
if [[ -z "$PORT" ]]; then
  echo "未能从 hilog 拿到 VM service 地址" >&2
  exit 1
fi
echo "==> VM service: 127.0.0.1:$PORT token=$TOKEN" >&2
$HDC fport tcp:$PORT tcp:$PORT >/dev/null

cd "$(dirname "$0")/.."
BIN=/tmp/vm_stdout
if [[ ! -x "$BIN" ]]; then
  "$DART" compile exe tool/vm_stdout.dart -o "$BIN" >&2
fi
"$BIN" "ws://127.0.0.1:$PORT/$TOKEN/ws" &
WATCHER=$!
sleep "$DURATION"
kill $WATCHER >/dev/null 2>&1 || true
$HDC fport rm tcp:$PORT tcp:$PORT >/dev/null 2>&1 || true
wait $WATCHER 2>/dev/null || true
