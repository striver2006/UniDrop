---
schema: trivium.review.v1
topic: macos-tray-notification
stage: code
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260914T032856Z
rerun_index: 1
snapshot_hash: sha256:e2d037ad73664f4602b6a773e1b9e9474a9363a96d649e0c49d14077a0d55812
blind: true
started_at: 2026-09-14T03:28:57.747Z
finished_at: 2026-09-14T03:46:31.816Z
duration_s: 253
tool_calls:
  read: 35
  grep: 22
  glob: 4
  run_command: 0
  project_reads: 15
evidence_ok: true
verdict: approve-with-nits
findings_count:
  blocker: 0
  major: 0
  minor: 0
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: fca052f6-4c60-4463-b37a-260493e86873
---

# 代码审查：macos-tray-notification

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260914T032856Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

本次变更完整解决了 macOS 托盘图标在深色菜单栏不可见以及系统通知失效的顽疾。
UNUserNotificationCenter 的 objc2 FFI 封装严密：利用 OnceLock 持久持有 delegate 避免了 weak 引用导致的静默失效，且在 setup 期先装 delegate 再请求授权；
通知点击唤起主窗口通过 app.run_on_main_thread 保证主线程执行 NSWindow 操作，并在回调线程同步调用 completionHandler，线程安全与无死锁保障完备；
running_as_app_bundle 准确比对 bundleIdentifier，彻底规避了裸二进制启动崩溃与宿主终端授权回落；
gen-tray-icon.py 严格遵循 Apple HIG 模板图规范（36x36 纯黑纯 alpha 通道），且交互上消除了菜单对左键的拦截；
仅有一处轻微的未引用符号导入（AGY-01），整体验收结论为 approve-with-nits。

**结论**：`approve-with-nits`

## 审查意见（共 1 条：吹毛求疵 1）

### AGY-01 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/platform/notification_macos.rs:25` |
| 类别 | maintainability ｜ 层次 code |
| 置信度 | high |

**问题**：从 objc2_v06 导入了 AnyThread，但在模块实现中未被显式使用，产生未使用的冗余导入。

**依据**：查阅 client/src-tauri/src/platform/notification_macos.rs 第 25 行：`use objc2_v06::{define_class, msg_send, AnyThread};`。该文件中 NotificationDelegate 的 define_class! 展开未声明 #[thread_kind = AnyThread]，全文无其它地方引用 AnyThread。

**建议**：将 client/src-tauri/src/platform/notification_macos.rs 第 25 行导入项中的 AnyThread 移除，精简为 `use objc2_v06::{define_class, msg_send};`。

## 认为正确的部分

- UNUserNotificationCenter Delegate 生命周期管理：使用 OnceLock<Retained<NotificationDelegate>> 长期持有 delegate 实例，有效避免了 ObjC 端 setDelegate: 仅为 weak 引用而在 Retained 被 drop 后静默失效的问题。并在 setup 启动期先行挂载 delegate 再请求授权，杜绝了并发时序窗口内丢失通知回调的隐患。
- 唤起窗口线程安全与闭包设计：在 delegate 回调中通过 app.run_on_main_thread 将 reveal_main_window 派发至主线程事件循环，严格遵循了 AppKit/NSWindow 只能在主线程操作的规范；同时 completionHandler 在当前回调线程同步调用，避免了借用引用跨线程移动或死锁。
- 非 Bundle 运行环境边界防护：running_as_app_bundle 严格校验 NSBundle 的 bundleIdentifier 与配置预期值，既彻底根除了裸二进制（tauri dev）调用 currentNotificationCenter 抛出 NSInternalInconsistencyException 导致的崩溃，又避免了回落到父进程（如 Terminal）身份向用户弹窗索权的乱象。
- 菜单栏托盘模板图标规范：gen-tray-icon.py 产出 36x36 RGBA 纯黑图像（RGB 全为 0，抗锯齿细节仅由 alpha 通道表达），尺寸严格对齐 tray-icon 硬编码的 18pt @2x 规范且保持正方形宽高比，配合 icon_as_template(true) 与 show_menu_on_left_click(false) 实现了深浅色菜单栏自适应与对齐 Windows 的显隐交互。
- 双代 objc2 隔离演进：针对 UNUserNotificationCenter 引入 objc2 0.6 与 block2 0.6，与既有剪贴板模块的 objc2 0.5 保持包名重命名隔离，既复用了底层依赖树已有的 0.6 版本，又避免重构稳定剪贴板 FFI 代码带来的不必要回归风险。

## 未覆盖范围（本侧盲区）

- macOS 实际真机不同深浅色模式与不同多屏缩放下的实际视觉呈现效果（当前为无 GUI 执行环境，无法观测屏幕物理像素渲染）。
- 在实际打包分发的 .app 安装包内，用户在系统偏好设置中动态关闭/再次开启通知权限时的完整运行时交互验证。
- Windows 和 Linux 平台上现有 tauri-plugin-notification 在不同桌面环境（如 GNOME/KDE/Wayland）下的通知兼容性与回退行为。

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `client/src-tauri/src/platform/notification_macos.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/platform/notification.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/scripts/gen-tray-icon.py`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/Cargo.lock`
- `client/src-tauri/tauri.conf.json`
- `client/src-tauri/src/platform/clipboard_macos.rs`
- `client/src-tauri/icons/tray-macos.png`

> 编排器从工具轨迹中记录到的读取次数：{"read":35,"grep":22,"glob":4,"run_command":0,"project_reads":15}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
