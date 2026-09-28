# UniClip 移动端（iOS / Android / 鸿蒙 · 手机 + Pad）

一套 Dart 代码 + 一份 Rust 核心（`crates/unidrop-core`），三个平台共享。
架构与决策见 `docs/design/MOBILE_PLAN_V2.md`。

```
client-mobile/
├── lib/            # Flutter UI（四页签 + 自适应壳 + FFI 桥）
├── native/         # Rust FFI 层（unidrop-mobile-native，C ABI）
├── scripts/
│   ├── build_native_android.sh   # Rust → jniLibs（AGP 自动打包）
│   ├── build_native_ios.sh       # Rust → ios/NativeSources 静态库（force_load）
│   ├── build_native_ohos.sh      # Rust → 鸿蒙 .so（拷入 entry/libs 与工程 libs）
│   ├── use_ohos_deps.sh          # 主/鸿蒙 pubspec 变体切换（含 lock 备份恢复）
│   ├── prepare_ohos_plugin_wrappers.sh  # 插件 hvigor wrapper（pub get 后重跑）
│   ├── standardize_ohos_plugins.sh      # 插件目录结构标准化
│   ├── ohos_baseline.sh          # 真机：启动→拉日志→截图 一键基线
│   ├── ohos_dart_logs.sh         # 真机：VM service 直连抓 Dart stdout
│   └── check_ohos.sh             # 鸿蒙 target 交叉编译检查（本机）
├── android/        # Flutter 生成的 Android 工程
└── ios/            # Flutter 生成的 iOS 工程（含静态库链接配置）
```

## 快速开始

```bash
cd client-mobile

# 1. Rust 原生库（改过 crates/ 或 native/ 后重跑）
./scripts/build_native_android.sh          # debug；--release 出正式包
./scripts/build_native_ios.sh              # debug；--release 出正式包

# 2. Flutter
flutter pub get
flutter run                                # 连接设备/模拟器
```

构建产物验证（本机已通过的基线）：

| 平台 | 命令 | 说明 |
| :--- | :--- | :--- |
| Android | `flutter build apk --debug` | arm64-v8a + x86_64 的 `libunidrop_mobile.so` 打包在内 |
| iOS 真机 | `flutter build ios --no-codesign --debug` | 静态库 force_load 进主二进制（FFI 运行时查找无静态引用，普通链接会被 dead-strip） |
| 鸿蒙 | 真机构建/运行已通（Pura 70 Ultra · HarmonyOS 6.1） | 见下节；channel 断流有旁路 |
| 交叉检查 | `./scripts/check_ohos.sh` | aarch64-unknown-linux-ohos 的 cargo check（DevEco llvm） |

已知环境问题：Xcode 27 + Flutter 3.44.6 的 `--simulator` 构建存在环境级不兼容
（空白工程同样失败，`Flutter.framework` debug 产物的架构校验 bug）；真机构建不受影响，
升级 Flutter 后即可恢复。

## 鸿蒙（HarmonyOS NEXT）构建与调试

鸿蒙端 Dart 代码与本仓库完全一致（`lib/` 无平台分支），差异在平台壳与工具链。
工具链基座：**openharmony-sig flutter_flutter 3.7.12（Dart 2.19）** + DevEco Studio
自带 hvigor6（`flutter build hap` 经 `ohos/hvigorw` 委托给 DevEco，已是端到端一条命令）。

```bash
# 0. 一次性环境准备
ln -sfn 26.0.0 ~/Library/OpenHarmony/Sdk/26   # 华为工具的设备发现只认纯数字目录名
export PATH="/Users/chenzhenbo/DevLib/Flutter-Ohos/bin:\
/Users/chenzhenbo/Library/OpenHarmony/Sdk/26.0.0/toolchains:\
/Applications/DevEco-Studio.app/Contents/tools/ohpm/bin:$PATH"
export PUB_HOSTED_URL=https://pub.flutter-io.cn FLUTTER_STORAGE_BASE_URL=https://storage.flutter-io.cn

# 1. 切依赖到鸿蒙变体（pubspec.ohos.yaml；切回用 main）
./scripts/use_ohos_deps.sh ohos

# 2. Rust → 鸿蒙 .so（改过 crates/ 或 native/ 后重跑；产物进 ohos/entry/libs/arm64-v8a/）
./scripts/build_native_ohos.sh --release

# 3. 出包（kernel 编译 + ohpm + hvigor 签名一体），安装到真机
flutter build hap --debug
hdc install -r ohos/entry/build/default/outputs/default/entry-default-signed.hap
hdc shell aa start -a EntryAbility -b com.unidrop.unidrop_mobile
```

**真机调试与排障**（stderr 不进 hilog，通道断流时 attach 也不可用）：
- Rust 核心日志：`{cache_dir}/unidrop-debug.log`；Dart bootstrap 面包屑：
  `haps/entry/cache/unidrop-bootstrap.log`（沙箱路径见 `lib/dbg.dart`），
  `hdc file recv` 拉取，或 `scripts/ohos_baseline.sh` 一键拉取+截图。
- VM service 直连（绕过 fork attach 的 listViews 缺陷）：`scripts/ohos_dart_logs.sh`。
- 锁屏会拦截 `aa start`；远程注入设置（键盘断流时的改配置通道）：
  ```bash
  hdc shell "aa start -a EntryAbility -b com.unidrop.unidrop_mobile -U \
    'unidropmobile://setup?server=wss%3A%2F%2F<host>%3A<port>&account=<账号>&psk=<密钥>&trust=pinned&pin=<sha256指纹>'"
  ```

**当前平台限制**（fork 引擎 Dart→ArkTS platform channel 断流的实测规避，
根因待 fork 修复后回退）：剪贴板读写/文件选择/系统分享不可用
（`PlatformService` 已降级为不挂起）；设置页表单用内置屏上 ASCII 键盘
（`lib/widgets/ohos_keyboard.dart`）；设备名退化为通用名。

依赖插件的鸿蒙适配来自 openharmony-sig 的 [flutter_packages](https://gitee.com/openharmony-sig/flutter_packages)；
`path_provider` 的 pigeon 通道在断流设备上不可达，`resolvePaths` 走固定沙箱路径旁路。

服务端**零改动**：`os_type` 是自由字符串，移动端上报 `ios` / `android` / `ohos`。

## 架构约定

- **业务全部在 Rust**（`unidrop-core`）：协议 / ARQ / E2EE / TLS 信任 / 存储 /
  信令路由 / 接收策略。Dart 层只做展示与平台能力（文件选择 / 分享 / 剪贴板 /
  通知 / 网络类型）。
- **命令**：`NativeBridge.invoke(cmd, args)` —— 全部异步，立即回 `call_id`，
  结果经 `invoke-result` 事件投递（永不阻塞 UI isolate）。
- **事件**：事件名与桌面端 Tauri 事件逐字段一致（两端共用同一套事件协议），
  经 `NativeCallable.listener` 进入 Dart。
- **接收策略**：移动端首启默认 `wifi_only`（蜂窝下大文件转确认）；桌面默认
  `always`。网络类型由 Dart 侧 `connectivity_plus` 上报。
- **路径三分**：SQLite 在 `Application Support` / `filesDir`（参与备份），
  收件目录在 `Documents/UniDrop`（iOS 文件 App 可见）。

## 测试

```bash
cd client-mobile && flutter test          # Dart 单测（断点 / 模型）
cd .. && cargo test -p unidrop-core -p unidrop-mobile-native   # Rust 单测
```
