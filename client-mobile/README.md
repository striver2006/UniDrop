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
| 鸿蒙 | 见下节 | 代码就绪，构建需华为 Flutter SDK |
| 交叉检查 | `./scripts/check_ohos.sh` | aarch64-unknown-linux-ohos 的 cargo check（DevEco llvm） |

已知环境问题：Xcode 27 + Flutter 3.44.6 的 `--simulator` 构建存在环境级不兼容
（空白工程同样失败，`Flutter.framework` debug 产物的架构校验 bug）；真机构建不受影响，
升级 Flutter 后即可恢复。

## 鸿蒙（HarmonyOS NEXT）构建

鸿蒙端 **Dart 代码与本仓库完全一致**（`lib/` 不含平台分支），差异只在平台壳：

1. 安装[华为 Flutter 分支](https://gitee.com/openharmony-sig/flutter_flutter)
   （`dev` 分支跟随官方 stable 版本），设 `PATH` 指向其 `bin/flutter`；
2. 生成 ohos 平台目录（一次性）：
   ```bash
   flutter config --enable-ohos
   flutter create --platforms ohos .
   ```
3. 编译 Rust 为鸿蒙 .so（与 Android 同法，工具链换 DevEco llvm）：
   ```bash
   # aarch64-unknown-linux-ohos，CC/AR 见 scripts/check_ohos.sh
   cargo build -p unidrop-mobile-native --target aarch64-unknown-linux-ohos --release
   cp ../target/aarch64-unknown-linux-ohos/release/libunidrop_mobile.so \
      ohos/libs/arm64-v8a/   # 目录名以生成的工程为准
   ```
4. `flutter build hap --release`（需 DevEco 签名配置），产物上架华为 AppGallery。

依赖插件的鸿蒙适配来自 openharmony-sig 的 [flutter_packages](https://gitee.com/openharmony-sig/flutter_packages)
（path_provider / device_info_plus / connectivity_plus 等均有 ohos 实现）；
`file_picker`、`share_plus` 若缺位，`PlatformService` 已按能力探测降级设计，
可后续接 openharmony-sig 的对应适配或 ArkTS 通道。

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
