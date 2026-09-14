---
schema: trivium.review.v1
topic: macos-dock-hidden
stage: code
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260914T103457Z
rerun_index: 1
snapshot_hash: sha256:9b39b859c63e76ee5d231781d07fefd3858817a34175bbfece199961418d592d
blind: true
started_at: 2026-09-14T10:34:59.640Z
finished_at: 2026-09-14T10:52:59.570Z
duration_s: 193
tool_calls:
  read: 31
  grep: 8
  glob: 0
  run_command: 0
  project_reads: 15
evidence_ok: true
verdict: approve-with-nits
findings_count:
  blocker: 0
  major: 0
  minor: 0
  nit: 2
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: 31cde2dd-fc8b-426d-83be-a51b5acfce32
---

# 代码审查：macos-dock-hidden

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260914T103457Z`
> ｜ 本轮审核者 gemini、glm

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

整体架构改造非常严谨，未发现阻断性或严重缺陷，核心逻辑闭环完备；仅有两处轻微注释表述与单测覆盖建议。
激活策略配对（Info.plist LSUIElement + run 前 set_activation_policy）互补且时机论证完全成立，成功消除 Dock 图标启动闪烁。
watchdog 改为双速常驻轮询（稳态 300s / 异常态 30s）并删除提前 return 属于必要改动，根治了热配置变更导致的幽灵进程，且去重机制避免了无谓开销。
优先切换策略再唤醒窗口的顺序准确规避了 AppKit 焦点重置陷阱。
Unknown 态采取「策略静默放 Dock，通知不打扰」的非对称处置兼顾了安全与体验。
判定为 approve-with-nits。

**结论**：`approve-with-nits`

## 审查意见（共 2 条：吹毛求疵 2）

### AGY-01 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/platform/tray_placement_macos.rs:492` |
| 类别 | maintainability ｜ 层次 code |
| 置信度 | high |

**问题**：注释中「不存在『28 秒里彻底没有入口』的窗口期」的表述略有歧义。实际上当用户配置了 start_minimized: true 且恰好遭遇托盘被拒时，在启动至阶段一定论的这最多 28 秒内，确实存在屏幕上既无托盘图标、无 Dock 图标、主窗口也隐藏的短暂无入口窗口期；28 秒后两者才同时出现。

**依据**：client/src-tauri/src/platform/tray_placement_macos.rs:492-493 注释写道「不存在『28 秒里彻底没有入口』的窗口期」；但若开启了 start_minimized，client/src-tauri/src/lib.rs:872 启动时不显示窗口，而看门狗在首探 3s + 重试 25s 期间尚未执行 apply_activation_policy 和 handle_not_placed，此时 Dock 为 Hidden、托盘不可见、窗口不可见。

**建议**：将注释澄清为「不存在 28 秒后仍彻底无入口的死角，无入口窗口期被严格约束在上界 28 秒内且定论时窗口与 Dock 同步补齐」，避免后来维护者误解该边界场景。

### AGY-02 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/platform/tray_placement_macos.rs:905` |
| 类别 | test-gap ｜ 层次 code |
| 置信度 | high |

**问题**：policy_transition_dedupes 单测覆盖了 Placed 与 Rejected 的状态转移组合，但未覆盖 TrayPlacement::Unknown 参与时的转移（如 (Some(Hidden), Unknown) -> Some(Visible) 与 (Some(Visible), Unknown) -> None）。

**依据**：查阅 client/src-tauri/src/platform/tray_placement_macos.rs:905-936，policy_transition_dedupes 测试用例仅包含了 Placed 和 Rejected 两个分支，未对 Unknown 状态下的策略迁移做断言。

**建议**：在 policy_transition_dedupes 中补齐 Unknown 态的策略迁移与去重断言。

## 认为正确的部分

- 启动期激活策略在 run() 之前通过 App::set_activation_policy 设置为 Accessory，并且与 client/src-tauri/Info.plist 的 LSUIElement: true 形成严密互补，成功杜绝了发布包启动时 Dock 图标弹跳以及开发模式（tauri dev）下 tao 默认 Regular 策略覆盖的问题；落点在 run() 之前而非 setup() 内的时机论证完全成立。
- watchdog 改造成常驻双速轮询（稳态 300s / 异常态 30s）并移除两处早期 return，彻底解决了用户在运行期间动态修改 macOS 准入控制导致图标消失变为幽灵进程的隐患；稳态轮询轻量无句柄泄漏，去重机制保证了无无效主线程调度与 Dock 图标二次弹跳。
- 看门狗定论后优先执行 apply_activation_policy 再执行 handle_not_placed 的调用序论证成立，有效规避了在 Accessory 态下先激活窗口后切换 Regular 导致 macOS 丢弃前台窗口焦点的 AppKit 机制陷阱。
- TrayPlacement::Unknown 态处置优雅：在激活策略侧往安全侧倒（静默放出 Dock 图标兜底保证入口），在打扰侧往保守侧倒（不显示警告横幅、不强行打扰用户唤窗），兼顾了入口兜底安全性与静默用户体验。
- 唤起窗口链路（reveal_main_window）严格保持 unminimize() -> show() -> set_focus() 顺序，并在 RunEvent::Reopen、第二实例（single-instance）等入口全面对齐，保障了无 Dock 状态下的前台拉取能力。

## 未覆盖范围（本侧盲区）

- 真实 macOS 26 (Tahoe) 物理机环境下的端到端界面点击、多屏拖动与深浅色模式切换实机动态渲染验证（当前为纯静态代码与跨模块语义交叉审查）。
- 在不同第三方输入法或高负载场景下，Webview 内原生 key equivalent（如快捷键剪贴板粘贴）的极低概率键盘拦截表现。
- Windows 与 Linux 平台上的托盘交互回归（虽然已核验 #[cfg(target_os = "macos")] 隔离与 allow(unused_mut)，但未在非 macOS 操作系统实际运行）。

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/tauri.conf.json`
- `client/src-tauri/Info.plist`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/platform/mod.rs`
- `client/src-tauri/src/platform/tray_placement_macos.rs`
- `client/src-tauri/src/commands/window_cmd.rs`
- `client/src/App.tsx`
- `client/src/components/MenuBarHiddenBanner.tsx`
- `client/src/components/SendModal.tsx`
- `docs/INSTALL.md`
- `docs/USER_GUIDE.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":31,"grep":8,"glob":0,"run_command":0,"project_reads":15}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
