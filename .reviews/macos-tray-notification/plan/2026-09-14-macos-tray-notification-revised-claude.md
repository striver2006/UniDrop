---
schema: trivium.disposition.v1
topic: macos-tray-notification
stage: plan
role: claude
kind: revised
run_id: 20260914T031414Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - AGY-04
  - GLM-01
  - GLM-02
  - GLM-03
  - GLM-04
---

# macOS 菜单栏图标与系统通知修复方案（修订版）

- 主题：`macos-tray-notification`
- 角色：Driver（claude）
- 日期：2026-09-14
- 阶段：plan 修订（命令 A · 全流程 S3）
- 被修订对象：`2026-09-14-macos-tray-notification-claude.md`
- 两侧结论：Antigravity `request-changes`（重要 2 ｜ 次要 2）；ZCode `approve-with-nits`（次要 3 ｜ 吹毛求疵 1）

## 裁决表

共 8 条，**接受 8 条、驳回 0 条、暂缓 0 条**（AGY-04 整体接受，但其中一条建议的技术前提依据 GLM-02 的证据未予采纳，详见理由列与 §1）。

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | major | 接受 | 三点逐一核实全部成立，按原计划编码必然编译失败：`Image::from_bytes` 受 `#[cfg(any(feature="image-ico", feature="image-png"))]` 门控（`tauri-2.11.5/src/image/mod.rs:74`），而 tauri 的 default features 为 `["wry","compression","common-controls-v6","dynamic-acl","x11","dbus"]`，不含 `image-png`；`objc2-user-notifications-0.3.2/Cargo.toml` 确有独立 `UNNotification` 项；`objc2-foundation-0.3.2/Cargo.toml` 确有独立 `NSBundle = []` 项。两个修法选项中采纳「补 image-png」而非「改用 Image::new 传裸 RGBA」——PNG 产物可目视校验与比对，与 GLM-04 要求的产物可复现目标一致，且 image-png 只打开 tauri 已内置的解码路径，不引入新 crate。 | §2.1 依赖清单：tauri 补 `image-png`；`objc2-user-notifications` 补 `UNNotification`；`objc2-foundation-v06` 补 `NSBundle` |
| AGY-02 | gemini | major | 接受 | 原方案的真实漏洞。我只在发送入口写了 bundle 防护，但 setup 阶段的 `setDelegate` 与 `requestAuthorizationWithOptions` 同样要先取 `currentNotificationCenter()`，裸二进制下会在启动期就抛 `NSInternalInconsistencyException`——即防护本身没覆盖最早触发点，开发态启动即崩。 | §2.2 新增 `init_notifications(app_handle)` 作为唯一初始化入口，入口处统一判据，不成立则整体跳过 delegate 注册与授权请求；发送入口保留同样判据，两处独立成立、不依赖调用顺序 |
| AGY-03 | gemini | minor | 接受 | 核实绑定已导出所需常量：`UNNotificationDefaultActionIdentifier` 与 `UNNotificationDismissActionIdentifier`（`generated/UNNotificationResponse.rs:11,16`），`response.actionIdentifier()` 返回 `Retained<NSString>`（同文件 :55）。不过滤会导致用户划走通知时也弹出主窗口，属于用户明确表达「不想处理」后反被打断。 | §2.4：仅当 `actionIdentifier == UNNotificationDefaultActionIdentifier` 才唤起窗口；所有分支均须调用 completionHandler |
| AGY-04 | gemini | minor | 接受 | 指出的两个问题均成立并采纳：`reveal_main_window`（`lib.rs:32`）无 `pub(crate)`，跨模块调用不到；completionHandler 类型为 `&block2::DynBlock<...>` 借用引用而非 `'static`，无法移入异步闭包，必须当前帧同步调用。但其中「delegate 已在主线程派发，故可直接同步调用 reveal_main_window」这一前提未予采纳——它与 GLM-02 冲突，而我核对 `extern_protocol!` 定义（`generated/UNUserNotificationCenter.rs:219-242`）确认该 trait 仅继承 `NSObjectProtocol`，无任何 `MainThreadOnly`/`MainThreadMarker` 约束，绑定层不提供主线程保证。两条意见实际不互斥：窗口操作闭包只捕获 `AppHandle`（`'static`）可安全派发，completionHandler 不进闭包即可。 | §2.5 `reveal_main_window` 提升 `pub(crate)`；§2.3 completionHandler 同步调用、窗口操作走 `run_on_main_thread` |
| GLM-01 | glm | minor | 接受 | 与 AGY-01 第 2 点同一问题，两侧独立命中，合并落点。一并采纳其附带结论：`UNMutableNotificationContent` 由 `UNNotificationContent` feature 覆盖（`generated/mod.rs:108-123`），不需单列，原清单无此冗余项。缺 `UNNotification` 会使 `willPresent` 回调在 trait 上根本不存在，「前台也显示通知」（验收步骤 5）无法实现。 | 同 AGY-01 落点 |
| GLM-02 | glm | minor | 接受 | 证据支持本条，见 AGY-04 裁决说明。原计划把 `run_on_main_thread` 写成「为稳妥仍走」的可选保险，措辞会误导 S5 实现或日后重构直接省略；而 `reveal_main_window` 内部的 `is_minimized`/`unminimize`/`show`/`set_focus` 属 NSWindow 系 API，非主线程触碰是未定义行为。 | §2.3 新增「线程模型」段，将主线程派发从「稳妥起见」升格为硬性要求并写明原因 |
| GLM-03 | glm | minor | 接受 | 判据 `bundleIdentifier() == nil` 覆盖不了最常见开发场景：终端跑 `tauri dev` 时裸二进制的 `mainBundle` 回落到宿主应用，`bundleIdentifier` 非空，防护分支不触发，于是以 Terminal 身份请求通知授权——既非预期行为，也污染开发机通知设置。旁证有力：`tauri-plugin-notification` 自己在 dev 模式特意 `set_application("com.apple.Terminal")`（`desktop.rs:207-213`），正是对该回落事实的让步。 | §2.2 判据改为「`bundleIdentifier` 不等于 `app.config().identifier` 即跳过」，同时覆盖 nil 与回落两种情形；identifier 从配置读取，不硬编码 |
| GLM-04 | glm | nit | 接受 | 原计划承诺「脚本与产物一并提交便于复现」，但验证步骤全是运行时人工验收，无任何一环能发现「只改产物没改脚本」或反之，承诺会落空。 | §3 验证方法新增第 8 步：重跑脚本比对 sha256，并校验 36×36 / RGBA / 颜色通道全 0 三项不变量 |

## 1. 逐条裁决

### AGY-01（major）· feature 遗漏三处 → **接受**

三点我逐一核实，全部成立，按原计划编码必然编译失败：

1. `Image::from_bytes` 受 `#[cfg(any(feature = "image-ico", feature = "image-png"))]` 门控
   （`tauri-2.11.5/src/image/mod.rs:74`），而 tauri 的 `default` features 为
   `["wry", "compression", "common-controls-v6", "dynamic-acl", "x11", "dbus"]`，
   **不含** `image-png`；
2. `objc2-user-notifications-0.3.2/Cargo.toml` 确有独立的 `UNNotification = [...]` 项；
3. `objc2-foundation-0.3.2/Cargo.toml` 确有独立的 `NSBundle = []` 项。

**落点**：§3.2 依赖清单改为下述版本，三处 feature 全部补齐。

关于第 1 点的选型：审查员给了「补 `image-png`」与「改用 `Image::new` 传 RGBA」两个选项，
**采纳前者**。理由是 PNG 产物可被图像工具直接查看与比对，而裸 RGBA 字节无法目视校验，
与 GLM-04 要求的「产物可复现、可核对」目标一致；`image-png` 只是打开 tauri 已内置的
png 解码路径，不引入新 crate。

### AGY-02（major）· 非 bundle 防护未覆盖 setup → **接受**

原计划只在发送入口检查 bundle，但 setup 阶段的 `setDelegate` 与
`requestAuthorizationWithOptions` 同样要先取 `currentNotificationCenter()`，
裸二进制下会在启动期就抛 `NSInternalInconsistencyException`。这是原方案的真实漏洞。

**落点**：§3.2 新增 `init_notifications(app_handle)` 作为唯一初始化入口，
入口处统一做环境判据（判据本身按 GLM-03 修正），不满足则记 warn 并整体跳过
delegate 注册与授权请求；发送入口 `show_transfer_notification` 保留同样的判据，
两处独立成立，不依赖调用顺序。

### AGY-03（minor）· 未过滤 actionIdentifier → **接受**

核实 `UNNotificationDefaultActionIdentifier` 与 `UNNotificationDismissActionIdentifier`
均已由绑定导出（`generated/UNNotificationResponse.rs:11,16`），
`response.actionIdentifier()` 返回 `Retained<NSString>`（同文件 :55）。
不过滤会导致用户划走通知时也弹出主窗口。

**落点**：§3.2 `didReceive` 分支中，仅当 `actionIdentifier` 等于
`UNNotificationDefaultActionIdentifier` 时才唤起窗口；其余 action 直接放行。
**无论走哪个分支都必须调用 completionHandler**，否则系统会认为回调未完成。

### AGY-04（minor）· 可见性与 block 生命周期 → **部分接受**

拆成两部分处理：

**接受**：`reveal_main_window`（`lib.rs:32`）当前无 `pub(crate)`，
`platform/notification_macos.rs` 跨模块调用不到，须提升可见性。
**接受**：completionHandler 是 `&block2::DynBlock<...>` 借用引用，不是 `'static`，
无法移入异步闭包，必须在当前帧同步调用。

**驳回该条中「delegate 已在主线程派发，建议直接同步调用 reveal_main_window」的部分。**

理由：该前提与 GLM-02 直接冲突，我核对绑定源码后判定 GLM-02 正确——
`extern_protocol!` 定义的 `UNUserNotificationCenterDelegate`
（`generated/UNUserNotificationCenter.rs:219-242`）只继承 `NSObjectProtocol`，
**没有任何 `MainThreadOnly` / `MainThreadMarker` 约束**，绑定层不提供主线程保证；
Apple 文档也从未承诺该回调在主线程。以「已在主线程」为前提直接操作 NSWindow
是不安全的假设。

两条意见实际可以同时满足，并不互斥：窗口操作的闭包只捕获 `AppHandle`（`'static`），
经 `run_on_main_thread` 派发；completionHandler 不进闭包，在当前帧同步调用。
详见下方 §3.2「线程模型」。

### GLM-01（minor）· 缺 UNNotification feature → **接受**

与 AGY-01 第 2 点同一问题，两侧独立命中，合并到同一落点。
一并采纳其附带结论：`UNMutableNotificationContent` 由 `UNNotificationContent`
feature 覆盖（`generated/mod.rs:108-123`），**不需**单列，原清单无此冗余项。

### GLM-02（minor）· 线程假设不成立 → **接受**

见 AGY-04 的裁决说明，证据支持本条。原计划把 `run_on_main_thread`
写成「为稳妥仍走」的可选保险，措辞会误导 S5 实现或日后重构直接省略它。

**落点**：§3.2 新增「线程模型」段，把主线程派发**升格为硬性要求**，并写明原因。

### GLM-03（minor）· 非 bundle 判据覆盖不到 dev 态 → **接受**

判据 `bundleIdentifier() == nil` 确实覆盖不了最常见的开发场景：
从终端跑 `tauri dev` 时，裸二进制的 `mainBundle` 会回落到宿主应用（Terminal），
`bundleIdentifier` 非空，防护分支不触发，于是以 Terminal 身份请求通知授权——
既非预期行为，也会污染开发机的通知设置。

该判断有旁证：`tauri-plugin-notification` 自己在 dev 模式下特意
`set_application("com.apple.Terminal")`（`desktop.rs:207-213`），
正是对这一回落事实的让步。

**落点**：判据从「nil 才跳过」改为「`bundleIdentifier` 不等于本应用 identifier
（`com.unidrop.client`，见 `tauri.conf.json:5`）即跳过」。
该判据同时覆盖 nil 与回落两种情形，比叠加 `tauri::is_dev()` 更直接——
它约束的是「当前进程是否真以本应用身份运行」这一充要条件。
identifier 从 `app_handle.config().identifier` 读取，不硬编码字符串。

### GLM-04（nit）· 脚本与产物一致性无验收 → **接受**

原计划承诺「脚本与产物一并提交，便于日后调整后复现」，但验证步骤全是运行时人工验收，
没有任何一环能发现「只改产物没改脚本」或反之。承诺会落空。

**落点**：§6 新增第 8 步，重跑脚本并比对产物哈希，同时校验
36×36 / RGBA / 颜色通道全 0 三项不变量。

## 2. 修订后的关键设计（仅列相对原方案的变更）

原方案 §1 根因、§2 目标、§3.1 托盘模板图标、§4 改动清单、§5 风险表**维持不变**，
两侧审查均确认 §1 证据链与 §3.1 尺寸推导正确。以下为 §3.2 的修订结果。

### 2.1 依赖清单（修订 AGY-01 / GLM-01）

```toml
# tauri 依赖补 image-png：托盘模板图经 Image::from_bytes 载入，
# 该方法受 image-ico/image-png feature 门控，而 tauri 默认 features 不含它。
tauri = { version = "2.0.0", features = ["tray-icon", "image-png"] }
```

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.5"
objc2-foundation = { version = "0.2", features = ["NSString", "NSArray", "NSURL", "NSData"] }
objc2-app-kit = { version = "0.2", features = ["NSPasteboard", "NSPasteboardItem"] }

# 以下四项是 UNUserNotificationCenter 所需，属 objc2 0.6 那一代。
# 与上面的 0.5 并存是刻意的：0.6.4 早已由 tao/dispatch2 带进 macOS 构建
# （见 Cargo.lock），两代不交换任何类型；把剪贴板一并升到 0.6 会为本次
# 需求之外的稳定代码引入回归风险，收益与风险不成比例。
objc2-v06 = { package = "objc2", version = "0.6" }
objc2-foundation-v06 = { package = "objc2-foundation", version = "0.3", features = [
    "NSString",
    "NSError",
    # NSBundle 是独立 feature，用于判断当前进程是否真以本应用身份运行。
    "NSBundle",
] }
objc2-user-notifications = { version = "0.3", features = [
    "UNUserNotificationCenter",
    "UNNotificationContent",   # 覆盖 UNMutableNotificationContent，无需单列
    "UNNotificationRequest",
    "UNNotificationResponse",
    "UNNotificationTrigger",
    "UNNotificationSound",
    # willPresent 回调被 cfg(all(feature="UNNotification", feature="block2")) 门控，
    # 缺此 feature 则「前台也显示通知」无法实现。
    "UNNotification",
    "UNError",
    "block2",
] }
block2 = "0.6"
```

### 2.2 统一初始化入口与环境判据（修订 AGY-02 / GLM-03）

```
fn notifications_available(app: &AppHandle) -> bool
    读 NSBundle::mainBundle().bundleIdentifier()
    与 app.config().identifier 比较，相等才返回 true
    否则 log::warn! 说明「当前进程未以本应用身份运行，跳过系统通知」
```

两处调用该判据，彼此独立、不依赖调用顺序：

- `init_notifications(app_handle)`：setup 阶段调用。判据不成立则**整体跳过**
  delegate 注册与授权请求（修复 AGY-02 指出的启动崩溃）；成立则先 `setDelegate`
  再 `requestAuthorizationWithOptions`（顺序不可颠倒，否则首次授权弹窗期间的
  回调会丢失）。
- `show_transfer_notification` 的 macOS 分支：发送前再判一次。

### 2.3 线程模型（修订 GLM-02 / AGY-04，硬性要求）

> **delegate 回调的线程不受保证。** `UNUserNotificationCenterDelegate`
> 在 objc2 绑定中只继承 `NSObjectProtocol`，无 `MainThreadOnly` / `MainThreadMarker`
> 约束，Apple 亦未承诺主线程。而 `reveal_main_window` 内部的
> `is_minimized` / `unminimize` / `show` / `set_focus` 属 NSWindow 系 API，
> **必须在主线程执行**。

因此 `didReceive` 回调的实现固定为下述两步，顺序与同步性都不可改：

1. **窗口操作**：`app_handle.run_on_main_thread(move || reveal_main_window(&handle))`。
   该闭包只捕获 `AppHandle`（`'static`），不捕获任何 block 引用，因此可安全移入；
2. **completionHandler**：在当前帧**同步**调用。它的类型是
   `&block2::DynBlock<dyn Fn()>`，是借用引用而非 `'static`，
   移入第 1 步的闭包会违反生命周期约束（AGY-04 指出的真实约束）。

`willPresent` 回调同理：completionHandler 同步回传
`UNNotificationPresentationOptions::Banner | Sound`，使应用在前台时通知照常显示。

### 2.4 点击动作过滤（修订 AGY-03）

`didReceive` 中先取 `response.actionIdentifier()`，仅当其等于
`UNNotificationDefaultActionIdentifier` 时才执行上述第 1 步；
`UNNotificationDismissActionIdentifier` 及其他 action 不唤起窗口。
**两种分支都必须执行第 2 步**（调用 completionHandler）。

### 2.5 可见性调整（修订 AGY-04）

`lib.rs:32` 的 `reveal_main_window` 提升为 `pub(crate)`，
供 `platform/notification_macos.rs` 调用。这也是当前唯一的窗口唤起入口，
托盘菜单、第二实例、`RunEvent::Reopen` 都走它，通知点击复用它可保证行为一致。

## 3. 验证方法（新增第 8 步，修订 GLM-04）

原 §6 的 1–7 步维持不变，新增：

8. **资源可复现性校验**：重跑 `python3 client/scripts/gen-tray-icon.py`，
   比对产物与仓库内 `icons/tray-macos.png` 的 sha256 是否一致；
   并校验三项不变量——尺寸 36×36、格式 RGBA、颜色通道全 0（形状仅由 alpha 表达）。

## 4. 对 S5 编码的约束小结

1. 三处 feature 必须在动手前写进 `Cargo.toml`，不靠编译失败试错；
2. `init_notifications` 是 macOS 通知的唯一初始化入口，环境判据在入口处一次判定；
3. 环境判据用 `bundleIdentifier == config().identifier`，不硬编码字符串、不用 nil 判据；
4. delegate 回调中窗口操作走 `run_on_main_thread`，completionHandler 同步调用，两者不可互换；
5. `didReceive` 必须过滤 `actionIdentifier`，且所有分支都要调用 completionHandler；
6. `reveal_main_window` 改 `pub(crate)`，不新造窗口唤起路径。
