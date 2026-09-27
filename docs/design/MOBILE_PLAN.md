 # UniDrop（瞬贴）iOS / Android 移动端客户端开发计划
 
 | 项 | 内容 |
 | :--- | :--- |
 | **文档状态** | 草案 v1（待双审） |
 | **编制日期** | 2026-09-27 |
 | **代码基线** | 客户端 `0.4.2`（`client/src-tauri/Cargo.toml`、`tauri.conf.json`、`package.json` 三处一致）；Tauri 实际锁定 `2.11.5`（`client/src-tauri/Cargo.lock:4287`）；HEAD `5fcd77e` |
 | **范围** | 新增 iOS / Android 两端客户端，功能对齐既有 Windows / macOS / Linux 桌面端 |
 | **本文档的性质** | **纯计划。编制过程中未修改任何现有代码**，仓库工作树保持原状。文中所有「现状」结论均给出可核对的文件与行号 |
 | **配套文档** | `docs/design/REQUIREMENTS.md`（PRD）、`docs/design/DESIGN.md`（LLD）、`docs/需求.md`（需求台账） |
 
 > [!IMPORTANT]
 > 本仓库启用 TriviumCode「主攻 + 双审」流水线（见 `CLAUDE.md`、`.trivium/config.yaml`）。
 > 本文档属计划类产出，落地前应经 `/dual-review-plan`，留痕归 `.reviews/mobile-client/plan/`。
 
 ---
 
 ## 目录
 
 - [0. 摘要](#0-摘要)
 - [1. 现状盘点](#1-现状盘点)
 - [2. 技术路线决策](#2-技术路线决策)
 - [3. 「功能一致」的边界：能力差异矩阵](#3-功能一致的边界能力差异矩阵)
 - [4. 目标架构](#4-目标架构)
 - [5. 服务端配套改动清单](#5-服务端配套改动清单)
 - [6. WBS 与里程碑](#6-wbs-与里程碑)
 - [7. 测试与验收标准](#7-测试与验收标准)
 - [8. CI/CD、签名与上架](#8-cicd签名与上架)
 - [9. 风险登记册](#9-风险登记册)
 - [10. 工时汇总与人力建议](#10-工时汇总与人力建议)
 - [11. 待决策项](#11-待决策项)
 - [12. 下一步](#12-下一步)
 
 ---
 
 ## 0. 摘要
 
 | 项 | 结论 |
 | :--- | :--- |
 | **技术路线** | Tauri 2 Mobile（复用现有 Rust core + React UI）+ 自研原生移动插件。**不做原生重写，不换 Flutter / React Native** |
 | **可直接复用** | Rust 侧约 **8,560 / 12,277 行（≈70%）**：协议编解码、滑动窗口 ARQ、E2EE、TLS 三档信任、PathGuard、SQLite、保留策略、连接 actor 全部平台无关 |
 | **必须新写** | 移动端平台层（Rust ≈1,200 行）+ Kotlin / Swift 原生插件（≈3,000 行）+ 移动端 UI 壳 |
 | **「功能一致」的硬边界** | **3 处 OS 层面不可能等价**：后台常驻、文件级剪贴板注入、跨应用剪贴板监听。已给出平台原生等价替代（§3.1），需产品拍板（§11-5） |
 | **服务端配套** | 仅 iOS 后台接收需要（APNs + 离线暂存），约 **12 人日**，属本项目首次服务端功能性改动 |
| **工作量** | 客户端 **120 人日**（不做断点续传则 108）+ 服务端 **14 人日**（不做续传则 12）；单人 ≈24–27 周，**3 人并行 ≈14–18 周** |
 | **盘点副产品** | 发现 **6 处既有缺口**（§1.3），其中 3 处（接收端无条件自动接受、DB 建在缓存目录、断点续传未实现）会直接影响移动端可用性，已排入任务而非另开主题 |
 
 ---
 
 ## 1. 现状盘点
 
 ### 1.1 可复用资产（平台无关，移动端零改动即可运行）
 
 | 模块 | 行数 | 复用说明 |
 | :--- | ---: | :--- |
 | `protocol/`（`envelope.rs` + `binary_header.rs`） | 422 | 控制面 JSON Envelope 与数据面 64 B 定长帧，与服务端 `server/internal/protocol/` 严格对齐，**逐字节一致是硬要求** |
 | `core/connection_actor.rs` | 318 | 控制面 WS、HMAC-SHA256 盐化签名（`UNIDROP_V1\n{account}\n{device}\n{nonce}\n{ts}\n{salt}`）、指数退避 + Jitter、`reconnect_notify` 即时唤醒 |
 | `core/transfer_engine.rs` | 1,611 | 数据面 WS、滑动窗口 W=4、动态 RTO、ACK / NACK、CRC32（算在**密文**上）、SHA-256 整文件校验 |
 | `core/e2ee.rs` | 506 | HKDF-SHA256 会话密钥、AES-256-GCM、确定性 nonce（接收端自行重算）、元数据密封、版本门槛 |
 | `core/tls_trust.rs` | 1,477 | `public_ca` / `pinned` / `insecure` 三档 + 六类证书失败分类与对症文案 |
 | `core/sliding_window.rs`、`path_guard.rs`、`retention.rs`、`history_pruner.rs` | 615 | 全平台无关；`retention.rs` 已把「常量与 SQL 字面量两套值」的陷阱收敛掉 |
 | `storage/`（`db.rs` + `history_repo.rs` + `bitmap_repo.rs`） | 1,069 | SQLite schema、账号分区、归属闸门 |
 | `commands/`（除 `window_cmd.rs`） | 1,662 | 21 个 IPC 命令中的 18 个可直接复用或轻量改造 |
 | `app_state.rs` | 110 | 仅 `whoami_hostname()` 需换原生实现 |
 | 前端 `src/`（React 18 + Tailwind） | 3,116 | 组件与事件接线可复用，布局需重做 |
 
 **依赖交叉编译可行性**（`client/src-tauri/Cargo.toml`）：`rusqlite` 用 `bundled`、`rustls` 走 `ring` feature、`tokio-tungstenite` 用 `rustls-tls-webpki-roots`——**不依赖 OpenSSL 或任何系统 C 库**，这是移动端可行的关键前提（M0 仍需实测确认，见 R11）。
 
 **工程侧已就绪**：`crate-type = ["staticlib", "cdylib", "rlib"]`（`Cargo.toml:10-12`）已满足 Tauri 移动端要求；`icons/ios/`（21 个尺寸）与 `icons/android/`（mipmap 全套 + adaptive icon）已由 `tauri icon` 生成完毕。
 
 ### 1.2 桌面专属、移动端不成立的实现
 
 | 位置 | 内容 | 移动端处置 |
 | :--- | :--- | :--- |
 | `lib.rs:60-160`、`platform/tray_placement_macos.rs`(1,019 行)、`platform/tray_click_macos.rs`(157 行) | 菜单栏托盘、macOS 26 放置探测与看门狗、Accessory 激活策略 | 无对应物，全部 `#[cfg(desktop)]` 隔离 |
 | `lib.rs:304` `pub fn run()` | **缺 `#[cfg_attr(mobile, tauri::mobile_entry_point)]`** | 必须补，否则移动端没有入口点（`main.rs` 那条路径在移动端不存在） |
 | `lib.rs:437-455` 插件注册 | `tauri-plugin-single-instance`（Android 官方不支持）、`tauri-plugin-autostart`（仅 Windows / macOS / Linux） | 移动端**不得注册**，需源码级守卫测试钉住 |
 | `commands/history_cmd.rs:56` | `dialog().file().blocking_pick_folder()` —— 移动端**无文件夹选择器**，且 `blocking_*` 系列在移动端不可用 | 换 Android SAF / iOS `UIDocumentPicker` |
 | `commands/history_cmd.rs:111-140` `cmd_reveal_session` | `open -R` / `explorer /select,` / `xdg-open` | 换「分享 / 保存到… / 用其他应用打开」 |
 | `platform/mod.rs:31-115` | 四个剪贴板函数在非 Win/macOS/Linux 下直接返回 `Err("Unsupported platform")` / `ClipboardContent::Empty` | 新增 `clipboard_ios.rs` / `clipboard_android.rs` 并接入分派 |
 | `platform/mod.rs:184-190` | `start_clipboard_listener` 仅对三桌面平台 re-export | 新增移动端 re-export |
 | `core/cache_manager.rs:30-54` | 移动端会落到兜底分支 `std::env::temp_dir()`，而 iOS / Android 上 `/tmp` 不可写 | 必须改为注入 Tauri path resolver |
 | `storage/db.rs:6-10` | **数据库建在缓存目录内**（`resolve_default_cache_dir()/unidrop.db`） | 移动端致命（系统会清缓存）；桌面端同为隐患，见 §1.3-E |
 | `index.html` 的 `overflow-hidden`、`App.tsx:384-395` 的 `data-tauri-drag-region`、`tauri.conf.json` 的 380×560 无边框固定窗口 | 托盘弹窗式桌面 UI 假设 | 移动端另做壳（§4.5） |
 | `commands/window_cmd.rs`(120 行) | `cmd_hide_window`、托盘放置引导、菜单栏设置入口 | 移动端不适用，返回 no-op / `None` |
 
 ### 1.3 盘点中发现的 6 处既有缺口
 
 这些不是移动端新增需求，而是**盘点桌面端时发现的既有状态**，其中 A / C / E 三项是移动端跑起来的前置条件，已排入 §6 的任务而非另开主题。
 
 | # | 缺口 | 证据 | 对移动端的影响 | 排期 |
 | :--- | :--- | :--- | :--- | :--- |
 | **A** | **接收端无条件自动接受**：`accepted: true` 硬编码，无用户确认、无网络类型策略 | `lib.rs:568-578` | **阻断级**。蜂窝网下静默拉取 256 MB（服务端默认单次总量上限，`server/internal/limits/limits.go:25`）不可接受 | M2 |
 | **B** | **`CLIPBOARD_INJECTED` 回执从未发送**：PRD §1.1 第 3 条「注入反馈回执」只在枚举里声明 | `protocol/envelope.rs:22`，全仓库无发送点 | 移动端「已保存 / 已装载」同样需要这条回执，顺手补齐 | M2 |
 | **C** | **断点续传未实现**：`BitmapRepo` 无生产调用、`resumed_items` 恒空 | `core/e2ee.rs:118-121` 注释自述、`lib.rs:549` 与 `:573` | 移动端中断是常态（切后台 / 锁屏 / 蜂窝切换），优先级由「可选」升为「强烈建议」 | M2.5 |
 | **D** | **剪贴板监听只有 macOS 接线**：Windows / Linux 的 `listener_*.rs` 是空函数体 | `lib.rs:914` 的 `#[cfg(target_os = "macos")]`；`listener_windows.rs:7-10`、`listener_linux.rs:6-9` 均为注释占位 | 说明「自动同步」目前本就是单平台特性；移动端按前台监听实现即可，**不算功能降级** | 记录 |
 | **E** | **SQLite 与缓存同目录，且位于系统缓存区** | `storage/db.rs:6-10` + `core/cache_manager.rs:38-43`（macOS 落 `~/Library/Caches/UniDrop/cache/`） | `device_id`、PSK、账号、全部历史都在这一个库里。移动端必须迁移；桌面端建议同批修（含一次性文件搬迁） | M1 |
 | **F** | **目录不展开传输**（选目录被静默跳过） | `transfer_engine.rs:168-171`、`clipboard_cmd.rs:187-189` | 与移动端能力天然一致，无需变更，仅需 UI 文案对齐 | 记录 |
 
 ---
 
 ## 2. 技术路线决策
 
 ### 2.1 三方案对比
 
 | 维度 | **方案 A：Tauri 2 Mobile（选定）** | 方案 B：原生双端（SwiftUI + Compose）+ Rust core via UniFFI | 方案 C：Flutter / React Native + Rust core |
 | :--- | :--- | :--- | :--- |
 | 协议 / ARQ / E2EE 复用 | **100% 同一份 Rust 代码**，与桌面端逐字节一致 | 复用 core，但需新建 FFI 边界层（≈2,000 行）+ 两套语言绑定 | 同 B，另加 JS/Dart bridge |
 | 与桌面端线协议漂移风险 | **零**（同一 crate、同一 `Cargo.lock`） | 中（FFI 序列化边界易漂） | 中 |
 | UI 工作量 | 复用 3,116 行 React，只做移动壳 | 两套全新 UI（≈6,000 行） | 一套全新 UI（≈3,000 行） |
 | 平台原生能力（前台服务 / 推送 / Share Extension） | 需自研 Tauri 移动插件（Kotlin / Swift） | 原生最佳 | 需插件 |
 | 安全关键代码重复 | 无 | 无，但边界层新增攻击面 | 无，同 |
 | 团队技能要求 | Rust + 少量 Kotlin / Swift | Swift + Kotlin + Rust 三栈 | Dart + Rust |
 | 相对工期 | **1.0×（最短）** | ≈2.2× | ≈1.5× |
 | 主要风险 | iOS 后台限制、WebView 差异、移动插件生态成熟度 | 三栈长期维护成本 | 与现有 Tauri 资产割裂 |
 
 **淘汰理由**：本项目最贵的资产是 `core/` 的 5,392 行——滑动窗口 ARQ、E2EE 的 nonce 构造与密钥域分隔、PathGuard、TLS 六档失败分类。这些是**安全与正确性关键**代码且已有单测覆盖（例如 `core/e2ee.rs:319+` 的 nonce 唯一性论证、`core/retention.rs:56-70` 的 u32 回绕用例）。方案 B / C 都要求为它新建跨语言边界，等于给「已经跑通的加密与可靠性逻辑」再引入一层翻译缺陷面；而收益（UI 原生度）对本项目这种托盘型工具 App 并不关键。
 
 ### 2.2 选定方案：Tauri 2 Mobile + 自研原生插件（混合）
 
 ```text
 ┌────────────────────────────────────────────────────────────┐
 │  共享 Rust core（一份代码，五端同一套协议/ARQ/E2EE/存储）  │
 │  protocol · connection_actor · transfer_engine · e2ee      │
 │  tls_trust · sliding_window · path_guard · storage         │
 └───────────────────────▲────────────────────────────────────┘
                         │ 平台抽象（新增 platform::{ios,android}）
 ┌───────────────────────┴────────────────────────────────────┐
 │  自研 Tauri 插件 unidrop-mobile（Kotlin / Swift 原生实现） │
 │  文件选择 · 分享 · 前台服务 · 推送 · 剪贴板 · 设备名       │
 └───────────────────────▲────────────────────────────────────┘
                         │ Tauri IPC（命令 + 事件）
 ┌───────────────────────┴────────────────────────────────────┐
 │  React UI：桌面壳（现有） + 移动壳（新增 src/mobile/）     │
 └────────────────────────────────────────────────────────────┘
 ```
 
 **一条架构约束必须先说清楚**：Tauri 移动端的 Rust core 与 Android Activity / iOS 主 App 处于**同一进程**。因此 Android 侧「常驻」的上限是「用前台服务提升该进程的优先级」，而不是「把 core 搬进独立 Service 进程」。这不是实现取巧的问题，而是框架边界，M0-T0.8 与 M3 必须实测各 ROM 的真实存活率（见 R2）。
 
 ### 2.3 关键平台事实
 
 | 事实 | 对本计划的影响 | 来源 / 复核方式 |
 | :--- | :--- | :--- |
 | Tauri 2 移动端已稳定；Android 最低 **SDK 24（Android 7.0）**；iOS 部署目标由生成的 Xcode 工程决定（默认 13.0，**建议上调至 16.0**） | 决定支持范围与商店声明 | M0-T0.1 以 `tauri android init` / `tauri ios init` 生成的工程配置为准 |
 | `tauri-plugin-single-instance`：Android ✗；`tauri-plugin-autostart`：仅桌面三平台 | 移动端必须 `#[cfg(desktop)]` 隔离注册 + 守卫测试 | 官方插件支持矩阵；M0 复核 |
 | `tauri-plugin-dialog`：移动端 ✓，但**无文件夹选择器**；`blocking_*` 系列移动端不可用 | `history_cmd.rs:56` 必须重写 | 官方文档「Mobile Usage Notes」；M0-T0.5 复核 |
 | `tauri-plugin-notification` / `tauri-plugin-log`：移动端 ✓ | 通知与落盘日志可直接沿用（Android 13+ 需 `POST_NOTIFICATIONS` 运行时权限） | M0 复核 |
 | `tauri-plugin-shell`：移动端仅 `open` 类能力可用，进程派生不可用 | `cmd_open_menu_bar_settings` / `cmd_open_notification_settings` 需平台分流 | M0 复核 |
 | Android 14+ 前台服务**必须声明类型**；**Android 15 起 `dataSync` 类型每 24 h 限 6 h** | 常驻方案需 `specialUse` 或到期重调度 + 用户可见说明 | M3 实测 |
 | Android 10+ 禁止后台读剪贴板；Android 12+ 读取时系统弹隐私提示 | 剪贴板同步只能前台，且需 UI 说明 | M3 实测 |
 | iOS 无持久后台；退后台约 30 s 后 socket 挂起；`BGTaskScheduler` 执行时机不保证 | iOS「关屏即收」必须靠 APNs + 服务端离线暂存 | M4 |
 | iOS 16+ 读剪贴板需用户授权（系统横幅）；`UIPasteboard` 的 detect 类 API 可免弹窗预判 | 需两段式读取设计（先 detect 再读） | M2 实测 |
 | Rust 侧原生 TCP socket 是否受 iOS ATS / Android cleartext 策略约束 | **预期不受**（不经 `NSURLSession` / `OkHttp`），但必须实测；若受则需 `NSAppTransportSecurity` 例外或 `network_security_config.xml` | **M0-T0.6 必测** |
 
 ---
 
 ## 3. 「功能一致」的边界：能力差异矩阵
 
 ### 3.1 三条 OS 硬约束与原生等价替代（**需产品拍板，见 §11-5**）
 
 | # | 桌面端行为 | OS 约束 | 移动端等价方案（建议） | 等价度 |
 | :--- | :--- | :--- | :--- | :--- |
 | **1** | **文件注入系统剪贴板**，在任意目录 `Cmd+V` / `Ctrl+V` 落地（`CF_HDROP` / `NSPasteboard.fileURL` / `text/uri-list`） | iOS 沙盒下跨应用文件粘贴**不成立**；Android 仅部分应用接受 `content://` 形式的 `ClipData` | **iOS**：收件落 `Documents/UniDrop`，开启 `UIFileSharingEnabled` + `LSSupportsOpeningDocumentsInPlace` → 该目录在**「文件」App 中直接可见**，用户可在文件 App 内复制 / 移动 / 粘贴到任意位置（这是 iOS 上真正等价的路径）；叠加「分享 / 保存到… / 存相册 / 用其他应用打开」<br>**Android**：收件落应用外部私有目录 + 可选「自动存入系统下载目录（MediaStore）」；同时**尽力而为**地把 `content://` URI（FileProvider + `FLAG_GRANT_READ_URI_PERMISSION`）写入 `ClipData`，并在 UI 明示「可在支持粘贴的应用中粘贴，或使用『保存到…』」 | iOS ≈85%<br>（路径不同、结果等价）<br>Android ≈60%<br>（依赖目标应用） |
 | **2** | **常驻托盘守护进程**，7×24 在线接收 | iOS 不允许；Android 可，但受厂商策略与 Android 15 限制 | **Android**：前台服务（常驻通知）+ 开机自启 + 电池优化 / 厂商自启白名单引导 + 各 ROM 存活率实测<br>**iOS**：前台连接；退后台依赖 **APNs 可见推送**（点击 → App 前台化 → 领取会话 → 建立数据面拉取），静默推送仅作加速；**依赖服务端离线暂存**（PRD §4.2.3 已列为可选功能，本项目尚未实现） | Android ≈90%<br>iOS ≈70%<br>（有延迟、需用户点一下） |
 | **3** | **后台监听系统剪贴板**并自动同步（现仅 macOS 接线，见 §1.3-D） | Android 10+ 后台禁读；iOS 无后台剪贴板访问 | 两端均**前台监听**：Android `ClipboardManager.addPrimaryClipChangedListener`（前台生效）；iOS `changeCount` 轮询 + `detectPatterns` 免弹窗预判 + 用户手势触发读取。沿用 PRD §5.3.2 的密码管理器隐私规避清单（`org.nspasteboard.ConcealedType`、`com.agilebits.onepassword`，与 `platform/listener_macos.rs:31-40` 一致）；以「手动发送剪贴板」为主路径 | ≈70% |
 
 ### 3.2 21 个 IPC 命令逐条处置（注册点：`lib.rs:965-987`）
 
 | 命令 | 处置 | 移动端要点 |
 | :--- | :--- | :--- |
 | `cmd_get_online_devices` | **直接复用** | 前端补 ios / android 图标（`components/DeviceList.tsx:16-18` 现只有 Monitor / Laptop / Terminal） |
 | `cmd_get_self_info` | 复用 + 改造 | `whoami::hostname()`（`app_state.rs:75-86`）在移动端不可靠 → 走原生取设备名，并支持用户自定义显示名 |
 | `cmd_inject_files` | **语义替换** | → 「保存到… / 分享 / 存相册」；文本与图片仍写系统剪贴板 |
 | `cmd_send_files` | 复用 + 改造 | 路径来源换成 SAF / `UIDocumentPicker`。**Android SAF 返回 `content://`，Rust `File::open` 打不开** → 方案 b（首选）：Kotlin 侧开 `ParcelFileDescriptor` 传 fd，Rust 用 `File::from_raw_fd`，零拷贝；方案 a（兜底）：先复制进应用私有目录。M0-T0.4 决策。限额预检 `precheck_file_paths`（`clipboard_cmd.rs:172-232`）与「目录不展开」口径原样复用 |
 | `cmd_get_settings` / `cmd_save_settings` | 复用 + 扩展 | 新增字段必须带**自定义** `#[serde(default = "…")]`——`settings_cmd.rs:19-105` 的注释已把「裸 default 让 `0` 静默变成不限制」这个坑写清楚了，照办即可 |
 | `cmd_get_autostart` / `cmd_set_autostart` | 平台分流 | Android → `RECEIVE_BOOT_COMPLETED` + 前台服务开关；iOS → 返回不支持，前端隐藏该项。`core/startup.rs` 的 `--silent` 真值表在移动端不适用，保持桌面专属 |
 | `cmd_hide_window` | 移动端 no-op | 移动端无窗口隐藏语义，改为隐藏入口 |
 | `cmd_read_clipboard_preview` | 复用 + 改造 | iOS 16+ 授权态处理；不可读时返回 `EMPTY` + 原因，**不得静默失败** |
 | `cmd_send_clipboard` | 复用 + 改造 | 同上；四条发送路径共用的 `decide_e2ee` / `emit_fallback`（`clipboard_cmd.rs:305-356`）原样复用 |
 | `cmd_inject_session` | **语义替换** | 移动端 = 「装载 / 保存该会话」，成功后同样发 `CLIPBOARD_INJECTED` 回执（补 §1.3-B）；归属闸门 `ensure_session_owned`（`history_cmd.rs:29-38`）必须保留 |
 | `cmd_list_history` | **直接复用** | 账号分区、显示上限 200（`history_cmd.rs:15`）不变 |
 | `cmd_save_transfer_as` | 改造 | Android `ACTION_CREATE_DOCUMENT`；iOS `UIDocumentPicker` exportAsCopy；重名追加 `(n)` 的逻辑（`history_cmd.rs:75-95`）复用 |
 | `cmd_reveal_session` | **语义替换** | → 系统查看器 / 分享面板 / 跳转「文件」App |
 | `cmd_get_server_limits` | **直接复用** | 六项限额只读语义不变（`server/internal/limits/limits.go:22-40`），客户端不可写这条边界同样不变 |
 | `cmd_get_tray_placement`<br>`cmd_dismiss_tray_guidance`<br>`cmd_open_menu_bar_settings` | **移动端不适用** | 返回 `None` / no-op；前端不挂载 `MenuBarHiddenBanner.tsx` |
 | `cmd_get_notification_auth_status`<br>`cmd_open_notification_settings` | 复用 + 改造 | Android 13+ `POST_NOTIFICATIONS` 运行时权限；iOS `UNUserNotificationCenter`。`platform/notification.rs:14-29` 的三态返回（`None` = 本平台无此概念或探测失败）设计正好可沿用 |
 
 **移动端新增命令（约 10 个，落在自研插件里）**：
 `cmd_mobile_pick_files`、`cmd_mobile_share_session`、`cmd_mobile_save_to_photos`、`cmd_mobile_save_to_documents`、`cmd_mobile_set_device_name`、`cmd_mobile_get_network_info`（Wi-Fi / 蜂窝，服务于「仅 Wi-Fi 自动接收」）、`cmd_mobile_set_foreground_service`、`cmd_mobile_request_battery_optimization_exempt`、`cmd_mobile_request_notification_permission`、`cmd_mobile_register_push_token`。
 
 ### 3.3 事件面（前端接线全部复用）
 
 现有事件在移动端语义不变，`App.tsx:211-333` 的注册与清理逻辑可整体复用：
 `auth-success`、`auth-failed`、`tls-cert-failed`（六档分类载荷）、`e2ee-fallback`、`e2ee-offer-rejected`、`devices-updated`、`transfer-progress`、`transfer-offer-received`、`server-limits-updated`、`account-changed`、`history-pruned`、`open-settings`。
 
 两处已知的闭包陷阱必须原样保留（不要「顺手重构」）：`App.tsx:96-105` 的 `settingsRef`（监听 effect 依赖数组是 `[]`，直接读 `settings` 会永远拿到默认值）与 `App.tsx:107-113` 的 `dismissTimersRef`（按 `session_id` 记账，否则同一张卡片会堆积多个定时器）。
 
 桌面专属事件（`tray-placement` 等）移动端不发送，前端对应横幅不挂载。
 
 ### 3.4 设置项映射（`AppSettings`，`settings_cmd.rs:6-105`）
 
 | 桌面端字段 | iOS | Android |
 | :--- | :--- | :--- |
 | `server_url` / `account_id` / `psk_secret` | ✓ 相同 | ✓ 相同 |
 | `auto_inject` | 重定义为「接收后自动写入剪贴板（文本 / 图片）」+「自动保存到文件 App」 | 同 iOS，另加「自动写入文件剪贴板（尽力而为）」 |
 | `start_minimized` | **不适用**（前端隐藏） | 重定义为「启动后不弹主界面，仅驻留通知」 |
 | `history_max_entries` / `transfer_card_retain_secs` | ✓ 相同（每账号语义不变） | ✓ 相同 |
 | `cache_ttl_hours` / `cache_max_size_mb` / `cache_sweep_interval_minutes` | ✓ 机制相同，**默认值需下调**：容量 10 GB → 2 GB（手机磁盘与 iCloud 备份压力）；收件目录须置 `isExcludedFromBackup = true` | ✓ 机制相同，默认容量建议 2 GB |
 | `tls_trust_mode` / `pinned_cert_sha256` / `legacy_allow_insecure_tls` | ✓ 完全复用。三档 + 六类失败分类在移动端（弱网、企业自签部署）价值更高 | ✓ 同 |
 | `e2ee_enabled` | ✓ 完全复用，默认 `true`（安全侧默认值规则不变） | ✓ 同 |
 | **新增** | `device_display_name`、`receive_policy`（`always` / `wifi_only` / `ask`）、`auto_save_destination`、`push_enabled`、`cellular_upload_allowed`、`window_size_mobile` | 同 iOS，另加 `foreground_service_enabled`、`auto_start_on_boot`、`save_to_mediastore` |
 
 > 新增字段一律进 `AppSettings`（前端是唯一写入方）；**后端自己写的开关不得混入**，必须走 `storage/db.rs` 的 `get_local_flag` / `set_local_flag`——该文件注释已说明混入会造成 read-modify-write 覆盖。
 
 ### 3.5 前端处置
 
 **复用**：`DeviceList`、`TransferProgress`、`HistoryPanel`、`SettingsModal`、`ErrorBoundary`、`NotificationPermBanner`，以及 `types/index.ts` 的全部类型与 `certFailureStatusText`（`:138-158`）。
 
**重做**：布局壳（底部 Tab 导航替代 380×560 固定弹窗）、安全区（`env(safe-area-inset-*)`）、触控目标 ≥44 pt、长按菜单、下拉刷新、空态、软键盘遮挡与 resize、去掉 `data-tauri-drag-region` 与 `overflow-hidden`、`select-none` 收窄（历史面板需可选中复制）。`MenuBarHiddenBanner` 不挂载。

---

## 4. 目标架构

### 4.1 目录与代码组织（全部为新增，不改动现有文件语义）

```text
client/
├── src-tauri/
│   ├── gen/apple/                     # 新：tauri ios init 生成（Xcode 工程）
│   ├── gen/android/                   # 新：tauri android init 生成（Gradle 工程）
│   ├── capabilities/mobile.json       # 新：移动端权限集（platforms 字段限定）
│   ├── Info.plist                     # 增补：UIBackgroundModes / UIFileSharingEnabled /
│   │                                  #       LSSupportsOpeningDocumentsInPlace /
│   │                                  #       NSPhotoLibraryAddUsageDescription /
│   │                                  #       ITSAppUsesNonExemptEncryption / App Group
│   └── src/
│       ├── platform/
│       │   ├── paths_mobile.rs        # 新（P0）：DB / 临时下载 / 收件落地 三类路径解析
│       │   ├── clipboard_ios.rs       # 新：UIPasteboard 读写 + iOS 16 授权态
│       │   ├── clipboard_android.rs   # 新：ClipboardManager + ClipData(content://)
│       │   ├── listener_ios.rs        # 新：前台 changeCount 轮询 + detect 预判
│       │   ├── listener_android.rs    # 新：前台 PrimaryClipChangedListener
│       │   ├── share_ios.rs           # 新：UIActivityViewController / 存相册
│       │   ├── share_android.rs       # 新：ACTION_SEND / MediaStore
│       │   └── lifecycle_mobile.rs    # 新：Pause / Resume → reconnect_notify
│       └── commands/mobile_cmd.rs     # 新：§3.2 的 10 个移动端命令
├── plugins/unidrop-mobile/            # 新：自研 Tauri 插件（Rust + Kotlin + Swift）
│   ├── src/lib.rs
│   ├── android/…/ForegroundService.kt · SafBridge.kt · ClipboardBridge.kt ·
│   │            BootReceiver.kt · BatteryGuide.kt · FileProvider 配置
│   └── ios/Sources/DocumentPicker.swift · ShareSheet.swift · Pasteboard.swift ·
│              Push.swift · AppGroup.swift
├── ios-share-extension/               # 新（M4）：独立 Xcode target，App Group 共享容器
└── src/mobile/                        # 新：移动端壳（Tab 导航、安全区、手势）

server/                                # M2.5 / M4 才动（本计划阶段不动）
├── internal/push/                     # 新：APNs / FCM 网关 + push token 注册
└── internal/staging/                  # 新：离线暂存（TTL）+ 待领取队列
```

### 4.2 存储与路径策略（P0，含桌面端一次性迁移）

现状 `init_database(None)` 把库建在缓存目录内（`storage/db.rs:6-10`）。移动端必须拆成三类路径：

| 用途 | iOS | Android | 备注 |
| :--- | :--- | :--- | :--- |
| **SQLite**（设备 ID / PSK / 账号 / 历史 / 位图） | `Application Support/` | `filesDir/` | **参与系统备份，绝不可放 Caches / cacheDir** |
| **传输中临时分片** | `Caches/UniDrop/tmp/` | `cacheDir/` | 可随时被系统回收，符合语义 |
| **收件落地（用户可见）** | `Documents/UniDrop/` + `isExcludedFromBackup = true` + `UIFileSharingEnabled` | `getExternalFilesDir(DIRECTORY_DOWNLOADS)`，可选镜像写入 `MediaStore.Downloads` | TTL / LRU 清理仍按 `core/retention.rs` 生效 |

**迁移要求（M1 验收项）**：桌面端同批修，且必须做**一次性文件搬迁**——旧路径存在则 move 到新路径，move 失败则**保留旧库并告警**，绝不新建空库。否则老用户的 `device_id` 变更会导致服务端注册表出现幽灵旧设备、PSK / 账号丢失、历史清零。迁移必须幂等，且在迁移完成前不得启动清理循环（`lib.rs` 的 cache sweep worker）。

**iOS 备份体积**：`Documents` 默认参与 iCloud 备份，GB 级收件目录会撑爆用户备份。收件目录必须置 `isExcludedFromBackup`，这是 `cache_max_size_mb` 在移动端的对应义务。

### 4.3 后台与生命周期策略

**Android**
- 单 Activity 进程 + 前台服务提升优先级（见 §2.2 的框架约束）。
- 服务类型：Android 14+ 必须声明；`dataSync` 在 Android 15 有 6 h/24 h 限制 → 采用 `specialUse` 并在 Play Console 说明用途，或 `dataSync` + `AlarmManager` 到期重调度 + 通知栏可见说明。
- 开机自启：`RECEIVE_BOOT_COMPLETED` → 映射到 `cmd_get_autostart` / `cmd_set_autostart`，保持「事实源是操作系统、本地不留副本」这条既有原则（`docs/需求.md` 第 2 条）。
- 厂商白名单：小米 / 华为 / OPPO / vivo 各一条引导 intent，失败时降级为图文引导。

**iOS**
- 前台连接；`RunEvent::Pause` / `Resume` 触发 `reconnect_notify`（复用 `connection_actor.rs:73-88` 的即时唤醒机制，无需新增退避逻辑）。
- 退后台依赖 APNs：可见推送（用户点击 → 前台化 → 领取会话 → 数据面拉取）为主，静默推送（`content-available`）仅作加速，**不得作为可靠性依据**（唤醒时机不保证）。

**两端共同：接收策略（修 §1.3-A）**
落点在 `lib.rs:568` 那个无条件 `accepted: true` 的分支：
- `receive_policy = always`：保持现状（桌面端默认）。
- `receive_policy = wifi_only`（**移动端建议默认**）：蜂窝网下，小体积（文本 / 图片且低于阈值）自动接收，文件类转 `ask`。
- `receive_policy = ask`：弹确认，超时未响应则按现有 `reject_reason` 通路拒绝（`lib.rs:545-556` 已有该通路，无需新增协议字段）。
- 网络类型判定走 `cmd_mobile_get_network_info`；判定失败时**保守按蜂窝处理**（安全侧默认值规则，与 `settings_cmd.rs` 的注释精神一致）。

### 4.4 断点续传（M2.5，建议纳入）

移动端中断是常态。协议侧字段已就位：`resumed_items`（`protocol/envelope.rs:198`）、服务端 `server/internal/protocol/envelope.go:159`；客户端 `BitmapRepo` 与 `chunk_bitmaps` 表已建好但无生产调用。

落地路径：接收端保留部分文件 + 位图 → 重连后在 `TRANSFER_ANSWER` 回报 `resumed_items` → 发送端只发缺失块。

**服务端依赖**：当前同 `session_id` 重新授权会撞 `SESSION_ID_CONFLICT`（`server/internal/limits/limits.go:52`），需放开「同账号同设备的续传重授权」，并复核授权条目 TTL 与管道空闲存活的联动（`docs/需求.md` 第 6 条 ⑤ 已修过一次同类问题）。

**E2EE 硬约束**（`core/e2ee.rs:118-121` 已写明）：续传**不得复用 `session_id` 传不同内容**——确定性 nonce 的安全性正建立在「同 (key, nonce) 只加密同一份明文」上。这条必须转成续传自己的验收用例。

### 4.5 UI 壳与窗口模型

| 维度 | 桌面（现状） | 移动端（目标） |
| :--- | :--- | :--- |
| 窗口 | 380×560、`decorations: false`、`resizable: false`、初始 `visible: false`，由托盘唤出 | 全屏、系统导航、随 App 生命周期 |
| 导航 | 单面板 + 弹窗（`SendModal` / `SettingsModal`） | 底部 Tab：设备 / 传输 / 历史 / 设置；弹窗改全屏 sheet |
| 关闭语义 | `CloseRequested` → `hide()` + `prevent_close()`（`lib.rs:959-963`） | 系统返回手势；Android 返回键不退出前台服务 |
| 常驻提示 | 托盘图标 + tooltip | Android 常驻通知；iOS App 图标角标 + 推送 |

`tauri.conf.json` 需为移动端提供独立窗口配置或按平台条件化（Tauri 2 支持 `bundle.iOS` / `bundle.android` 分节；窗口配置的移动端语义以 M0-T0.1 实测为准）。

---

## 5. 服务端配套改动清单

> 本计划阶段**不改动服务端代码**，此处仅列出依赖项，供排期与决策。

| # | 改动 | 必要性 | 工作量 | 说明 |
| :--- | :--- | :--- | ---: | :--- |
| **S1** | `os_type` 接受 `"ios"` / `"android"` | **已满足** | 0 | 服务端只存储与日志输出（`internal/registry/session.go:18`、`internal/controller/control_ws.go:138` 与 `:166`），无枚举校验；`internal/protocol/envelope.go:63` 那句 `"windows" / "macos" / "linux"` 只是注释。Rust 的 `std::env::consts::OS` 在移动端天然产出 `ios` / `android` |
| **S2** | APNs / FCM 推送网关 + push token 注册信令（新 action）+ registry 增列 | iOS 后台接收必需 | 7 人日 | token 必须与 `account_id` 绑定校验，防止跨账号投递 |
| **S3** | 离线暂存（TTL Staging）+ 待领取队列 | iOS 后台接收必需 | 5 人日 | PRD §4.2.3 已列为可选功能；须满足「超时物理擦除」与「不落明文」两条既有要求 |
| **S4** | 放开同 `session_id` 的续传重授权 | 断点续传必需 | 2 人日 | 见 §4.4 |
| **S5** | 移动端限额建议默认值 | 建议 | 0 | 纯 env 配置（`UNIDROP_MAX_*`）。蜂窝下 128 MB 单文件 / 256 MB 单次偏大，建议为移动账号单独部署一组更小的值，或由客户端 `receive_policy` 兜住 |
| **S6** | STUN / P2P 移动端**暂不启用** | 决策项 | 0 | iOS 后台不允许长 UDP socket；`server/internal/stun/` 保持桌面专用 |

---

## 6. WBS 与里程碑

> 优先级：**P0** = 首期必做；**P1** = 应做；**P2** = 可选。人日为单人有效工作日。

### M0 · 可行性 Spike（10 人日，P0，**go/no-go 闸门**）

| 任务 | 交付物 / 验收 |
| :--- | :--- |
| T0.1 `tauri ios init` / `tauri android init`；补 `#[cfg_attr(mobile, tauri::mobile_entry_point)]`（`lib.rs:304`）；空壳跑通真机 WebView | 两端真机各一张运行截图；生成的 `gen/apple`、`gen/android` 工程可打开可构建 |
| T0.2 交叉编译验证：`rusqlite(bundled)` / `ring` / `tokio-tungstenite` / `image` × `aarch64-apple-ios`、`aarch64-apple-ios-sim`、`aarch64-linux-android`、`armv7-linux-androideabi`、`x86_64-linux-android` | 每个 target 的 `cargo check` 日志；需 patch 的 crate 清单与替代方案 |
| T0.3 真机连现有服务器跑通控制面：`AUTH_CHALLENGE → AUTH_REQUEST → AUTH_RESPONSE`、设备上线广播、限额下发 | 抓包 + 服务端 `device authenticated` 日志（`control_ws.go:166`）；设备列表出现 `IOS` / `ANDROID` |
| T0.4 **Android SAF → Rust 读文件**：fd 传递（方案 b）vs 先复制（方案 a） | 二选一结论 + 可运行 demo；含 100 MB 文件的耗时与峰值内存对比 |
| T0.5 iOS 文件选择能力实测（`tauri-plugin-dialog` mobile vs 自研 `UIDocumentPicker`）+ Files App 可见性（`UIFileSharingEnabled`） | 结论 + 截图；确认「文件 App 里能看到 UniDrop 目录并粘贴到别处」这条核心等价路径成立 |
| T0.6 iOS ATS / Android cleartext 对 Rust 原生 socket 是否生效 | 结论，覆盖 `ws://` 与自签 `wss://` 两种；若受限，给出最小例外配置 |
| T0.7 WebView 上 React UI 渲染与 IPC 吞吐（1 MB 文本、4 MB 图片） | 帧率、IPC 往返耗时、内存占用数据 |
| T0.8 Android 前台服务 + Rust core 同进程存活初测（1 款原生 ROM + 1 款国产 ROM） | 灭屏 30 分钟存活结论与日志 |

**闸门判据**：T0.2 / T0.4 / T0.5 / T0.7 任一证伪 → 路线需重评（回到 §2.1 选方案 B 或 C）。

### M1 · 移动端基座（15 人日，P0）— 依赖 M0

- 路径三分与桌面端一次性迁移（§4.2，含幂等与回滚）
- `lib.rs` 全量 `#[cfg(desktop)]` 隔离：托盘、激活策略、`single-instance`、`autostart`、窗口显隐、`CloseRequested` 拦截
- `capabilities/mobile.json`；`tauri.conf.json` 增 `bundle.iOS` / `bundle.android`；`Info.plist` 与 `AndroidManifest.xml` 基线
- 原生设备名 + 用户自定义显示名（替换 `app_state.rs:75-86` 在移动端的取值路径）
- `os_type` 上报与前端图标 / `types/index.ts` 联合类型扩展
- 设置页移动端形态（服务器地址 / 账号 / PSK / 设备名 / 限额只读展示 / TLS 三档 / E2EE 开关）
- 前台连接生命周期：`RunEvent::Pause` / `Resume` → `reconnect_notify`

**验收**：真机上配置服务器 → 上线 → 看见设备列表与服务端限额 → 切后台 5 分钟回前台自动重连；**桌面端老库迁移后 `device_id` / PSK / 历史条数完全不变**（回归用例覆盖 macOS / Windows / Linux 三端各一次）。

### M2 · 收发主链路（20 人日，P0）— 依赖 M1

- **发送**：文件（SAF / DocumentPicker）、剪贴板文本与图片（iOS 16+ 授权态、Android 12+ 隐私提示）、限额预检复用、E2EE 协商与回落提示复用
- **接收**：数据面滑窗、PathGuard 落地、SHA-256 校验、历史入库、系统通知（Android 13+ `POST_NOTIFICATIONS`）
- **接收策略**（修 §1.3-A）：`receive_policy` 三档 + 网络类型判定
- **落地与装载**（§3.1-1 全套）：iOS Documents + Files App 可见 + 分享 / 保存到… / 存相册；Android 外部私有目录 + MediaStore 开关 + `ClipData` 尽力而为 + 分享
- **`CLIPBOARD_INJECTED` 回执补齐**（修 §1.3-B）
- 历史面板：另存为、打开、分享、删除；归属闸门保留
- 移动端窗口与块长参数化（4 MB × 4 可下调），低内存自动降级

**验收**：§7.2 五端互操作矩阵全通；E2EE 开 / 关两态均通且回落提示可见；超限文案与桌面端一致；蜂窝网下按策略不静默拉取。

### M2.5 · 断点续传（12 人日客户端 + 2 人日服务端，**P1，需 §11-2 拍板**）

见 §4.4。**验收**：传输中途杀进程 / 断网，重连后仅传缺失块且整文件 SHA-256 通过；「不得复用 `session_id` 传不同内容」转成测试负例。

### M3 · Android 常驻与系统集成（15 人日，P0 / P1）— 可与 M4 并行

| 任务 | 优先级 |
| :--- | :--- |
| 前台服务 + 常驻通知（Rust core 同进程保活） | P0 |
| Android 15 `dataSync` 6 h 限制应对（`specialUse` 或到期重调度 + 用户可见说明） | P0 |
| 电池优化 + 厂商自启白名单引导页（小米 / 华为 / OPPO / vivo） | P0 |
| 开机自启 → 映射 `cmd_*_autostart` | P1 |
| 前台剪贴板监听 + 密码管理器隐私规避清单 | P1 |
| 系统 Share 接收入口（`ACTION_SEND` / `ACTION_SEND_MULTIPLE`）→ 选设备发送 | P1 |
| 桌面小组件 / 快捷方式 | P2 |

**验收**：灭屏 30 分钟可收；4 款主流 ROM 存活率报告；Android 15 设备上限时行为符合预期且有用户可见说明。

### M4 · iOS 常驻与系统集成（18 人日客户端 + 12 人日服务端，P1）

- APNs 推送 + 离线暂存（服务端 S2 / S3）
- 可见推送点击 → 领取会话 → 建立数据面拉取；静默推送仅作加速
- **Share Extension**（独立 Xcode target）：从「照片」「文件」等任意 App 分享到 UniDrop → 选设备发送；需 App Group 共享 settings 与 SQLite；扩展内嵌 Rust `staticlib`（或最小 Swift 发送器，视扩展内存上限实测结果而定，见 R12）
- Files App 集成：`UIFileSharingEnabled` + `LSSupportsOpeningDocumentsInPlace`；收件目录 `isExcludedFromBackup`
- 存相册（`NSPhotoLibraryAddUsageDescription`）
- 前台剪贴板两段式读取（detect 预判 + 手势触发实读）
- App Intents 快捷指令「发送到 <设备>」（P2）

**验收**：App 被系统杀死后，发送端发起传输 → 手机收到推送 → 点击 → 文件完整落地（SHA-256 校验通过）；从「照片」与「文件」两个 App 经 Share Extension 发送成功。

### M5 · UI 移动化与体验（15 人日，P0，**可与 M2–M4 全程并行**）

底部 Tab 壳 · 安全区 / 触控 / 长按 / 下拉刷新 / 空态 · 键盘遮挡 · 移动端横幅体系（复用 TLS 六档分类与 `certFailureStatusText`，新增「后台受限」提示）· 深浅色（P2）· i18n 抽取（P1，当前中文硬编码在 Rust 与 TSX 两侧）· vitest 覆盖新组件。

### M6 · 测试、CI/CD、签名与上架（15 人日，P0）

见 §7、§8。含真机矩阵执行、CI 落地、两端签名与商店物料、灰度发布、文档交付。

---

## 7. 测试与验收标准

### 7.1 单元 / 集成测试

**复用（不改逻辑，应全绿）**：`cargo test` 已覆盖协议编解码、滑动窗口、E2EE 的 nonce 唯一性与密钥域分隔、PathGuard、`retention.rs` 的 u32 回绕边界（`:56-70`）、历史归属闸门的**接线守卫**（`history_cmd.rs:145-175`，源码级断言）。前端 `App.test.tsx`、`HistoryPanel.test.tsx`、`SettingsModal.test.tsx` 同样复用。

**新增编译门禁**：`cargo check --target {aarch64-apple-ios, aarch64-apple-ios-sim, aarch64-linux-android, x86_64-linux-android}` 进 CI。这是防「桌面 `cfg` 漏改导致移动端编译不过」最便宜的手段，且能在 PR 阶段而非发版阶段暴露。

**新增守卫测试**（照抄仓库既有风格——用源码级断言钉住「接线」，而不是只测逻辑）：

| 守卫 | 钉住什么 |
| :--- | :--- |
| `mobile_version_passes_the_e2ee_gate` | 移动端上报的 `app_version` 必须过 `peer_supports_e2ee`，且**必须是三段式**——`parse_semver`（`core/e2ee.rs:306-317`）明确拒绝四段及以上，Android `versionCode` 绝不可混入 `app_version`。与既有 `our_own_version_passes_the_gate`（`e2ee.rs:474-482`）同款 |
| `mobile_does_not_register_desktop_only_plugins` | `single-instance` / `autostart` 的注册必须在 `#[cfg(desktop)]` 内 |
| `database_is_not_inside_cache_dir` | 三类路径互不相同，且 DB 不在缓存目录下（钉住 §1.3-E 不复发） |
| `receive_policy_gates_auto_accept` | `lib.rs` 的自动接受分支必须经过策略判定（钉住 §1.3-A 的修复不被回退） |

### 7.2 五端互操作矩阵（对齐并扩展 DESIGN §9.2）

必测 12 组，每组覆盖 5 个用例：

| 组合 | 用例 |
| :--- | :--- |
| iOS ↔ macOS<br>iOS ↔ Windows<br>iOS ↔ Android<br>Android ↔ macOS<br>Android ↔ Windows<br>Android ↔ Linux<br>（每对双向各测一次） | ① 文件：多文件 + 含中文名 / 空格 / emoji / 长路径<br>② 文本：普通文本 + 超 4 MB 触发限额<br>③ 图片：PNG（含 > 64 MB 触发限额）<br>④ E2EE 开：加密传输 + 元数据不可见（服务端只见密文与明文限额字段）<br>⑤ E2EE 关 / 对端过旧：明文回落且提示可见 |

**协议一致性对照**：可复用仓库自带的 `server/cmd/diag-sender` 与 `server/cmd/diag-receiver` 作为参考实现，验证移动端产出的帧与桌面端逐字节一致。

### 7.3 真机与 ROM 矩阵

| 平台 | 设备 / 版本 | 重点 |
| :--- | :--- | :--- |
| iOS | iPhone × iOS 17 / 18 / 当期最新正式版（与仓库注释中的 macOS 26 同代） | 剪贴板授权横幅、Files App 可见性、后台挂起后重连、推送点击领取 |
| iOS（P2） | iPad | 布局与多任务分屏 |
| Android 原生 | Pixel × API 24（最低支持）/ 33 / 34 / 35 | `POST_NOTIFICATIONS` 运行时权限、前台服务类型、Android 15 的 6 h 限制 |
| Android 国产 ROM | 小米 HyperOS、华为 EMUI / HarmonyOS、OPPO ColorOS、vivo OriginOS 各 1 | **存活率与自启引导是重点**，需出实测报告而非承诺 |

### 7.4 弱网与中断测试

- 工具：Network Link Conditioner（iOS）/ Android Emulator network profile / 服务端侧限流。
- 场景：3G、丢包 5%、RTT 300 ms 下验证滑窗重传与动态 RTO；传输中切飞行模式 / 切后台 / 杀进程 / Wi-Fi↔蜂窝切换的行为矩阵。
- 每个场景必须断言三件事：**不静默丢数据**、**历史状态落终态**（不残留 `TRANSFERRING` 死行，`HistoryRepo::reset_stale_in_flight` 生效）、**用户看得见原因**（对齐「失败卡片永不自动消失」这条既有设计，`settings_cmd.rs:31-37`）。

### 7.5 安全复核清单（移动端新增攻击面）

- Android `FileProvider`：`grantUriPermissions` 范围最小化，`exported = false`，`paths.xml` 不放开根目录。
- Share Extension / App Group 容器内的 **PSK 存储**：iOS 建议 Keychain + App Group，而非明文 SQLite（桌面端现状是明文 SQLite，`docs/需求.md` 第 7 条已记录该取舍；移动端设备失窃概率更高，建议收紧）。
- push token 与 `account_id` 的绑定校验，防止跨账号投递。
- 离线暂存的服务端加密与 TTL 物理擦除（PRD §4.2.3 要求）。
- **手机丢失场景**：现模型是「一把全局 PSK 填到所有设备」，`docs/需求.md` 第 6、7 条已如实记录「账号是便利分区而非安全边界」「无前向保密、设备间不隔离」。移动端上线会**显著放大**这个既有风险 → 建议把 DESIGN §6.1 的设备级密钥（`paired_devices` schema 已在文档中，且 `storage/db.rs` 的迁移注释说明了它为何被 DROP）提上日程，或至少提供服务端「踢除设备」能力。见 §11-9。

### 7.6 验收标准（Definition of Done）

1. §7.2 矩阵 12 组 × 5 用例全通，且 E2EE 开启时服务端抓包看不到文件名 / sha256 / 预览摘要。
2. §7.3 真机矩阵全通；国产 ROM 存活率有实测数据（不要求 100%，但要求「有数」）。
3. §7.1 四条守卫测试 + 四个 target 的 `cargo check` 进 CI 且绿。
4. 桌面端三平台回归通过：老库迁移后 `device_id` / PSK / 账号 / 历史条数不变，托盘、菜单栏探测、通知、自启、单实例行为无变化。
5. 移动端常驻内存与桌面端同量级（PRD 承诺主进程 20–30 MB；移动端含 WebView 需另立基线并在 M0-T0.7 给出实测值）。
6. 文档齐备（§8 文档行）。
7. 双审留痕齐备：`.reviews/mobile-client/{plan,code}/` 逐条裁决完成。

---

## 8. CI/CD、签名与上架

| 项 | 内容 |
| :--- | :--- |
| **新增 workflow** | `mobile-ci.yml`：4 个 target 的 `cargo check` + `pnpm build` + `vitest run` + `cargo test`；`build-mobile.yml`：`macos-latest` 出 `.ipa`，`ubuntu-latest` 出 `.aab` / `.apk` |
| **现有 workflow 影响** | `.github/workflows/client-ci.yml` 与 `build-release.yml` 的 `paths: client/**` 触发条件天然覆盖新增文件，无需改；但 `build-release.yml` 的桌面矩阵不受影响，移动端走独立 workflow，避免互相拖慢 |
| **凭据管理** | Apple API Key：**注意 `client/.env` 里已有真实 Key ID / Issuer 与本地 `.p8` 路径**，该文件已被 `.gitignore` 排除；移动端 CI 必须走 GitHub Secrets，**不得把这份内容复制进仓库或 workflow 明文**。Android keystore 以 base64 进 Secret；Play Service Account JSON 同理 |
| **版本号策略** | 现有三处（`Cargo.toml` / `tauri.conf.json` / `package.json`）+ Android `versionCode`（自增）+ iOS `CFBundleVersion`。**遵守「不自动修改版本号字段」的既有偏好**：所有版本变更走独立 `chore(release)` commit，不由脚本代改。`app_version` 必须保持三段式（见 §7.1 守卫） |
| **App Store 合规** | `PrivacyInfo.xcprivacy` 隐私清单（含剪贴板、设备名、推送标识）、剪贴板用途说明、`UIBackgroundModes` 用途说明、出口合规（`ITSAppUsesNonExemptEncryption`）。`insecure` TLS 档在 iOS 构建中建议隐藏或加强警示（审核与用户风险双向考虑） |
| **Google Play 合规** | 数据安全表单（传输中加密 ✓、是否收集数据）、前台服务类型用途声明、`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` 受政策限制 → **建议改为引导用户手动设置而非申请该权限**、targetSdk 按当期政策确认（每年 8/31 收紧） |
| **灰度路径** | TestFlight 内测 → App Store 分阶段发布；Play Internal Testing → 封闭测试 → 分阶段发布；国内 Android 视渠道决策（§11-3） |
| **文档交付** | 新增 `docs/INSTALL_MOBILE.md`；`docs/USER_GUIDE.md` 增移动章节；`docs/design/DESIGN.md` 增移动附录（建议 §5.7）；`docs/design/REQUIREMENTS.md` §7 兼容性矩阵补 iOS / Android；`README.md` 平台矩阵与目录结构更新 |
| **流程** | 本计划经 `/dual-review-plan`；编码阶段按主题走 `/dual-run` 或 `/dual-review-code`，留痕落 `.reviews/mobile-client/`。审查意见须逐条裁决（接受 / 驳回 / 暂缓 + 理由） |

---

## 9. 风险登记册

| # | 风险 | 概率 | 影响 | 缓解 |
| :--- | :--- | :--- | :--- | :--- |
| **R1** | iOS 无法后台常驻，用户「像桌面一样随时能收」的预期落空 | 高 | 高 | M4 的 APNs + 离线暂存；在此之前 UI 明示「App 关闭时接收会延迟到下次打开」；决策权交给 §11-1 |
| **R2** | 国产 ROM 杀后台，前台服务也保不住 | 高 | 中 | 白名单引导页 + 4 款 ROM 实测报告 + 推送兜底；**不承诺 100%**，把实测数据写进用户文档 |
| **R3** | Android 15 `dataSync` 6 h/24 h 限制 | 中 | 中 | `specialUse` 类型 + `AlarmManager` 重调度 + 用户可见说明 |
| **R4** | Android SAF 的 `content://` 与 Rust `File::open` 不兼容 | 高 | 中 | **M0-T0.4 前置决策**：fd 传递（首选）/ 复制兜底 |
| **R5** | `tauri-plugin-dialog` 移动端能力不足（无文件夹选择器、`blocking_*` 不可用） | 中 | 中 | 自研 `unidrop-mobile` 插件接管全部文件选择 |
| **R6** | WKWebView / Android WebView 渲染与 IPC 差异 | 中 | 中 | M0-T0.7 前置实测；必要时移动端单独组件分支 |
| **R7** | App Store 审核（剪贴板、后台模式、加密、`insecure` 档） | 中 | 高 | 先 TestFlight；隐私清单与用途说明提前准备；`insecure` 档在 iOS 构建中隐藏或强警示 |
| **R8** | 版本号形态导致 E2EE **静默回落明文** | 低 | 高 | 三段式硬约束 + §7.1 守卫测试。这是「安全功能静默失效」类缺陷，仓库已有同类前车之鉴（`e2ee.rs:298-305` 记录了 `0.3.0-beta.1` 被误判为不支持） |
| **R9** | **DB 在缓存目录被系统清理**（移动端必现，桌面端偶发） | 高 | 高 | M1 路径三分 + 一次性迁移 + 回归用例（§4.2） |
| **R10** | 移动端内存压力（4 MB × 4 窗口 + WebView） | 中 | 中 | 窗口 / 块长参数化，低内存自动降级；M0-T0.7 实测 RSS 基线 |
| **R11** | `rusqlite(bundled)` / `ring` 交叉编译失败 | 低 | 中 | M0-T0.2 前置；备选 `libsqlite3-sys` 系统库 / 换 crypto provider（会引入第二份 provider，与 `Cargo.toml` 中「显式声明 ring 是为了避免依赖树出现两份」的既有约束冲突，需评估） |
| **R12** | Share Extension 内存上限导致大文件发送失败 | 中 | 中 | 扩展内限制单文件大小并引导回主 App；或扩展只做「选设备 + 交回主 App」 |
| **R13** | 手机丢失 + 单一全局 PSK | 中 | 高 | §7.5：设备级密钥（DESIGN §6.1）或服务端远程踢除 |
| **R14** | 移动端与桌面端并行开发造成 `core/` 回归 | 中 | 高 | `core/` 由单一 owner 负责；移动端改动一律走「新增 `platform/*_mobile.rs` + `cfg` 分支」，**不改既有桌面实现**；四个 target 的 `cargo check` 进 CI |

---

## 10. 工时汇总与人力建议

| 阶段 | 客户端人日 | 服务端人日 | 优先级 | 依赖 |
| :--- | ---: | ---: | :--- | :--- |
| M0 可行性 Spike | 10 | 0 | P0 | — |
| M1 移动端基座 | 15 | 0 | P0 | M0 |
| M2 收发主链路 | 20 | 0 | P0 | M1 |
| M2.5 断点续传 | 12 | 2 | P1 | M2 + S4 |
| M3 Android 常驻 | 15 | 0 | P0 / P1 | M2 |
| M4 iOS 常驻与集成 | 18 | 12 | P1 | M2 + S2 / S3 |
| M5 UI 移动化 | 15 | 0 | P0 | 可与 M2–M4 并行 |
| M6 测试 / CI / 上架 | 15 | 0 | P0 | M2–M5 |
| **合计** | **120**（不做 M2.5 则 **108**） | **14**（不做 S4 则 **12**） | | |

**人力配置建议**：
- Rust core owner ×1 —— 掌握协议 / E2EE / 滑窗，负责 M0、M1、M2、M2.5，并对 `core/` 目录负全责（R14 的主要缓解手段）
- Android ×1 —— M3 + 插件 Kotlin 侧
- iOS ×1 —— M4 + 插件 Swift 侧 + Share Extension
- 兼职 UI / QA —— M5、M6

**3 人并行后约 14–18 周**；单人则约 24–27 周。
关键路径：`M0 → M1 → M2 → (M2.5 ∥ M3 ∥ M4) → M6`；M5 全程并行。
区间上限来自两处不可压缩的串行等待：M0 的 go/no-go 闸门，以及 M6 里商店审核与真机矩阵占用的**日历时间**（不是人日）。

---

## 11. 待决策项

| # | 决策 | 影响 | 建议 |
| :--- | :--- | :--- | :--- |
| **1** | iOS v1 范围：接受「App 未打开就收不到」，还是首期纳入 APNs + 离线暂存？ | ±12 人日 + 服务端首次功能性改动 | 首期只做「前台 + 可见推送提示」，S2 / S3 排入 v1.1；避免首期就被服务端改动拖住 |
| **2** | 断点续传是否首期做？ | ±14 人日（含服务端 S4） | **建议做**。移动网络中断是常态，不做则「传一半切后台」等于必失败 |
| **3** | Android 分发渠道：Google Play / 国内商店 / 直接 APK？ | 合规工作量、targetSdk、FGS 类型、电池优化权限策略 | 与本项目「自建服务器」定位一致的话，直接 APK + 国内商店优先，Play 作为可选 |
| **4** | iOS 分发：App Store 上架，还是先 TestFlight / 企业签？ | 审核周期与合规物料 | 先 TestFlight 验证，再决定上架 |
| **5** | **核心语义替代**（§3.1-1）：是否接受「iOS 用 Files App 可见目录 + 分享 / 保存，Android 用 MediaStore + 尽力而为的 `ClipData`」作为「文件级剪贴板注入」的等价实现？ | 全计划唯一的产品语义降级点 | 接受。iOS 那条路径的用户体验实际上不差于桌面（文件 App 里可直接粘贴到任意目录） |
| **6** | 接收确认策略默认值：移动端「仅 Wi-Fi 自动接收，蜂窝询问」，桌面端是否同步收紧？ | 桌面端行为变更 | 移动端按建议默认；桌面端保持现状（避免给既有用户制造意外），仅新增该开关 |
| **7** | 版本号：移动端与桌面端同号同步发布，还是独立号？ | E2EE 门槛判定、发版节奏 | **同号同步**。最省事且天然满足 `peer_supports_e2ee` |
| **8** | i18n 是否首期抽取？ | ±5 人日；当前中文硬编码在 Rust 与 TSX 两侧 | 首期不做，M5 里预留 key 结构；出海前再抽 |
| **9** | 是否借移动端上线一并做设备级密钥（DESIGN §6.1）？ | 安全模型升级，±20 人日 | 至少先做服务端「踢除设备」；完整 §6.1 单开主题 |
| **10** | 代码归属：复用同一 Tauri crate，还是新建 `client-mobile/`？ | CI 复杂度、线协议漂移风险 | **复用同一 crate**（§2.2 的核心论据） |

---

## 12. 下一步

1. 确认 §11 的 10 条决策（尤其 **1、2、5** 三条，它们决定总工期与范围）。
2. 本文档经 `/dual-review-plan` 双审，审查意见逐条裁决后落 `.reviews/mobile-client/plan/`。
3. 双审通过后开 `mobile-client` 主题，先做 **M0（10 人日 spike）**。
4. **M0 报告是 go/no-go 闸门**：R4 / R6 / R11 三条任一证伪，路线需回到 §2.1 重评。
5. M0 通过后再排 M1–M6 的详细迭代计划与人力到位时间。

---

## 附录 A · 本文档引用的代码位置索引

| 主题 | 位置 |
| :--- | :--- |
| IPC 命令注册表（21 条） | `client/src-tauri/src/lib.rs:965-987` |
| 应用入口（缺 mobile entry point） | `client/src-tauri/src/lib.rs:304`、`client/src-tauri/src/main.rs` |
| 桌面插件注册（single-instance / autostart） | `client/src-tauri/src/lib.rs:437-455` |
| 接收端无条件自动接受 | `client/src-tauri/src/lib.rs:568-578` |
| 拒绝通路（`reject_reason`） | `client/src-tauri/src/lib.rs:545-556` |
| 剪贴板监听仅 macOS 接线 | `client/src-tauri/src/lib.rs:914` |
| 窗口关闭拦截 | `client/src-tauri/src/lib.rs:959-963` |
| 平台剪贴板分派与「Unsupported platform」兜底 | `client/src-tauri/src/platform/mod.rs:31-115` |
| 缓存目录解析（移动端落 `temp_dir()`） | `client/src-tauri/src/core/cache_manager.rs:30-54` |
| DB 建在缓存目录 | `client/src-tauri/src/storage/db.rs:6-10` |
| 设置模型与 serde default 陷阱注释 | `client/src-tauri/src/commands/settings_cmd.rs:6-105` |
| 限额预检与目录跳过口径 | `client/src-tauri/src/commands/clipboard_cmd.rs:172-232` |
| E2EE 决策与回落提示 | `client/src-tauri/src/commands/clipboard_cmd.rs:305-356` |
| E2EE 版本门槛与三段式解析 | `client/src-tauri/src/core/e2ee.rs:283-317` |
| 自有版本必须过门槛（守卫测试范式） | `client/src-tauri/src/core/e2ee.rs:474-482` |
| nonce 构造与「续传不得复用 session」约束 | `client/src-tauri/src/core/e2ee.rs:110-130` |
| 保留策略唯一事实源与 u32 回绕注释 | `client/src-tauri/src/core/retention.rs:1-70` |
| 归属闸门与其接线守卫测试 | `client/src-tauri/src/commands/history_cmd.rs:29-38`、`:145-175` |
| `blocking_pick_folder`（移动端不可用） | `client/src-tauri/src/commands/history_cmd.rs:56` |
| `cmd_reveal_session` 的桌面三连 | `client/src-tauri/src/commands/history_cmd.rs:111-140` |
| 通知三态授权查询 | `client/src-tauri/src/platform/notification.rs:14-29` |
| 设备名解析 | `client/src-tauri/src/app_state.rs:75-86` |
| 启动形态真值表（桌面专属） | `client/src-tauri/src/core/startup.rs` |
| 前端事件接线与两处闭包陷阱 | `client/src/App.tsx:96-115`、`:211-333` |
| 证书失败分档文案 | `client/src/types/index.ts:100-158` |
| 设备图标（缺 ios / android） | `client/src/components/DeviceList.tsx:16-18` |
| 服务端限额默认值与错误码 | `server/internal/limits/limits.go:22-60` |
| 服务端 `os_type` 仅存储与日志 | `server/internal/registry/session.go:18`、`server/internal/controller/control_ws.go:138`、`:166` |
| `resumed_items` 协议位（两端均已声明） | `client/src-tauri/src/protocol/envelope.rs:198`、`server/internal/protocol/envelope.go:159` |

## 附录 B · 移动端权限与清单基线

**Android（`AndroidManifest.xml`）**
`INTERNET`、`ACCESS_NETWORK_STATE`、`FOREGROUND_SERVICE`、`FOREGROUND_SERVICE_DATA_SYNC`（或 `_SPECIAL_USE`）、`POST_NOTIFICATIONS`（API 33+）、`RECEIVE_BOOT_COMPLETED`、`WAKE_LOCK`、`VIBRATE`、`READ_MEDIA_IMAGES` / `READ_MEDIA_VISUAL_USER_SELECTED`（API 33+）或 `READ_EXTERNAL_STORAGE`（≤32）。
**刻意不申请**：`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`（Play 政策受限，改为引导用户手动设置）。
另需：`FileProvider` 配置、`network_security_config.xml`（视 M0-T0.6 结论）。

**iOS（`Info.plist` / entitlements）**
`UIBackgroundModes`（`fetch`、`remote-notification`、`processing`）、`UIFileSharingEnabled`、`LSSupportsOpeningDocumentsInPlace`、`NSPhotoLibraryAddUsageDescription`、`ITSAppUsesNonExemptEncryption`、`LSApplicationCategoryType`、App Group entitlement（Share Extension 共享容器所需）、`CFBundleURLTypes`（扩展回跳，视方案而定）。
另需：`PrivacyInfo.xcprivacy` 隐私清单。

---

*本文档为计划产出，未修改任何既有代码。落地前须经双审（`/dual-review-plan`）。*
