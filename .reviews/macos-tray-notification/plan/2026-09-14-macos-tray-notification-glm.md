---
schema: trivium.review.v1
topic: macos-tray-notification
stage: plan
role: glm
vendor: zcode
agent_cli: zcode/0.16.5
model_requested: GLM-5.3
model_effective: bigmodel-coding-plan/GLM-5.3
model_effective_source: trace
model_route: config_copy
run_id: 20260914T031414Z
rerun_index: 1
snapshot_hash: sha256:363c3fed3a5eb1b902317a84e7eca2aaf7a2162a46b0f4a68398a119e4d0b41f
blind: true
started_at: 2026-09-14T03:14:15.705Z
finished_at: 2026-09-14T03:23:53.830Z
duration_s: 578
tool_calls:
  read: 21
  grep: 13
  glob: 6
  run_command: 0
  project_reads: 13
evidence_ok: true
verdict: approve-with-nits
findings_count:
  blocker: 0
  major: 0
  minor: 3
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: sess_8bbdaca7-928b-4e59-b93e-471fd5505c30
---

# 计划审查：macos-tray-notification

> Critic-B · 智谱 ZCode ｜ 模型 `bigmodel-coding-plan/GLM-5.3`
> ｜ 运行 `20260914T031414Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

计划质量高，方向正确：两条根因的证据链我逐条核实成立（NSUserNotificationCenter 废弃 API 确在 mac-notification-sys objc/notify.m:79；插件静默吞错确在 desktop.rs:61-67/216-218；彩色托盘图标确来自 lib.rs:696 且 tauri-codegen 对非 Windows 目标取 bundle.icon 首个 png）。依赖策略成立：Cargo.lock 证实 objc2 0.5.2/0.6.4 双代并存是 tao/dispatch2/tauri 链带来的既成事实，objc2-user-notifications 0.3.2 目前由 iOS 侧 objc2-ui-kit 拉入、macOS 构建不参与编译。36×36@2x 模板图标推导与 tray-icon 0.24.2 硬编码 18pt + setSize 的实现吻合。最需要修的一条是 GLM-01：feature 清单缺 UNNotification，willPresent 回调将无法编译；其次「回调在主线程派发」的前提不成立（缓解应升格为硬性要求）、非 bundle 防护的 nil 判据在常见 dev 场景不触发。均为小改可修，无方向性返工风险。

**结论**：`approve-with-nits`

## 审查意见（共 4 条：次要 3 ｜ 吹毛求疵 1）

### GLM-01 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.2 objc2-user-notifications feature 清单` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：feature 清表缺 `UNNotification`。`userNotificationCenter:willPresentNotification:withCompletionHandler:` 在 objc2-user-notifications 0.3.2 中被 `#[cfg(all(feature = "UNNotification", feature = "block2"))]` 门控，不开该 feature 则 trait 上根本不存在此方法，计划的核心卖点之一「应用前台时通知仍显示」（验收步骤 5）无法实现。其余所列 feature 均真实存在，且 UNMutableNotificationContent 无需单列（由 UNNotificationContent feature 覆盖）。

**依据**：读 ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/objc2-user-notifications-0.3.2/src/generated/UNUserNotificationCenter.rs 第 222-231 行（willPresent 方法带 cfg 门控）、同 crate src/generated/mod.rs 第 108-123 行（UNMutableNotificationContent 归 UNNotificationContent feature）、该 crate Cargo.toml [features] 段（存在 UNNotification 项、无独立 UNMutableNotificationContent 项）。

**建议**：在 §3.2 依赖清单的 features 里补上 `UNNotification`；S5 核对 feature 时把它当作已知缺口直接补入，而不是靠编译失败试错。

### GLM-02 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.2「线程安全」段` |
| 类别 | concurrency ｜ 层次 plan |
| 置信度 | medium |

**问题**：计划断言「两个回调均由系统在主线程派发」，该前提无依据且很可能不成立：UNUserNotificationCenterDelegate 的回调官方从未承诺主线程，普遍在后台线程执行；而 reveal_main_window 调用的 is_minimized/unminimize/show/set_focus 属于必须在主线程操作的 NSWindow 系 API。计划虽写了「为稳妥仍走 run_on_main_thread」，但把唯一正确的做法定性成可省略的保险——S5 实现或日后重构若按错误前提直接调用，就会在非主线程触碰窗口对象。

**依据**：objc2-user-notifications 0.3.2 生成的 delegate trait（src/generated/UNUserNotificationCenter.rs:221-242）方法签名不携带 MainThreadMarker；对比 tray-icon 0.24.2 对主线程专属 API 的严格处理（src/platform_impl/macos/mod.rs:35 `MainThreadMarker::new().ok_or(Error::NotMainThread)?`、:75 `button(mtm)`），绑定层未给出任何主线程保证。未做运行时实验，故置信度标 medium。

**建议**：把该段表述改为「回调线程不受保证，必须经 app_handle.run_on_main_thread 派发到主线程后再操作窗口」，从『稳妥起见』升格为硬性要求；didReceive 的 completionHandler 在派发完成后调用。

### GLM-03 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.2「非 bundle 环境必须防护」` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | medium |

**问题**：以 `bundleIdentifier() == nil` 为跳过条件，覆盖不了最常见的开发态：从 Terminal（或 IDE 内嵌终端）跑 `tauri dev` 时，裸二进制的 NSBundle.mainBundle 会回落到宿主应用（Terminal），bundleIdentifier 非空（如 com.apple.Terminal），防护分支不触发。此时不会崩（非 nil 即不抛异常），但会以宿主身份调用通知中心并请求授权，开发机上可能出现「Terminal 想要发送通知」的弹窗，而非计划预期的「跳过并记 warn」。nil 分支只在无 bundled 祖先（ssh/launchd 直启）时出现。

**依据**：tauri-plugin-notification 2.4.0 desktop.rs:207-213 在 dev 模式特意 `set_application("com.apple.Terminal")`，正是上游对「dev 裸进程挂在宿主 bundle 身份下」这一事实的让步，佐证 mainBundle 回落行为；本次未实际运行 dev 复现，宿主回落为 macOS 通行行为，故标 medium。

**建议**：判据从「nil 才跳过」改为「bundleIdentifier 不等于本应用标识（com.unidrop.client，见 client/src-tauri/tauri.conf.json:5）即跳过」，或叠加 `tauri::is_dev()` 时跳过，使 dev 态行为与计划描述一致。

### GLM-04 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4 改动清单 / §6 验证方法` |
| 类别 | test-gap ｜ 层次 plan |
| 置信度 | medium |

**问题**：gen-tray-icon.py 与 tray-macos.png 一并提交以保证可复现，但验证步骤没有「重跑脚本、比对产物与提交的 PNG 一致」这一环；日后只改产物不改脚本（或反之）不会被任何验收发现，『便于复现』的承诺落空。

**依据**：§4 清单含脚本与产物两项新增；§6 验证步骤 1-7 全部是打包后的人工运行时验收，无脚本↔产物一致性检查。

**建议**：§6 增加一步：重跑 `python3 client/scripts/gen-tray-icon.py` 后比对产物与仓库内文件（或校验 36×36、RGBA、颜色通道全 0 的不变量）。

## 认为正确的部分

- §1 两条根因及引用行号核实无误：~/Library/Logs/com.unidrop.client/UniDrop.log 首屏确有 [01:57:44] Starting minimized to tray（且每条两遍，与计划发现的日志重复问题一致）；transfer_engine.rs:700/987/1004/1014 四处 `let _ =` 吞错属实；desktop.rs:61-67 确为硬编码 Granted 的空权限实现，补权限请求无效的澄清正确
- objc2 双代共存是既成事实而非新增负担：client/src-tauri/Cargo.lock 同时含 objc2 0.5.2（本项目剪贴板）与 0.6.4（block2 0.6.2 ← dispatch2 0.3.1 ← tao 0.35.3 ← tauri 2.11.5），objc2-foundation 0.3.2 亦已在树上（wry 链）；objc2-user-notifications 的依赖者依赖指纹（objc2-core-data/core-graphics/quartz-core 等）与「由 iOS 专用 objc2-ui-kit 拉入」的说法一致；「新模块用 0.6 别名、旧模块不动」及否决整体升级的理由成立
- 模板图标规格推导正确：tray-icon 0.24.2 src/platform_impl/macos/mod.rs:296-297 硬编码 18pt 并按原始宽高比 setSize（36×36 位图在 18pt 逻辑尺寸下即 2x 像素密度，Retina 清晰），:310 setTemplate 证实 icon_as_template 真实生效；正方形、纯黑 + alpha 表形状符合 macOS 模板图标规范
- 「左键分支从未生效」的判断与源码一致：show_menu_on_left_click 默认 true（tauri 2.11.5 src/tray/mod.rs:300/314），tray-icon 在 mouseDown 时 performClick 弹菜单，菜单跟踪会话吞掉后续 mouseUp，现有只匹配 Up 的处理器收不到事件；关闭后左键切窗/右键菜单可行，且改动被 cfg 限定在 macOS，Windows/Linux 零变化
- delegate 用 static OnceLock 长期持有是必要的：objc2 生成的 setDelegate 标注为 weak property，通知中心不持有 delegate；先设 delegate 再请求授权的顺序也对
- 发送面设计与绑定吻合：requestAuthorizationWithOptions 仅需 UNUserNotificationCenter+block2 feature，addNotificationRequest 需 UNNotificationRequest+block2 且 handler 收 *mut NSError；identifier 用 uuid 防同 id 覆盖正确；授权 options 取 Alert+Sound 合理
- 错误留痕改造与 lib.rs:652-654 既有「Err 必须留痕」规矩呼应；保留 capabilities 的 notification:default 合理（client/package.json 仍依赖 @tauri-apps/plugin-notification，Windows/Linux 继续走插件）
- 验证方法真实可测：client/package.json:13 存在 tauri:build 脚本（含公证），日志路径实际存在，「必须打包验证、dev 测不出通知」的提醒与防护分支的行为自洽

## 未覆盖范围（本侧盲区）

- 未实际执行 cargo build/cargo info：feature 缺口与依赖兼容性判断来自静态读 crate 源码与 Cargo.lock，未经编译验证
- 未做 macOS 运行时实验（授权弹窗形态、willPresent 前台行为、点击唤起焦点、dev 态 bundle 身份回落），GLM-02/GLM-03 的置信度因此受限
- Windows/Linux 侧仅确认改动被 cfg 隔离，未逐一回归其托盘与通知行为
- 授权在运行中被用户经系统设置动态更改、专注模式（DND）下的表现：计划未提，本次也未深查
- tauri-plugin-log 双写问题按计划列为非目标，未审；scripts/notarize-dmg.sh 内容未读（属既有设施）
- 另一位审查员的视角与结论（按任务书要求背靠背，不引用不猜测）

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/platform/mod.rs`
- `client/src-tauri/src/platform/notification.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/Cargo.lock`
- `client/src-tauri/capabilities/default.json`
- `client/src-tauri/tauri.conf.json`
- `client/package.json`

> 编排器从工具轨迹中记录到的读取次数：{"read":21,"grep":13,"glob":6,"run_command":0,"project_reads":13}

---

*本文档由 TriviumCode 编排器从 `glm` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
