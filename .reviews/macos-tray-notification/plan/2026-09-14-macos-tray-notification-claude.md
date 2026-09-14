# macOS 菜单栏图标与系统通知修复方案

- 主题：`macos-tray-notification`
- 角色：Driver（claude）
- 日期：2026-09-14
- 阶段：plan（命令 A · 全流程 S1）

## 1. 背景与根因

用户报告 macOS 端两个功能「都没生效」：菜单栏看不到图标、收到消息不弹提醒。

排查后确认：**两个功能的代码都存在，也都被执行到了**，问题出在两个 macOS 平台细节上。
以下根因均有证据，不是推测。

### 1.1 托盘图标：创建成功，但视觉上不可见

运行日志 `~/Library/Logs/com.unidrop.client/UniDrop.log` 首屏即有：

```
[01:57:44][unidrop_client_lib][INFO] Starting minimized to tray per user setting
```

该行位于 `lib.rs:742`，在托盘构建块（`lib.rs:684-735`）**之后**，证明 setup 全程无异常、
托盘对象已建立。另外排除了两个常见嫌疑：

- `app.default_window_icon()` 在 macOS 上确实返回 `Some`。`tauri-codegen` 的
  `context.rs:211-243` 对非 Windows 目标统一取 `bundle.icon` 里第一个 `.png`，
  即本项目的 `icons/32x32.png`，不是 `None`；
- `let _tray` 提前 drop 不会摘掉图标。`TrayIcon::register()`（`tauri-2.11.5/src/tray/mod.rs:446`）
  会 `resources_table().add(self.clone())`，应用侧持有一份引用。

真正的原因在 `lib.rs:696-698`：托盘图标直接复用了应用图标 `icons/32x32.png`——
一张深蓝黑色主体、青色描边的**彩色**图标。macOS 菜单栏要求**模板图标**
（纯黑 + alpha，系统按深浅色自动反色）。深色主体落在深色模式菜单栏上，
与背景几乎融为一体，因而「看不见」。

附带一处从未生效的逻辑：`TrayIconBuilder::show_menu_on_left_click` 默认为 `true`
（`tauri-2.11.5/src/tray/mod.rs:316` 文档明载），所以 `lib.rs:715-734` 中
「左键切换窗口显隐」的分支在 macOS 上被系统弹菜单抢先，从未被触发。

### 1.2 系统通知：调用发生了，但底层 API 已被系统废弃

同一份日志显示接收链路完整跑通（`TRANSFER_OFFER` → 下载 → SHA-256 校验通过 →
`TRANSFER_COMPLETE`），必然执行到 `transfer_engine.rs:1014` 的通知调用。

根因在依赖链末端：

```
tauri-plugin-notification 2.4.0
  └─ notify-rust 4.18.0
       └─ mac-notification-sys 0.6.15
            └─ NSUserNotificationCenter   (objc/notify.m:79)
```

`NSUserNotification` / `NSUserNotificationCenter` 自 macOS 11 起被 Apple 废弃，
本机为 **macOS 26.6.2**，该 API 已不再投递任何通知。

失败完全静默，所以此前零信号——错误被三层吞掉：

1. ObjC 层不报错；
2. 插件内 `tauri::async_runtime::spawn(async move { let _ = notification.show(); })`
   （`tauri-plugin-notification-2.4.0/src/desktop.rs:215-217`）；
3. 调用处 `let _ = show_transfer_notification(...)`（`transfer_engine.rs:700/987/1004/1014`）。

需要澄清一个容易走错的方向：`request_permission()` / `permission_state()` 在桌面端是
**硬编码返回 `Granted`** 的空实现（`desktop.rs:61-67`），补权限请求对本问题无效。

## 2. 目标与非目标

### 目标

1. 托盘图标在 macOS 深色/浅色模式下均清晰可辨；
2. 收到文本、图片、文件时弹出真实的系统通知；
3. 点击通知可唤起主窗口；
4. 通知失败不再静默——留下可排查的日志。

### 非目标（本次不做）

- 不改 Windows / Linux 的通知与托盘行为（继续走 `tauri-plugin-notification`）；
- 不改传输、E2EE、历史等业务逻辑；
- 不引入通知的 action 按钮、分组、badge 计数；
- 不修 `tauri-plugin-log` 疑似重复注册 target 导致日志每条打印两遍的问题
  （已发现，与本主题无关，另立主题）。

## 3. 方案设计

### 3.1 托盘模板图标

**尺寸依据（重要）**：`tray-icon` 的 macOS 实现把图标高度**硬编码缩放到 18.0 pt**，
宽度按原始宽高比换算（`tray-icon/src/platform_impl/macos/mod.rs:295-296`）：

```rust
let icon_height: f64 = 18.0;
let icon_width: f64 = (width as f64) / (height as f64 / icon_height);
```

因此源图必须是**正方形**（否则宽度异常），且为 Retina 清晰度应提供 18pt @2x =
**36×36 px**。同一函数末尾 `nsimage.setTemplate(icon_is_template)` 证实
`icon_as_template(true)` 会真实生效。

**新增资源**：`client/src-tauri/icons/tray-macos.png`，36×36 px，RGBA，
颜色通道全 0（纯黑），形状由 alpha 通道表达，图形取剪贴板轮廓以延续品牌识别。

**生成方式**：新增 `client/scripts/gen-tray-icon.py`，用 python3 标准库
（`zlib` + `struct`）直接写 PNG，不引入 Pillow 等第三方依赖。脚本与产物一并提交，
便于日后调整形状后复现。

**代码改动**（`lib.rs:693-698`）：macOS 走内嵌的模板图标并开启 template 模式，
其余平台维持现状。

```rust
let mut tray_builder = TrayIconBuilder::new()
    .menu(&tray_menu)
    .tooltip("瞬贴 (UniDrop) - 跨平台剪贴板与文件分发");

#[cfg(target_os = "macos")]
{
    // 菜单栏图标必须是模板图（纯黑 + alpha），由系统按深浅色自动反色；
    // 直接用彩色应用图标会在深色菜单栏上与背景融为一体（本次修复的原症状）。
    let tray_icon = tauri::image::Image::from_bytes(
        include_bytes!("../icons/tray-macos.png"),
    )?;
    tray_builder = tray_builder
        .icon(tray_icon)
        .icon_as_template(true)
        // 默认 true 会让左键弹菜单，把下面 on_tray_icon_event 的左键分支挡死。
        .show_menu_on_left_click(false);
}
#[cfg(not(target_os = "macos"))]
if let Some(icon) = app.default_window_icon() {
    tray_builder = tray_builder.icon(icon.clone());
}
```

关闭 `show_menu_on_left_click` 后，macOS 上左键切换显隐、右键弹菜单，
与 Windows 行为一致，也让既有的 `on_tray_icon_event` 分支真正生效。

### 3.2 系统通知改用 UNUserNotificationCenter

`platform/notification.rs` 改为平台分发，与现有 `platform/clipboard_*.rs` 的组织方式一致：

- macOS → 新增 `platform/notification_macos.rs`，走 `UNUserNotificationCenter`；
- 其他平台 → 保留现有 `tauri-plugin-notification` 实现，零改动。

#### 依赖选型与 objc2 代际问题（关键决策）

`objc2-user-notifications` 最新版 0.3.2 要求 `objc2 >=0.6.2, <0.8.0`，
而本项目现用 `objc2 0.5` / `objc2-foundation 0.2` / `objc2-app-kit 0.2`。

查证依赖树后确认：**两代 objc2 在 macOS 构建里本就并存，是 Tauri 带来的既成事实**，
不是本次引入的新负担：

```
objc2 v0.6.4  ← block2 0.6.2 ← dispatch2 0.3.1 ← tao 0.35.3 ← tauri-runtime-wry ← tauri 2.11.5
objc2 v0.5.2  ← objc2-app-kit 0.2.2 / objc2-foundation 0.2.2 ← unidrop-client（仅本项目自用）
```

另需说明：`objc2-user-notifications 0.3.2` 虽已出现在 `Cargo.lock`，但它是
**iOS 专用**的 `objc2-ui-kit` 传递引入的，在 `aarch64-apple-darwin` 上不参与编译
（`cargo tree -i objc2-user-notifications --target aarch64-apple-darwin` 返回
"nothing to print"）。因此本次要显式声明它，会新增实际编译单元，
但**不新增 objc2 代际**——0.6.4 那一代已在 macOS 构建中。

**选定路线**：新模块用 objc2 0.6 生态，现有剪贴板代码保持 0.5 不动。

理由：两代不交换任何类型，共存安全；改动面收敛在新文件内，
不触碰已稳定工作的剪贴板读写与监听（`clipboard_macos.rs` 126 行 +
`listener_macos.rs` 52 行），避免核心功能回归。

被否决的替代方案：将整个 macOS 模块升级到 objc2 0.6。虽然能让本 crate 只用一代，
但需改写剪贴板全部 FFI（`Id` → `Retained`、`msg_send_id!` → `msg_send!`、
`declare_class!` → `define_class!`），为本次需求之外的代码引入回归风险，
收益与风险不成比例。若审查认为长期应统一代际，建议另立主题专门做迁移。

`Cargo.toml` 的 macOS 依赖段新增（沿用本仓库对 `webpki-roots` / `ring`
显式声明并写明理由的既有风格）：

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.5"
objc2-foundation = { version = "0.2", features = ["NSString", "NSArray", "NSURL", "NSData"] }
objc2-app-kit = { version = "0.2", features = ["NSPasteboard", "NSPasteboardItem"] }

# 以下三项是 UNUserNotificationCenter 所需，属 objc2 0.6 那一代。
# 与上面的 0.5 并存是刻意的：0.6.4 早已由 tao/wry 带进 macOS 构建，
# 两代不交换类型；把剪贴板一并升到 0.6 会为本次需求之外的代码引入回归风险。
objc2-v06 = { package = "objc2", version = "0.6" }
objc2-foundation-v06 = { package = "objc2-foundation", version = "0.3", features = ["NSString", "NSError"] }
objc2-user-notifications = { version = "0.3", features = [
    "UNUserNotificationCenter", "UNNotificationContent",
    "UNNotificationRequest", "UNNotificationResponse",
    "UNNotificationTrigger", "UNNotificationSound", "UNError", "block2",
] }
block2 = "0.6"
```

> 注：确切的 feature 名以 `cargo info objc2-user-notifications` 输出为准，
> S5 编码时逐个核对，不足则补。

#### 非 bundle 环境必须防护（健壮性要点）

`UNUserNotificationCenter.currentNotificationCenter()` 要求进程有 bundle
identifier；裸二进制（`tauri dev` 直接跑 `target/debug/unidrop-client`）下调用会抛
`NSInternalInconsistencyException` 导致崩溃。

因此**入口先检查** `NSBundle::mainBundle().bundleIdentifier()`：为 `nil` 时不调用
通知中心，记 `log::warn!` 后返回 `Ok(())`——开发态没有通知可以接受，崩溃不行。

#### 授权

与桌面端插件的空实现不同，`UNUserNotificationCenter` 的授权是真实的。
在 `lib.rs` setup 阶段（托盘构建附近）调用一次
`requestAuthorizationWithOptions:completionHandler:`，options 取 `.Alert | .Sound`
（不要 badge，本应用无角标需求）。回调只记日志，不阻塞启动。

用户若拒绝，后续 `add` 会静默丢弃——这由日志体现，不再是无声失败。

#### 发送

```
UNMutableNotificationContent { title, body, sound: default }
  → UNNotificationRequest::requestWithIdentifier(uuid, content, trigger: nil)
  → center.addNotificationRequest_withCompletionHandler(req, handler)
```

`trigger: nil` 表示立即投递。identifier 用 `uuid::Uuid::new_v4()`（项目已依赖 `uuid`），
避免同 id 覆盖。completionHandler 里把 `NSError` 转成 `log::warn!`。

#### 点击跳转与 delegate

用 objc2 0.6 的 `define_class!` 定义一个实现 `UNUserNotificationCenterDelegate`
的类，实现两个方法：

- `userNotificationCenter:willPresentNotification:withCompletionHandler:`
  → 回传 `.Banner | .Sound`，使应用在前台时通知同样显示
  （否则 macOS 默认前台不弹，会被误判为「又不工作了」）；
- `userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:`
  → 唤起主窗口。

delegate 必须在 `requestAuthorization` 之前设置，并**长期持有**
（`static OnceLock<Retained<...>>`），否则被释放后回调丢失。

唤起窗口复用既有的 `reveal_main_window()`（`lib.rs:32`），它已处理
「先 unminimize 再 show + set_focus」的顺序问题。`AppHandle` 通过
`static OnceLock<AppHandle>` 在 setup 时存入供 delegate 取用。

**线程安全**：两个回调均由系统在主线程派发，而 `reveal_main_window` 操作窗口也要求
主线程，故可直接调用；为稳妥仍走 `app_handle.run_on_main_thread(...)`，
该调用在已处于主线程时是安全的。

### 3.3 通知失败不再静默

`show_transfer_notification` 保持返回 `Result<(), String>`，
把 `transfer_engine.rs` 四处调用点的 `let _ = ...` 改为：

```rust
if let Err(e) = show_transfer_notification(&app_handle, "…", "…") {
    log::warn!("Failed to show notification: {}", e);
}
```

理由与本仓库既有规矩一致——`lib.rs:652-654` 对后台清理失败就写明
「Err 必须留痕：若持续失败而外部零信号，故障呈现形态恰是这个功能本身要防的那件事」。
本次 bug 正是这条规矩缺位的直接后果。

## 4. 改动清单

| 文件 | 改动 |
|---|---|
| `client/src-tauri/icons/tray-macos.png` | 新增，36×36 RGBA 模板图标 |
| `client/scripts/gen-tray-icon.py` | 新增，无第三方依赖的图标生成脚本 |
| `client/src-tauri/Cargo.toml` | macOS 段新增 4 项依赖（含代际说明注释） |
| `client/src-tauri/src/lib.rs` | 托盘图标改模板图 + 关左键菜单；setup 中请求通知授权并装 delegate、存 AppHandle |
| `client/src-tauri/src/platform/notification.rs` | 改为平台分发 |
| `client/src-tauri/src/platform/notification_macos.rs` | 新增，UNUserNotificationCenter 实现 |
| `client/src-tauri/src/platform/mod.rs` | 注册新模块 |
| `client/src-tauri/src/core/transfer_engine.rs` | 4 处调用点补失败日志 |

`capabilities/default.json` 的 `notification:default` **保留不动**：
Windows/Linux 仍走插件，且前端未使用 JS 通知 API（已 grep 确认），移除无收益且扩大改动面。

## 5. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 非 bundle 环境调用通知中心崩溃 | 入口检查 `bundleIdentifier()`，为 nil 则跳过并记 warn |
| delegate 被释放导致点击无响应 | `static OnceLock` 长期持有 |
| 两代 objc2 共存引发混淆 | 两代不交换类型；Cargo.toml 写明理由；新代码只用 0.6 别名 |
| 模板图在浅色模式下过淡/过重 | 形状由 alpha 表达，实测后调整脚本参数重新生成 |
| 用户曾拒绝通知授权，改完仍不弹 | 日志明示授权状态；验证步骤含「系统设置中确认 UniDrop 通知已开启」 |
| `tauri dev` 下测不出通知 | 验证必须用打包后的 .app，已写入验证步骤 |

## 6. 验证方法

`.trivium/config.yaml` 声明的收口命令（S10 由 `TV verify` 真实执行）：

- build：`cd client && pnpm build`、`cd server && go build ./...`
- test：`cd server && go test ./...`

自动化之外，本主题必须补**人工验收**，因为两个症状都只在真实 macOS 桌面环境显现：

1. `cd client && pnpm tauri:build` 产出 .app 并安装到 `/Applications`
   （**不能用 `tauri dev` 验证通知**——裸二进制无 bundle，通知路径会被防护分支直接跳过）；
2. 启动后确认菜单栏出现清晰图标；在系统设置里切换深色/浅色模式，两种模式下均清晰可辨；
3. 左键点击图标 → 主窗口显隐切换；右键 → 弹出菜单（显示主窗口/偏好设置/退出）；
4. 从另一台设备分别发送**文本、图片、文件**三类内容，确认每类都弹出系统通知；
5. 应用窗口处于前台时再发一次，确认通知**依然显示**（验证 `willPresent` 分支）；
6. 点击通知横幅，确认主窗口被唤起并获得焦点；
7. 查看 `~/Library/Logs/com.unidrop.client/UniDrop.log`，确认无 `Failed to show notification`；
   若用户此前拒绝过授权，日志应能明确指出。

## 7. 待审查员重点确认

1. objc2 两代共存 vs 整体升级到 0.6——路线取舍是否成立；
2. `UNUserNotificationCenter` FFI 的授权请求、delegate 生命周期、completionHandler
   block 的所有权处理是否正确；
3. 点击回调唤起主窗口的线程安全处理是否充分；
4. 模板图标 36×36 的尺寸推导（基于 tray-icon 硬编码 18pt）是否正确；
5. 非 bundle 环境的防护是否覆盖了全部入口。
