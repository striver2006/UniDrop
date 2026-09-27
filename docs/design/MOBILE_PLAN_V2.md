# UniClip（瞬贴）移动端开发计划 V2 —— iOS / Android / 鸿蒙，手机 + Pad

| 项 | 内容 |
| :--- | :--- |
| **文档状态** | 定稿（用户已授权直接执行，技术选型接受本方推荐） |
| **编制日期** | 2026-09-27 |
| **取代** | `docs/design/MOBILE_PLAN.md`（v1，其 §1 现状盘点、§2.3 平台事实、§3.1 OS 硬约束与等价替代、§9 风险登记册仍然有效，本文引用不重复） |
| **代码基线** | 客户端 `0.4.2`，HEAD `fb96966` |
| **范围变化** | v1 仅 iOS / Android → V2 增加 **HarmonyOS NEXT（纯血鸿蒙）**，且要求 **Pad 形态** |

---

## 1. 为什么推翻 v1 的技术路线

v1 选定 **Tauri 2 Mobile**，核心论据是「复用 Rust core + 复用 React UI」。加入鸿蒙后该论据失效：

| 事实 | 后果 |
| :--- | :--- |
| Tauri 2 / wry **没有鸿蒙后端**，官方 roadmap 无计划；OpenHarmony 社区移植不可用于生产 | 鸿蒙端必须另写 ArkTS/ArkUI 全套 UI + 平台层 |
| HarmonyOS NEXT 不运行 Android APK | 鸿蒙无法用 Tauri 的 Android 产物兜底 |
| v1 §3.5 本就承认移动端「布局重做」：380×560 托盘弹窗、`data-tauri-drag-region`、`overflow-hidden` 全部不成立 | React UI 的复用价值只剩组件逻辑，布局与交互全重写 |
| 新增 Pad 要求（列表-详情双栏、断点布局） | WebView 壳的窗口模型（v1 §4.5）进一步吃亏 |

结论：**Tauri 路线在「三端 + Pad」语境下退化为「iOS/Android 一套 + 鸿蒙另写一套」，两套 UI、两套平台层、双倍维护**。

## 2. V2 技术决策（已定案）

### 2.1 路线：Flutter（一套 Dart UI → 三端）+ 共享 Rust core + 自研 C ABI FFI

```text
                    ┌────────────────────────────────────────────┐
                    │ crates/unidrop-core（从 src-tauri 抽取）   │
                    │  protocol · e2ee · transfer_engine(滑窗ARQ) │
                    │  connection_actor · tls_trust · path_guard  │
                    │  storage(SQLite) · retention · signal_router│
                    │  EventSink trait ← 桌面/移动各写适配器      │
                    └──────┬─────────────────────────┬───────────┘
                           │ path dep                │ path dep
              ┌────────────┴───────────┐   ┌─────────┴──────────────────┐
              │ client/src-tauri（桌面）│   │ client-mobile/native（FFI）│
              │ Tauri 壳 + 桌面 EventSink│   │ extern "C" + JSON + 事件回调│
              │ React UI（不动）        │   │ cdylib: libunidrop.so/.a   │
              └────────────────────────┘   └─────────┬──────────────────┘
                                                      │ dart:ffi + NativeCallable
                                     ┌────────────────┴────────────────┐
                                     │ client-mobile（Flutter 工程）   │
                                     │ 一套 Dart 代码：iOS / Android / │
                                     │ HarmonyOS（华为 Flutter 分支）  │
                                     │ 手机断点 + Pad 双栏自适应       │
                                     └─────────────────────────────────┘
```

**候选方案裁决**（扩展 v1 §2.1 的对比）：

| 维度 | **Flutter（选定）** | Tauri 2 Mobile（v1） | Kotlin MP | React Native | uni-app |
| :--- | :--- | :--- | :--- | :--- | :--- |
| 鸿蒙 NEXT 支持 | **华为官方维护 flutter_flutter 分支，AppGallery 已大规模商用** | ✗ | ✗（无官方） | 社区，成熟度低 | 有，但原生能力/Rust 接入弱 |
| 三端 UI 代码份数 | **1** | 2（React + ArkTS） | 2–3 | 2 | 1（质量差） |
| Rust core 复用 | dart:ffi（鸿蒙上加载 .so 为官方支持路径） | 原生（iOS/Android） | 原生 | turbo module | 差 |
| Pad 自适应 | Material 3 断点 + NavigationRail，成熟 | WebView 壳弱 | Compose 好 | 一般 | 弱 |
| 主要代价 | **React UI 3,116 行不复用，Dart UI 全新（≈3,000 行）** | 两套平台层 | iOS 侧新 | 鸿蒙风险 | 质量风险 |

「不复用 React UI」的真实代价比看起来小：v1 §3.5 已要求布局、导航、安全区、触控、键盘全部重做，可复用的只是组件逻辑；而换来的是鸿蒙端零额外 UI 成本。

### 2.2 FFI 决策：自研 C ABI 薄层，不用 flutter_rust_bridge

- API 面小（≈30 个函数），`serde_json` 双端都有，`JSON in / JSON out + 错误码` 足够；
- 事件下行用 C 回调 + Dart `NativeCallable.listener`（Dart 3 原生，三端一致）；
- 避免 codegen 工具链与华为 Flutter 分支 / Dart 版本组合的兼容风险，不引入第四种构建依赖；
- 代价：Dart 侧手写模型与 codec（≈300 行），可接受。

### 2.3 其余沿用 v1 的决策

- 复用同一份协议/ARQ/E2EE/TLS/存储代码，**与桌面端零线协议漂移**（v1 §2.1 核心论据继续成立，只是承载方式从「同一 Tauri crate」变为「同一 core crate + 两个壳」）；
- OS 硬约束三条（后台常驻 / 文件级剪贴板注入 / 跨应用剪贴板监听）与等价替代方案照 v1 §3.1 执行；
- 版本号三端同号同步；`app_version` 保持三段式（E2EE 门槛）；
- 服务端 `os_type` 为自由字符串（`ios` / `android` / `ohos` 直接可用，服务端零改动）。

## 3. 本次执行范围（Session 1 交付）

| # | 交付物 | 验收 |
| :--- | :--- | :--- |
| E1 | Cargo workspace + `crates/unidrop-core` 抽取（git mv 保历史；EventSink 抽象；信令路由下沉） | 桌面端 `cargo test` 全绿（166 个既有测试）；桌面构建不受影响 |
| E2 | `client-mobile/native` FFI crate | host `cargo test` + `aarch64-linux-android` / `aarch64-apple-ios(-sim)` / `aarch64-unknown-linux-ohos` 四 target `cargo check` 通过 |
| E3 | `client-mobile` Flutter 工程（iOS/Android/ohos 三平台目录） | `flutter analyze` 干净；Android debug 构建、iOS no-codesign 构建在本机通过（鸿蒙构建需华为 Flutter SDK，见 §5.3） |
| E4 | 完整 UI：设备 / 发送 / 接收卡片 / 历史 / 设置，手机断点 + Pad 双栏 | 真机/模拟器手动走查 |
| E5 | 接收策略 `receive_policy`（v1 §1.3-A 缺口的修复，落在 signal_router） | 蜂窝/询问策略单测 |
| E6 | 文档：本计划 + README 平台矩阵 + 鸿蒙构建说明 | — |

**明确不在本次范围**（沿用 v1 规划，后续 Session）：断点续传（v1 §4.4）、Android 前台服务常驻（v1 M3）、iOS APNs + Share Extension（v1 M4，需服务端 S2/S3）、Share 接收入口、i18n。

## 4. unidrop-core 抽取设计（E1 的细化）

现状盘点（沿用 v1 §1 并经本轮代码复核确认）：

- `protocol/`、`storage/`、`app_state.rs` **零 tauri 依赖**；
- `core/` 11 个文件中 8 个零 tauri；tauri 面集中在：`transfer_engine.rs`（约 15 处 `emit` + 3 处 `state::<AppState>()` + 1 处 `platform::show_transfer_notification`）、`connection_actor.rs`（1 处 emit）、`history_pruner.rs`（整体是壳）；
- `commands/settings_cmd.rs:7-218` 的 `AppSettings` 模型零 tauri，但被 core 反向依赖（core↔commands 循环）；
- `lib.rs:478-860` 的信令分发主循环（TRANSFER_OFFER/ANSWER/COMPLETE/…）是两端共同业务。

抽取手法：

1. 仓库根新建 Cargo workspace：`members = ["crates/unidrop-core", "client/src-tauri", "client-mobile/native"]`；
2. `git mv` `protocol/ storage/ core/ app_state.rs` → `crates/unidrop-core/src/`；`AppSettings` 模型从 `settings_cmd.rs` 拆出为 `core::settings`（`settings_cmd.rs` 改为薄 IPC 壳）；`clipboard_cmd.rs` 中零 tauri 的 `precheck_file_paths` 与业务函数（`dispatch_offer` / `decide_e2ee` 等）把 `State<'_, AppState>` 换成 `&AppState` 后下沉；
3. crate 内模块循环（core↔storage 等）在单一 crate 内自动合法，无需解环；
4. 新增 `core::events`：`UiEvent`（serde，与桌面事件名/载荷保持逐字段兼容）+ `EventSink` trait；`transfer_engine` / `connection_actor` / `history_pruner` 的 `AppHandle` 参数全部换成 `Arc<AppState> + Arc<dyn EventSink>`；
5. `lib.rs` 信令主循环下沉为 `core::signal_router`，桌面 setup 与移动 FFI 层都调它；
6. 桌面端写 `TauriEventSink`（emit → tauri 事件 + 通知），移动端 FFI 写 `JsonEventSink`（序列化推给 Dart 回调）；
7. 守卫：既有 166 个测试随文件迁移且必须全绿；`.github/workflows` 与 `scripts/build-all.sh` 的路径随 workspace 调整。

**移动端关键差异落点**（对齐 v1 §1.2/§4.2）：

- 路径三分：SQLite → `Application Support`（iOS）/ `filesDir`（Android/ohos）；收件目录 → `Documents/UniDrop`（iOS，`UIFileSharingEnabled` + 排除备份）/ 外部私有 Downloads（Android/ohos）；
- `whoami::hostname()` → 平台通道取设备名（iOS `UIDevice.name`、Android `Build.MODEL`、ohos `deviceInfo.marketName`）；
- 剪贴板读写走 Flutter 平台通道（移动端无系统级 API 对 Rust 暴露）；
- 移动端默认块长/窗口参数化下调（4 MB×4 → 2 MB×4 起步）。

## 5. 平台要点

### 5.1 iOS / Android（本机可全量验证）
工具链齐备：Xcode 27、Android SDK + NDK。debug / no-codesign 构建进本次验收；签名与商店流程沿用 v1 §8。

### 5.2 Pad 适配（新增要求）
- 断点：compact `<600` / medium `600–840` / expanded `>840`（Material 3 window size class）；
- expanded：设备页与历史页列表-详情双栏（Master-Detail），设置页双栏；导航在 medium+ 用 `NavigationRail`，compact 用底部 `NavigationBar`；
- 键盘/触控笔外接、横竖屏自由旋转，不做方向锁。

### 5.3 鸿蒙（HarmonyOS NEXT）
- **Dart 代码 100% 共享**；平台差异集中在 `ohos/` 工程目录（华为 Flutter 分支生成）与少量平台通道实现（ArkTS）；
- Rust：`aarch64-unknown-linux-ohos`（rustup tier-3 std 已实测可装），`CC` 指向 DevEco 自带 native llvm（本机 `/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native/llvm`）；
- 交叉编译关键依赖 `rusqlite(bundled)` / `ring` / `tokio-tungstenite` 需在该 target 过 `cargo check`（本次验证）；`ring` 若失败备选 `rustls` aws-lc/rcgen 路径重评；
- Dart FFI 加载 `libunidrop.so` 为华为官方支持路径；
- 推送（后续阶段）：华为 Push Kit 替代 APNs/FCM，服务端网关另立（不在本次）；
- 分发：华为 AppGallery；构建需另行安装华为 Flutter SDK（`flutter_flutter` 分支），本 session 交付接入文档。

## 6. 风险增补（在 v1 §9 之上）

| # | 风险 | 缓解 |
| :--- | :--- | :--- |
| V1 | 抽 crate 触碰桌面端引发回归 | git mv 保历史；166 个既有测试全绿是闸门；改动集中在 EventSink 参数化，语义不变 |
| V2 | 华为 Flutter 分支与官方 stable API 漂移 | 锁定分支版本；三端共享代码避免 `dart:io` 之外的私有 API |
| V3 | `aarch64-unknown-linux-ohos` 上 ring/rusqlite 编译失败 | tier-3 + DevEco llvm 的 CC/AR 环境先验证；失败则该平台降级 sqlite 系统库或换 crypto provider（连带评估 v1 R11 的约束） |
| V4 | 自研 FFI 层内存安全（字符串跨界） | 统一 `CString::into_raw` + Dart 侧 `free` 配对约定；泄漏用 valgrind/ASan 抽查 |
| V5 | workspace 化改动 CI 产物路径 | `build-release.yml` 的 bundle 收集路径同步修改并本地核对 |

## 7. 执行记录

### Session 1（2026-09-27，本计划当日落地）

| # | 交付 | 状态 | 证据 |
| :--- | :--- | :--- | :--- |
| E1 | Cargo workspace + `crates/unidrop-core` 抽取（git mv 保历史；HostBridge 抽象；信令路由 / 发送流程 / 设置保存下沉；receive_policy 三档） | ✅ | commit `5acbfa2`；`cargo test --workspace` 151(core)+14(desktop) 全绿 |
| E2 | `client-mobile/native` FFI crate（JSON 命令 + 事件回调 + 移动默认 wifi_only） | ✅ | commit `d7aba41`；四 target `cargo check` 通过：android(aarch64/x86_64)、ios(设备/模拟器)、**ohos(aarch64，DevEco llvm)** |
| E3 | Flutter 工程（四页签 + 自适应壳 + FFI 桥 + 平台服务 + 构建脚本） | ✅ | commit `d8f41c8`；`flutter analyze` 零问题、6 Dart 测试通过 |
| E4 | Android / iOS 构建闭环 | ✅ | `app-debug.apk` 含双 ABI `.so`；iOS 真机 no-codesign 构建通过，`Runner.debug.dylib` 含 7603 个 Rust 符号（force_load） |
| E5 | 接收策略落地 | ✅ | `signal_router.rs` 三档判定 + Ask 超时看门狗 + 蜂窝阈值守卫测试 |
| E6 | 文档（本计划 + client-mobile/README + 根 README 平台矩阵） | ✅ | — |

**执行中的既有缺口修复**：v1 §1.3-A（无条件自动接受）已由 receive_policy 落地关闭；
§1.3-B（`CLIPBOARD_INJECTED` 回执）与 §1.3-C（断点续传）仍按 v1 排期（后续 Session）。

**执行中发现并绕过的坑**（留给后续维护者）：

1. iOS 静态库必须 `-force_load`：FFI 符号经 `DynamicLibrary.process()` 运行时查找，
   没有静态引用，普通链接会被 dead-strip 整库丢弃（症状：构建成功但运行时
   `Failed to lookup symbol`）。
2. Flutter 3.44 默认 Swift Package Manager：自定义 Podfile 反而破坏构建
   （「non-standard Podfile」迁移报错）；本地静态库改走 xcconfig 直链
   （`OTHER_LDFLAGS[sdk=…]`，真机/模拟器目录分开——同为 arm64 无法 lipo 合并）。
3. pbxproj 残留的 CocoaPods phase（Check Pods Manifest.lock / Pods_Runner 链接）
   在去 Podfile 化后必须清掉，否则 xcodebuild 报 sandbox not in sync。
4. **Xcode 27 + Flutter 3.44.6 的模拟器构建存在环境级 bug**（Flutter.framework
   debug 产物架构校验失败，空白工程同挂）——与本项目无关，真机构建不受影响，
   Flutter 升级后自愈。
5. `file_picker` 8.1 与 compileSdk 36 的新 Gradle 插件冲突（AAR metadata 校验），
   升至 10.x 解决；`share_plus` 11 恢复 `SharePlus.instance.share` API。
6. Xcode 27 最低 deployment target 为 15.0（模板的 13.0 需上调）。

**未完成 / 待后续 Session**（沿用 v1 规划）：Android 前台服务常驻（M3）、
iOS APNs + Share Extension（M4，需服务端 S2/S3）、断点续传（M2.5）、
移动端系统通知前台化展示、真机五端互操作矩阵（v1 §7.2）。

### Session 2（2026-09-27 下午 · 真机实测 + 鸿蒙攻坚）

**真机实测修复（Mi 10 / CZB-iPhone，全部闭环并装机）**：
FFI 事件字符串所有权（NativeCallable 异步投递读到空串）→ 设置页指纹输入框
与常驻保存（切 pinned 档死锁 + 键盘遮挡丢配置）→ 键盘无法收回锁死页面 →
iOS 白屏根因（静态库符号不进动态导出表，-exported_symbols_list 显式导出）→
Android release 三连（INTERNET 权限只在 debug manifest / Impeller Vulkan
在 MIUI 渲染空屏回退 Skia）→ 半开死链（控制面 read 45s 空闲超时，服务端
PONG 为锚）→ 切网重连误伤在途握手（connectivity 同型波动回调，改为类型
真变才重连；Mi10→iPhone 图片失败即此）→ 分享面板从 sheet 内 present 被拒
（先收 sheet 再分享）→ 设置回显时序（IndexedStack 常驻页面 vs 异步
settings 的 hydrate）→ 历史时间 UTC 显示 + SEND 记录误报「已清理」→
设置项补齐（存储四项 + 自动复制，toJson 透传钉了测试）→ App 图标（Mac
同源箭头 + 移动渐变配色）→ iOS 签名改个人 team B8QGM665TS（k_bo@163.com）。

**实测确认可用**：Android 全链路（认证/设备发现/收发/历史/自动注入剪贴板）；
iOS 认证与接收（用户实测收图）；Mac↔Android 双向传输（服务端日志留痕）。

**鸿蒙攻坚（部分落地）**：华为 Flutter 生态唯一工具链完整的基座是
3.7.12/Dart 2.19（3.22 分支无 ohos 工具）。已落地：Rust ohos .so release
编译链（build_native_ohos.sh）、Dart 层 2.19 兼容化（官方 3.44 回归全绿）、
pubspec.ohos.yaml 依赖变体（openharmony-sig 插件 fork + connectivity
4.x/6.x 运行时 shim）、ohos 工程壳 + prepare_ohos_plugin_wrappers.sh、
flutter_tools 两处 patch（模块级 profile 容错 + 扁平布局模块名 '.'）。
卡点：flutter build hap 的插件 har 生成链穿透七层生态断层后止步于
pnpm×华为源运行时 bug（ERR_INVALID_THIS）；恢复路径：等 3.22+ ohos 分支
工具链成熟，或用 DevEco GUI 打开 ohos/ 工程做 hvigor 迁移构建。

---
