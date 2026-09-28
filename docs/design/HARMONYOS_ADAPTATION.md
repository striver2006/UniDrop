# 鸿蒙（HarmonyOS NEXT）适配问题与解决方案全记录

> 适用范围：UniClip（瞬贴）移动端 `client-mobile/`，三端（iOS/Android/鸿蒙）共享一套 Dart 代码 + 一份 Rust 核心。
> 工具链基座：openharmony-sig flutter_flutter **3.7.12-ohos-1.0.4**（Dart 2.19）+ DevEco Studio hvigor6 + fork 专属构件桶。
> 真机：HUAWEI Pura 70 Ultra · HarmonyOS 6.1.1（API 24）。
> 本文记录 2026-09-27 ～ 09-28 两轮攻坚中确认的全部问题、定位方法与最终方案。

## 一、问题全景分层

鸿蒙适配的问题按层次分为四层，自下而上互相掩盖，必须逐层穿透：

| 层 | 问题族 | 典型症状 |
| :--- | :--- | :--- |
| 构建链 | hvigor 版本断层、插件布局、签名 | `flutter build hap` 根本不出包 |
| 打包 | Rust .so 不进 HAP、版本回退留旧库 | 启动报「dynamic library not found」/ 闪退 |
| 运行时 | channel 断流、dlopen 路径、引擎竞态 | 设置页无限转圈、键盘不弹、按钮没反应 |
| 测试/CI | 死 cfg、依赖漂移、runner 环境 | Mobile CI 全红 |

## 二、构建链问题

### P1. hvigor 4（2023）与 API 26 SDK（2026）双向不互认

**现象**：`flutter build hap` 主工程阶段炸在 `getModuleProductPath undefined`——华为 3.7.12 工具链自带的 hvigor 4.0.2 读不懂 2026 SDK 的目录结构。

**解法**：`ohos/hvigorw` 改写为委托脚本，直接 exec DevEco Studio 自带的 hvigor6（`/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin/hvigorw`），并兜底导出 `DEVECO_SDK_HOME`（缺它报 00303217 配置错误）。此后 `flutter build hap --debug/--release` 一条命令端到端完成（kernel/AOT 编译 + ohpm + hvigor 签名打包）。

### P2. 设备发现：SDK 目录名必须纯数字

**现象**：hdc 明明连得上手机，`flutter devices` 却看不到 ohos 设备。

**根因**：fork 工具的 SDK 探测只认 `Sdk/<纯数字>/toolchains/hdc` 布局；本机 SDK 目录是 `26.0.0`（带点），`isNumeric` 判空。

**解法**：`ln -sfn 26.0.0 ~/Library/OpenHarmony/Sdk/26`。

### P3. flutter_tools 工程兼容性补丁

- 模块级 `build-profile.json5`（插件的）只有 apiType/targets 没有 modules 数组，老代码对 null 强转直接 crash——1.0.4 上游已含容错，旧检出需补丁。
- **Kotlin DSL 误判**（本地补丁，换检出须重带）：`isUsingGradle` 只认 `build.gradle`，官方 3.44 起新工程默认 `build.gradle.kts`——误判导致 manifest 路径错算到 `android/AndroidManifest.xml`，embedding 判 v1，在 connectivity_plus（仅支持 v2）处致命退出。补丁：加认 `.kts`。

### P4. 变体切换态管理

鸿蒙依赖变体（`pubspec.ohos.yaml`，Dart 2.19 约束 + openharmony-sig fork 插件）与主变体（Dart 3.12）不能共存于同一 `pubspec.yaml` 解析。`scripts/use_ohos_deps.sh [ohos|main]` 负责切换（含 lock 备份/恢复）；**忘记切回主变体就提交会污染仓库**（曾有整次 CI 因此挂掉）。

## 三、打包问题

### P5. Rust 核心不进 HAP

**现象**：124MB 的 HAP 装到真机，Flutter UI 正常但一切核心功能静默缺失。

**根因**：`build_native_ohos.sh` 把 `libunidrop_mobile.so` 拷到工程级 `ohos/libs/`，而 hvigor 只打包 **entry 模块内**的 `entry/libs/`。

**解法**：脚本两处都放，以 `ohos/entry/libs/arm64-v8a/` 为准。

### P6. dlopen 路径与「降级安装陷阱」

鸿蒙的 dlopen 搜索路径不含应用 libs 目录（无 Android 的 classloader namespace 机制）：

- HAP 的 native 库安装在 `/data/storage/el1/bundle/libs/arm64/`（历史包曾落 `el2/base`），`bridge.dart` 按序试 el1 → el2 → 裸名兜底；
- **降级安装陷阱**：`hdc install -r` 遇 versionCode 回退（1.0.4 工具从 pubspec build 号同步 versionCode，旧包是手写的大值）会**留下旧 native 库**——症状是 release 包启动 `Wrong full snapshot version` 闪退，Dart 代码却是新的。先 `hdc uninstall` 再装，或保持 versionCode 单调递增。

## 四、运行时问题

### P7. ★ Dart→ArkTS platform channel 单向断流（本轮核心顽疾）

**现象**：设置页无限转圈；系统键盘不弹；剪贴板读写失效；按钮点了没反应。

**定位链**（全部真机实证）：

1. bootstrap 面包屑（`lib/dbg.dart` 落盘日志）显示挂在 `resolvePaths()`；
2. DartMessenger 侧插件 handler 注册日志齐全，但 `handleMessageFromDart` 入口**零到达**——消息丢在引擎 NAPI 层；
3. 连 `flutter/platform` 系统通道（HapticFeedback 探针）都无回包；
4. VM service `getStack` 显示 isolate 空栈 = 等一个永不完成的 Future，不是同步阻塞。

**根因**：fork 引擎（2024-04 构建）与 HarmonyOS 6.1.1 ROM 的兼容问题——fork README FAQ #8 记载「ROM 更新后不再支持申请有执行权限的匿名内存，debug 运行闪退」，修复于 `a44b8a6d`（2024-07-25）。**换用 3.7.12-ohos-1.0.4（含该修复）后断流痊愈**（剪贴板调用从 2s 超时变为秒回，系统键盘可弹出且带中文输入法）。

**断流期旁路矩阵**（均以 `isOhosRuntime()` 门控，channel 修复后逐条可回退）：

| 断流能力 | 旁路方案 |
| :--- | :--- |
| path_provider（pigeon 永不回包） | 固定沙箱路径直给（`resolvePaths` 鸿蒙分支） |
| 系统键盘不弹 | 内置屏上 ASCII 键盘（`lib/widgets/ohos_keyboard.dart`） |
| 剪贴板读不到 | 超时 2s 不挂起 + 文案说实话 + 手动输入路径 |
| 选文件/分享 | 即时降级提示「暂不支持」，不挂起 |
| 设备名 | 断流期退通用名；痊愈后接 `unidrop/device_name` 通道 |
| 设置改不了（键盘不弹） | URI 注入口（见 P10） |

回退开关：`lib/dbg.dart::kOhosNativeKeyboard`（true=系统键盘，false=屏上键盘）。

### P8. 鸿蒙无观测出口

stderr 不进 hilog，fork 的 `flutter attach` 因 listViews 空数组永远等不到连接。排障资产：

- `lib/dbg.dart`：面包屑落盘 `haps/entry/cache/unidrop-bootstrap.log`（`hdc file recv` 拉取）；
- Rust 侧双写 `{cache_dir}/unidrop-debug.log`；
- `tool/vm_stdout.dart`：裸 dart:io WebSocket 直连 VM service（抓 Stdout/Logging/evaluate）；
- `scripts/ohos_baseline.sh` / `ohos_dart_logs.sh` 一键基线。

### P9. 引擎启动竞态

fork 的 `FlutterAbility.onCreate` 里 `await onAttach`（插件注册在其尾部），但系统的 `onWindowStageCreate` **不等它**，并发执行 Dart entrypoint——bootstrap 早期 channel 调用先于插件注册到达即被静默丢弃。解法：`retryChannel` 超时重试（2s×8，附单测），编排上自愈。

### P10. URI 设置注入口

键盘断流期改不了设置（服务器地址等），且鸿蒙沙箱 hdc shell 不可写。解法：`aa start -U 'unidropmobile://setup?server=…&account=…&psk=…&trust=pinned&pin=<指纹>'` → EntryAbility 解析落盘 `bootstrap_config.json` → Dart 在 `native.start` 前读取并经 `settings` overlay 覆盖（一次性消费）。设置页保存后即持久化，不再需要注入。

### P11. os_type 两处错报 linux

`aarch64-unknown-linux-ohos` 的 `consts::OS` 是 "linux"，真实标识在 `target_env="ohos"`。两处同源修复：`native/src/lib.rs`（原死代码 `cfg!(target_os="ohos")`，CI `-D warnings` 硬错误）与 `crates/unidrop-core/src/app_state.rs`（直取 `consts::OS`）。真机实证上报 "ohos"。

### P12. release AOT 三关

1. `gen_snapshot` 缺失：fork 工具要 `android-arm-profile/darwin-x64/gen_snapshot`；
2. 架构墙：fork 构件只有 x86_64，Apple Silicon 需 Rosetta（`sudo softwareupdate --install-rosetta --agree-to-license`）；
3. **快照版本锁死**：`Wrong full snapshot version, expected '8af47494…' found 'adb4292f…'`——fork 引擎改过 VM 快照哈希，官方 Dart 2.19.6 的 gen_snapshot 产物不匹配（fork README FAQ #7 逐字记载）。解法：`FLUTTER_STORAGE_BASE_URL=https://flutter-ohos.obs.cn-south-1.myhuaweicloud.com` + 删 `bin/cache` 重建，gen_snapshot 与设备引擎严格配套。

### P13. 依赖变体的锁/解析污染

- `.dart_tool/flutter_build/dart_plugin_registrant.dart` 是变体相关生成物，切换变体必须删 `.dart_tool` 重建（否则主线 3.12 语法喂给 2.19 编译器爆炸）；
- `dart compile`（主线 SDK）在 client-mobile 目录跑会顺手把 package_config 写回主线版本——同样的坑；
- pub-cache 里的 fork 插件被昨天的 wrapper 脚本改过：1.0.4 工具走官方源码 har 流，需要 `git reset --hard && git clean -fdx` 复原。

## 五、测试 / CI 问题（Mobile CI 首跑 5 红 → 全绿）

| 失败 | 根因 | 修复 |
| :--- | :--- | :--- |
| Core & FFI Tests | 死 cfg `target_os="ohos"` 在 `-D warnings` 下硬错误 | 改判 `target_env="ohos"` |
| Cross-compile ×3 | `setup-rust-toolchain` 输入名 `targets:` 应为 `target:` | 改名 |
| 同上 | runner 上 `ndk-bundle` 早已不存在 | 改 `$ANDROID_NDK_LATEST_HOME`，CC/AR/linker 按三元组拼 |
| 同上 iOS | ring/aws-lc-sys 需要 xcrun | iOS job 移到 `macos-latest` |
| Flutter Analyze | info 级 deprecation 在 3.47 起致命；3.44 同样 exit 1（此前误判是管道遮蔽退出码） | 钉 Flutter 3.44.6 + `--enforce-lockfile` + 代码侧清 issue |
| Android job | StderrLogger/LOGGER 在 Android 上是死代码 | `#[cfg(not(target_os = "android"))]` 门控 |

## 六、UX 回归问题

- **按钮「没反应」**：modal bottom sheet 盖住了 SnackBar，且不支持时静默 return。修法：发送入口统一返回 bool + 成败都提示 + 面板内联反馈行（不依赖被遮挡的 SnackBar）+ 成功自动收面板；
- **剪贴板误报**：断流期超时兜底 null 把「读不到」报成「没有」。文案改为实话并指向手动输入；
- **中文输入**：channel 痊愈后回到系统键盘（真机实证拼音候选「啊」入框），屏上键盘降级为回退；
- **真实设备名**：三端统一 `unidrop/device_name` 通道——Android 读 `Settings.Global.device_name`，鸿蒙读 `settings.general.DEVICE_NAME`（回落 marketName），iOS 用 `iosInfo.name`。

## 七、当前限制与后续路线

- fork 引擎与 ROM 的兼容是长期风险：跟住 openharmony-sig 3.7.12-ohos 标签（或评估 3.22+ 分支）；
- 断流旁路（固定路径/屏上键盘/URI 注入）保留为防御性回退，`isOhosRuntime()`/`kOhosNativeKeyboard` 两个开关即可拆除；
- file_picker/share_plus 无 ohos 实现，鸿蒙端选文件/分享暂缺（等 openharmony-sig 适配或自研 ArkTS 通道）；
- 分发上架走 AppGallery + 正式签名（当前为 debug 证书侧载）。

## 附录 A：命令速查

```bash
# 环境（一次性）
ln -sfn 26.0.0 ~/Library/OpenHarmony/Sdk/26
export PATH="<Flutter-Ohos>/bin:~/Library/OpenHarmony/Sdk/26.0.0/toolchains:\
/Applications/DevEco-Studio.app/Contents/tools/ohpm/bin:\
/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin:$PATH"
export PUB_HOSTED_URL=https://pub.flutter-io.cn
export FLUTTER_STORAGE_BASE_URL=https://flutter-ohos.obs.cn-south-1.myhuaweicloud.com

# 构建出包
./scripts/use_ohos_deps.sh ohos            # 切鸿蒙依赖变体（main 切回）
./scripts/build_native_ohos.sh --release   # Rust → ohos .so
flutter build hap --release                # AOT + 签名一体（delegation 到 DevEco hvigor6）

# 装机与排障
hdc uninstall com.unidrop.unidrop_mobile   # 降级陷阱：versionCode 回退先卸载
hdc install ohos/entry/build/default/outputs/default/entry-default-signed.hap
hdc shell aa start -a EntryAbility -b com.unidrop.unidrop_mobile
hdc shell "aa start -a EntryAbility -b com.unidrop.unidrop_mobile -U \
  'unidropmobile://setup?server=…&account=…&psk=…&trust=pinned&pin=…'"
./scripts/ohos_baseline.sh                 # 启动→拉日志→截图 一键基线
```
