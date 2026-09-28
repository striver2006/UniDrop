#!/bin/bash
# 一键鸿蒙真机基线：启动 App → 等 bootstrap → 拉取 Dart 面包屑日志与
# Rust unidrop-debug.log → 截图。用法：scripts/ohos_baseline.sh
set -e
HDC=/Users/chenzhenbo/Library/OpenHarmony/Sdk/26.0.0/toolchains/hdc
BUNDLE=com.unidrop.unidrop_mobile
SBX=/data/app/el2/100/base/$BUNDLE

$HDC shell aa force-stop $BUNDLE >/dev/null 2>&1 || true
$HDC shell aa start -a EntryAbility -b $BUNDLE
echo "==> 已启动，等待 bootstrap 15s…"
sleep 15

echo "==> bootstrap 面包屑（Dart 段）："
$HDC file recv $SBX/haps/entry/cache/unidrop-bootstrap.log /tmp/unidrop-bootstrap.log >/dev/null 2>&1 \
  && cat /tmp/unidrop-bootstrap.log || echo "(无 bootstrap 日志——resolvePaths 之前就挂了)"

echo "==> Rust 核心日志："
$HDC file recv $SBX/haps/entry/files/flutter/UniDrop/unidrop-debug.log /tmp/unidrop-debug.log >/dev/null 2>&1 \
  && tail -30 /tmp/unidrop-debug.log || echo "(无 unidrop-debug.log——native.start 未完成)"

$HDC shell snapshot_display -f /data/local/tmp/baseline.jpeg >/dev/null 2>&1
$HDC file recv /data/local/tmp/baseline.jpeg /tmp/baseline.jpeg >/dev/null 2>&1
echo "==> 截图已存 /tmp/baseline.jpeg"
