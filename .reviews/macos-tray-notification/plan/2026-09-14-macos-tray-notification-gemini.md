---
schema: trivium.review.v1
topic: macos-tray-notification
stage: plan
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260914T031414Z
rerun_index: 1
snapshot_hash: sha256:363c3fed3a5eb1b902317a84e7eca2aaf7a2162a46b0f4a68398a119e4d0b41f
blind: true
started_at: 2026-09-14T03:14:15.705Z
finished_at: 2026-09-14T03:23:53.830Z
duration_s: 243
tool_calls:
  read: 41
  grep: 10
  glob: 13
  run_command: 0
  project_reads: 16
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 2
  minor: 2
  nit: 0
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: bace573a-72d0-47c6-be12-39bdac4c9770
---

# 计划审查：macos-tray-notification

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260914T031414Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

方案整体设计扎实，根因分析透彻，36x36 模板图推导及 objc2 两代共存决策稳妥。
但存在两处关键缺陷需调整：非 bundle 环境防护未覆盖 setup 启动阶段，裸二进制运行会在设置 delegate 时直接崩溃；
Cargo.toml 依赖声明遗漏了 tauri/image-png、objc2-user-notifications/UNNotification 与 objc2-foundation/NSBundle 特性，将导致编译失败。
此外通知点击回调应校验 UNNotificationDefaultActionIdentifier 避免划走通知时误弹窗口。建议修改后进入实施。

**结论**：`request-changes`

## 审查意见（共 4 条：重要 2 ｜ 次要 2）

### AGY-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/Cargo.toml` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：方案在 Cargo.toml 规划的依赖 feature 存在三处遗漏，按计划编码将直接导致编译失败：1) tauri 未启用 image-png 特性，无法在运行时调用 Image::from_bytes；2) objc2-user-notifications 缺少 UNNotification 特性，导致 UNUserNotificationCenterDelegate 协议不包含 willPresentNotification 回调；3) objc2-foundation-v06 缺少 NSBundle 特性，无法调用 NSBundle::mainBundle() 进行非 bundle 防护。

**依据**：查阅 client/src-tauri/Cargo.toml 第 17 行可知 tauri 仅启用了 [\"tray-icon\"] 特性；而在 tauri-2.11.5/src/image/mod.rs:74 中，Image::from_bytes 显式受 #[cfg(any(feature = \"image-ico\", feature = \"image-png\"))] 条件编译保护。查阅 objc2-user-notifications-0.3.2/src/generated/UNUserNotificationCenter.rs:222 可知 userNotificationCenter:willPresentNotification:withCompletionHandler: 声明在 #[cfg(all(feature = \"UNNotification\", feature = \"block2\"))] 下，方案 §3.2 的 features 列表未声明 UNNotification。查阅 objc2-foundation-0.3.2/Cargo.toml:72 可知 NSBundle 是一项独立可选 feature，方案声明的 features 仅含 [\"NSString\", \"NSError\"]。

**建议**：在 Cargo.toml 的 tauri 依赖 features 中补充 image-png（或改用已有依赖 image 解码 RGBA 后调用 Image::new）；在 objc2-user-notifications 的 features 中补齐 UNNotification；在 objc2-foundation-v06 的 features 中补齐 NSBundle。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.2` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：非 bundle 环境（如 tauri dev / cargo run）的防护未覆盖应用 setup 启动阶段。若仅在发送通知（show_transfer_notification）时检查 bundleIdentifier，应用在 setup 注册 delegate 与请求授权时即会抛出 NSInternalInconsistencyException 导致崩溃。

**依据**：方案 §3.2 明示 UNUserNotificationCenter.currentNotificationCenter() 在无 bundle id 时会抛出 NSInternalInconsistencyException，并提出通过检查 NSBundle::mainBundle().bundleIdentifier() 防护并返回 Ok(())；但在 §3.2 授权与 delegate 章节，方案规划在 lib.rs 的 setup 阶段调用 setDelegate 与 requestAuthorizationWithOptions。这两个接口同样需要调用 currentNotificationCenter()。若未在 setup 阶段同样做 bundleIdentifier 检查，tauri dev 裸二进制启动时会在 setup 期间直接崩溃。

**建议**：将 macOS 通知中心的初始化逻辑封装为独立函数（如 init_notifications），并在入口处统一检查 bundleIdentifier 是否存在；若为 None 则记录 warn 日志后直接跳过 delegate 注册与授权请求，确保开发态进程顺利启动。

### AGY-03 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.2` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：UNUserNotificationCenterDelegate 的 didReceiveNotificationResponse 回调未过滤 actionIdentifier，用户在通知横幅上点击关闭/忽略时也会错误唤起主窗口。

**依据**：Apple UserNotifications 框架规范中，用户点击通知横幅触发 UNNotificationDefaultActionIdentifier，而用户划走或关闭通知时若产生 response，其 actionIdentifier 为 UNNotificationDismissActionIdentifier。方案 §3.2 描述 didReceiveNotificationResponse 直接调用 reveal_main_window，未对 actionIdentifier 进行判断，这会导致用户关闭通知时窗口也被异常弹出。

**建议**：在 didReceiveNotificationResponse 回调实现中，先比对 response.actionIdentifier() 是否等于 UNNotificationDefaultActionIdentifier，仅在用户主动点击通知内容时唤起主窗口，并在无论何种 action 下均确保调用 completionHandler。

### AGY-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/lib.rs:32` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | high |

**问题**：reveal_main_window 函数当前为 lib.rs 私有泛型函数，跨模块在 platform/notification_macos.rs 中直接调用存在可见性阻碍；且 delegate 回调中若异步调度 run_on_main_thread，需注意 block2 completionHandler 无法随 'static 闭包捕获的问题。

**依据**：client/src-tauri/src/lib.rs 第 32 行 reveal_main_window 声明为 fn reveal_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>)，未带 pub(crate)；且 UNUserNotificationCenterDelegate 回调已由系统保证在主线程派发，若在回调内使用 app_handle.run_on_main_thread 异步执行，completionHandler 作为非 'static 引用的 Block 无法移入闭包，必须在当前帧同步执行或注意所有权处理。

**建议**：将 reveal_main_window 提升为 pub(crate)；由于 delegate 方法已在主线程派发，建议在 delegate 中直接同步调用 reveal_main_window 并随后同步调用 completion_handler.call(())，避免不必要的跨线程异步闭包与 Block 生命周期处理。

## 认为正确的部分

- 托盘图标根因定位准确：32x32.png 为深色彩色图且未开启 template 模式，导致在深色菜单栏上与背景融为一体；同时查证了 show_menu_on_left_click 默认为 true 导致现有左键事件被系统抢先拦截，修复方向明确正确。
- 托盘图标尺寸推导严谨：根据 tray-icon 源码实现（高度硬编码为 18.0 pt，宽度由宽高比换算），推导出正方形在 Retina @2x 下应为 36x36 px 纯黑底色 + alpha 遮罩，符合 macOS 模板图标规范与 AppKit 渲染机制。
- 脚本化无依赖生成资源：规划用 Python 3 标准库（zlib 与 struct）生成 36x36 RGBA PNG，不引入第三方图像库依赖，且脚本与产物一同提交，保证后续资产维护的可复现性。
- 系统通知废弃根因确凿：准确定位到 tauri-plugin-notification 依赖的底层 NSUserNotificationCenter 自 macOS 11 起废弃并在当前系统静默失效，升级到 UNUserNotificationCenter 是不可逆且必要的演进。
- objc2 双代共存决策审慎：经 Cargo.lock 验证，objc2 0.6.4 已随 tao/wry 参与 macOS 依赖树，新通知模块采用 0.6 生态且两代不跨 FFI 交换类型，避免了对已稳定运行的剪贴板模块（NSPasteboard 0.5）做无谓重构与回归风险。
- delegate 生命周期理解正确：准确识别出 UNUserNotificationCenter.delegate 为 weak 弱引用属性，规划使用全局 OnceLock<Retained<...>> 长期持有 delegate 实例，防止回调因提前 drop 而丢失。
- 通知失败错误留痕：将 transfer_engine.rs 中的 4 处 let _ = 改为捕获 Result 并记录 log::warn!，符合项目已有的错误留痕规范。

## 未覆盖范围（本侧盲区）

- Windows 与 Linux 平台上的通知与托盘行为（方案明确列为非目标，未纳入变更范围）。
- 通知的高级交互形态（如自定义 Action 按钮、内联快捷回复输入框、通知分类 Category 等）。
- 底层网络传输引擎与滑动窗口、E2EE 密文解密逻辑。
- Retina 与非 Retina 多显示器热插拔时系统菜单栏对 36x36 模板图的即时重绘表现。

## 实际查阅的项目文件

- `.reviews/macos-tray-notification/plan/2026-09-14-macos-tray-notification-claude.md`
- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/Cargo.lock`
- `client/src-tauri/capabilities/default.json`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/platform/notification.rs`
- `client/src-tauri/src/platform/mod.rs`
- `client/src-tauri/src/platform/clipboard_macos.rs`
- `client/src-tauri/src/platform/listener_macos.rs`
- `client/src-tauri/src/core/transfer_engine.rs`

> 编排器从工具轨迹中记录到的读取次数：{"read":41,"grep":10,"glob":13,"run_command":0,"project_reads":16}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
