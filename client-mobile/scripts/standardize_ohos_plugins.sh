#!/bin/zsh
# openharmony-sig fork 插件布局守卫（2026-09-29 重写）。
#
# 历史：上游插件曾是「src/main 直挂 + 工程级 build-profile 缺失」的布局，
# 本脚本曾把它们重排成 entry/ 标准结构并补工程级 profile。2026-09-28 起
# 上游 master 已迁移为官方新结构——扁平 src/main + **模块级** build-profile
# （含 apiType/targets）——flutter 工具扫描与 hvigor 的
# `--mode module assembleHar`（经主工程 hvigorw 组插件 HAR，schema 校验
# 要求插件根 build-profile 为模块级）都原生通吃。
# 此时再重排反而会让 hvigor 模块模式 schema 校验失败（00303038），
# 所以本脚本的主职责改为：检测原生结构则跳过；发现旧布局则告警
# （仅当该插件真被依赖时才会阻塞构建——hvigor 会给出明确报错；
# 修法：删 pub-cache 对应 git 缓存重新 resolve，让 pub 拉原生结构）。
set -euo pipefail

for plugin_ohos in ~/.pub-cache/git/flutter_packages-*/packages/*/*_ohos/ohos \
                   ~/.pub-cache/git/flutter_plus_plugins-*/packages/*/*/ohos; do
  [[ -d "$plugin_ohos" ]] || continue
  if [[ -f "$plugin_ohos/src/main/module.json5" ]] &&
     grep -q '"apiType"' "$plugin_ohos/build-profile.json5" 2>/dev/null; then
    echo "native: $plugin_ohos"
    continue
  fi
  echo "warn: 非官方新结构（未用到的插件无碍；若构建报 00303038，删缓存重 resolve）：$plugin_ohos" >&2
done
