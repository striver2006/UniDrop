---
schema: trivium.review.v1
topic: macos-tray-notification
stage: code
role: glm
vendor: zcode
agent_cli: zcode/0.16.5
model_requested: GLM-5.3
model_effective: bigmodel-coding-plan/GLM-5.3
model_effective_source: trace
model_route: config_copy
run_id: 20260914T032856Z
rerun_index: 1
snapshot_hash: sha256:e2d037ad73664f4602b6a773e1b9e9474a9363a96d649e0c49d14077a0d55812
blind: true
started_at: 2026-09-14T03:28:57.747Z
finished_at: 2026-09-14T03:46:31.816Z
duration_s: 1054
tool_calls:
  read: 11
  grep: 6
  glob: 2
  run_command: 0
  project_reads: 13
evidence_ok: true
verdict: approve-with-nits
findings_count:
  blocker: 0
  major: 0
  minor: 1
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
consulted_unverified: true
heterogeneous: true
session_id: sess_57a4971b-de3d-44be-a914-333ce4346c93
---

# 代码审查：macos-tray-notification

> Critic-B · 智谱 ZCode ｜ 模型 `bigmodel-coding-plan/GLM-5.3`
> ｜ 运行 `20260914T032856Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

最危险的一条是 GLM-01：willPresent 回传的 Banner 选项是 macOS 11+ 才有的，而打包清单沿 tauri 默认仍声明最低 10.13，10.14/10.15 上前台通知可能被静默抑制——但置信度低、受众趋零。三个审查焦点本身全部核实正确：delegate 用进程级 static 长期持有并以正确顺序先于授权请求安装（上游绑定确认 setDelegate 为 weak property）；点击回调在任意队列触发时 NSWindow 操作全部经 run_on_main_thread 派发且 completionHandler 原帧调用；模板图标 36×36/RGB 全 0/纯 alpha，与生成脚本逐像素一致，image-png feature 与 show_menu_on_left_click(false) 的理由均经上游源码证实。全部 FFI 签名逐一比对上游生成绑定无误。

**结论**：`approve-with-nits`

## 审查意见（共 2 条：次要 1 ｜ 吹毛求疵 1）

### GLM-01 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/platform/notification_macos.rs:60` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | low |

**问题**：willPresent 恒定回传 UNNotificationPresentationOptions::Banner(1<<4)|Sound。Banner/List 是 macOS 11.0 才引入的 presentation 选项（macOS 11 前前台展示用已废弃的 Alert bit=1<<2），而 tauri.conf.json 未设 bundle.macOS.minimumSystemVersion，tauri 默认写入 LSMinimumSystemVersion=10.13。若真有用户在 10.14/10.15 上运行：框架存在、投递可用，但前台回调传入系统不认识的 bit 且未置 Alert bit，前台横幅大概率被静默抑制——恰是本次修复要根除的「收不到提醒」形态在旧系统上的残影。注：UserNotifications.framework 的硬链接在本次改动前已由 mac-notification-sys 引入（10.13 无法启动非本次新增）。

**依据**：tauri.conf.json 无 minimumSystemVersion 配置；tauri-utils-2.9.3/src/config.rs 的 fn macos_minimum_system_version() 返回 Some("10.13".into())（本地 cargo registry 源码）；objc2-user-notifications-0.3.2/src/generated/UNUserNotificationCenter.rs 中 const Banner = 1<<4（对应 Apple 头文件中 macOS 11.0+ 可用的 UNNotificationPresentationOptionBanner）；notification_macos.rs:60-62 的 completion_handler.call。

**建议**：在 tauri.conf.json 的 bundle.macOS.minimumSystemVersion 显式设为 "11.0"（一行配置，让清单与实际能力一致）；若确需支持 10.14/10.15，则按 ProcessInfo 的 operatingSystemVersion 对旧版本回退 Alert 选项。

### GLM-02 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/scripts/gen-tray-icon.py:146` |
| 类别 | test-gap ｜ 层次 code |
| 置信度 | high |

**问题**：生成器与产物 client/src-tauri/icons/tray-macos.png 之间没有任何一致性校验。「产物可由脚本无环境复现」这一承诺目前只活在注释里：将来改了脚本忘了重跑、或有人直接改二进制产物，漂移会无声发生，且模板图一旦 RGB 通道非 0 或尺寸变化，症状正是本次修复的原问题（深色菜单栏上不可见）。

**依据**：我用脚本算法在内存中完整复算 build_mask 并与已提交 PNG 解码后的 alpha 逐像素比对：36x36、RGB 全 0、1296 像素零差异（今日一致）；仓库内 grep 未见任何引用该脚本或校验该产物的测试/CI 步骤。

**建议**：给脚本加 --check 模式（在内存中重新生成并与磁盘产物逐字节比对，不一致即非零退出），挂进现有测试或 CI；或最低成本地在测试里断言产物尺寸与 RGB 通道恒 0。

## 认为正确的部分

- delegate 生命周期处理正确：上游 objc2-user-notifications 0.3.2 的 UNUserNotificationCenter.rs 明确注明 setDelegate 为 weak property（'This is a weak property'），用进程级 OnceLock<Retained> 长期持有是正确的配套；先装 delegate 再请求授权的顺序及注释里的理由成立
- 点击回调线程安全正确：did_receive 在任意队列被调起，is_minimized/unminimize/show/set_focus 等 NSWindow 系操作全部经 app.run_on_main_thread 派发，闭包只捕获 'static 的 AppHandle，completionHandler 在当前帧同步调用避免把借用引用移进闭包——处理方式与注释论证完全一致；block2 0.6 的 Block::call 为安全函数，原帧调用无问题
- running_as_app_bundle 用 identifier 比对而非 nil 判据的论证属实：tauri-plugin-notification 2.4.0/src/desktop.rs 在 dev 模式确实 set_application("com.apple.Terminal")，nil 判据覆盖不了宿主 bundle 回落；该守卫同时正当化了 init 处对 currentNotificationCenter 抛 NSInternalInconsistencyException 的防御
- 模板图标规格正确：实测 36x36 RGBA、RGB 恒 0、形状仅由 alpha 表达；tray-icon 0.24.2 源码确认菜单栏图标硬编码 icon_height=18.0 且宽度按宽高比缩放，36px 正方形源图（18pt@2x）的选择正确规避失真；tauri Image::from_bytes 确受 image-png feature 门控，补依赖必要；show_menu_on_left_click 默认确为 true，置 false 才能放行左键事件
- objc2 0.5/0.6 双代并存策略成立：Cargo.lock 实际为 block2 0.5.1+0.6.2、objc2 0.5.2+0.6.4、objc2-foundation 0.2.2+0.3.2 并存，通知模块与剪贴板模块无类型交换；uuid(v4) 依赖在 Cargo.toml:45 本就存在
- notification_macos.rs 的 FFI 调用逐一与上游生成绑定核对一致：alloc 是 AnyThread trait 的安全方法（导入 AnyThread 正是为它，非死导入）、UNNotificationContent::new/setTitle/setBody/setSound、requestWithIdentifier_content_trigger、addNotificationRequest_withCompletionHandler、mainBundle() 非可选返回、bundleIdentifier() 返回 Option、UNNotificationDefaultActionIdentifier 为 extern static（unsafe 访问已正确包裹）、define_class! 未标 MainThreadOnly 时默认 AnyThread 适合回调线程不受保证的 delegate
- RCA 属实：notify-rust 4.18.0 → mac-notification-sys 0.6.15 的 objc/notify.m 确实使用 NSUserNotificationCenter（macOS 11 起废弃）；transfer_engine 四处把 let _ = 改为 Err 留痕正是对该静默失效路径的对症处理

## 未覆盖范围（本侧盲区）

- 未编译、未运行（审查禁写，cargo check 也会写 target/ 目录）；所有编译层面结论基于与本地 ~/.cargo/registry 上游源码的逐一签名比对，而非本地构建验证
- 未在真实 macOS 上运行验证：授权弹窗时机、通知点击唤窗、深浅色菜单栏下的模板反色效果、macOS 26 上的实际投递行为
- 「NSUserNotificationCenter 在 macOS 26 上不再投递」取自模块注释与该 API 自 macOS 11 废弃的事实，未独立查证 Apple 的移除声明
- Windows/Linux 通知路径仅读代码未做行为验证；client/package.json 中未被任何 TS 代码引用的 @tauri-apps/plugin-notification 为既有状态，不属本 diff 范围
- 图标美学（笔画粗细、图形占位相对 Apple HIG 的观感）仅通过数值与 ASCII 渲染判断，未在真实菜单栏目视确认
- GLM-01 中 10.14/10.15 上未知 bit 导致前台抑制的后果是从位掩码语义与 API 可用性推断的，未实测

## 实际查阅的项目文件

- `AGENTS.md`
- `CLAUDE.md`
- `README.md`
- `.reviews/macos-tray-notification/code/_meta/changes-20260914T032856Z.diff`
- `client/scripts/gen-tray-icon.py`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/tauri.conf.json`
- `client/src-tauri/icons/tray-macos.png`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/platform/mod.rs`
- `client/src-tauri/src/platform/notification.rs`
- `client/src-tauri/src/platform/notification_macos.rs`

> 编排器从工具轨迹中记录到的读取次数：{"read":11,"grep":6,"glob":2,"run_command":0,"project_reads":13}

---

*本文档由 TriviumCode 编排器从 `glm` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
