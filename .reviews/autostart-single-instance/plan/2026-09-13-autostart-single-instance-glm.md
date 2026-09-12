---
schema: trivium.review.v1
topic: autostart-single-instance
stage: plan
role: glm
vendor: zcode
agent_cli: zcode/0.16.5
model_requested: GLM-5.3
model_effective: bigmodel-coding-plan/GLM-5.3
model_effective_source: trace
model_route: config_copy
run_id: 20260912T171855Z
rerun_index: 1
snapshot_hash: sha256:69f749fe00fb39863157477872b094302ca55bc36ce7ebaeb10d42b7fddf4b7e
blind: true
started_at: 2026-09-12T17:18:56.540Z
finished_at: 2026-09-12T17:26:53.377Z
duration_s: 477
tool_calls:
  read: 17
  grep: 2
  glob: 1
  run_command: 0
  project_reads: 19
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 1
  minor: 4
  nit: 2
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: sess_913d362e-e62e-4d9f-83b2-9d2c29636bdf
---

# 计划审查：autostart-single-instance

> Critic-B · 智谱 ZCode ｜ 模型 `bigmodel-coding-plan/GLM-5.3`
> ｜ 运行 `20260912T171855Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

方向正确、现状核验全部属实（逐条对了 lib.rs、Cargo.toml、tauri.conf.json、db.rs、DESIGN.md、client-ci.yml 的引用），§3.6 的 serde(default) 兼容性分析是全篇最有价值的一条。最危险的是保存写路径的契约缺口：cmd_save_settings 是全量单命令保存，计划只规定 autostart 字段「OS 成功才落库、失败返回 Err」，未决定 OS 失败对其余字段（server_url/PSK 编辑）的影响，且 §4.2 前端清单没有任何承接该错误的改动——现有 SettingsModal 提交后立即关窗且不 await onSave，'不能静默吞掉'的意图落不了地。其次：写路径不与 OS 现值比对（幂等未定义、会话内外部关闭可能被陈旧提交值复活）；start_minimized 文案与 §3.2 真值表自相矛盾；两处测试缺口。全部可在计划层用几句话决策解决，无 blocker。

**结论**：`request-changes`

## 审查意见（共 7 条：重要 1 ｜ 次要 4 ｜ 吹毛求疵 2）

### GLM-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.1（写路径）+ §4.2；client/src-tauri/src/commands/settings_cmd.rs:21-73；client/src/components/SettingsModal.tsx:27-33；client/src/App.tsx:150-163` |
| 类别 | contract ｜ 层次 plan |
| 置信度 | high |

**问题**：cmd_save_settings 一次性保存整个 AppSettings（SQLite 全量 JSON、内存态、actor 配置、清空设备表、触发重连）。计划要求 OS 调用失败时「向前端返回明确错误」且「成功后才把该字段写进 SQLite」，但没有决定失败时其余字段怎么办：(a) 若整体中止保存，用户同一次提交里的 server_url/PSK 编辑也被丢弃，只剩一条笼统的「保存设置失败」toast；(b) 若仅 autostart 字段被跳过而其余照存，命令仍返回 Err，App.tsx:153 的 setSettings 不会执行，UI 与已落库的 DB 状态脱节。且 §4.2 前端改动清单只有加两个 checkbox，没有任何错误承接改动：SettingsModal.handleSubmit 调 onSave 后无条件 onClose（不 await），错误只能在弹窗关闭后以脱离上下文的 toast 出现。

**依据**：读 settings_cmd.rs 全文确认保存流程为全量单命令；读 SettingsModal.tsx:21-34 确认提交即关窗、onSave 是 fire-and-forget；读 App.tsx:150-163 确认错误处理只有通用 toast 且 setSettings 仅在成功分支执行。计划 §3.1/§4.2 原文如上，未覆盖该组合语义。

**建议**：在 §3.1 写明部分成功的取舍并同步 §4.2：要么「OS 失败则整体中止保存、弹窗保持打开/表单回滚」，要么把自启开关拆成独立命令与表单保存解耦，使两个失败域互不污染。给出验收判据（失败后界面各字段应显示什么）。

### GLM-02 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.1（写路径）；client/src/App.tsx:60-64` |
| 类别 | concurrency ｜ 层次 plan |
| 置信度 | medium |

**问题**：写路径「按新值调用 enable()/disable()」不与 OS 现值或旧值比对，两个后果未决策：一是普通保存（如只改 PSK）也会触发 OS 写，disable() 在注册项本就不存在时是否报错取决于 auto-launch crate 行为，可能产生虚假的保存失败；二是会话中途用户在系统设置里关闭自启后，前端 settings 仅在启动时拉取一次（App.tsx fetchInitialData），弹窗显示陈旧值，用户保存无关编辑会以陈旧的 true 调 enable()，把用户刚在 OS 层做的关闭悄悄复活——这正是 G4 声称要防的漂移，但读时覆盖防不了这条写路径。

**依据**：settings_cmd.rs:15-18 确认读自内存态；App.tsx:60-64 确认设置只在启动/保存后拉取；计划 §3.1 写路径原文只按「新值」调用，无比对逻辑；§6.2 第 7 条只测读路径。

**建议**：写路径先查 OS 现值，仅在与提交值不一致时调用 enable/disable，并把「目标态==现值」视为成功；或至少在计划中写明无变化时不触发 OS 写。补充一条「会话中途外部关闭后再保存无关设置」的手工用例。

### GLM-03 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.2 真值表 vs §4.2 UI 文案` |
| 类别 | correctness ｜ 层次 requirement |
| 置信度 | high |

**问题**：§3.2 明确「手动启动永远显示窗口」（start_minimized 对手动启动无效，is_autostart_launch 只认 --silent），因此 autostart_enabled 关闭时 start_minimized 是纯占位开关；但 §4.2 文案「启动后不弹出主窗口，仅在系统托盘常驻」无条件承诺不弹窗，括注「手动启动也可能想最小化」更与真值表直接矛盾。实现者无论跟哪一边，另一边就是错的；用户开着该开关双击图标仍会弹窗，与文案不符。

**依据**：计划 §3.2 真值表与结论段、§3.3 的 is_autostart_launch 定义、§4.2 文案与括注原文相互冲突；§6.2 也没有「手动启动 + start_minimized=true」的用例来锚定期望行为。

**建议**：先定语义（推荐维持 §3.2，最不惊讶），再把文案改为「开机自启拉起时不弹出主窗口」并在开关副标题说明仅对自启生效；§6.2 补一条手动启动 + 开关开启 → 仍显示窗口的用例。

### GLM-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.5 / §6.2` |
| 类别 | test-gap ｜ 层次 plan |
| 置信度 | high |

**问题**：§3.5 新增的「第二实例带 --silent → 早退、保持窗口现状」是本轮单实例加固唯一的新分支，但 §6.2 的 9 条手工用例（第 8、9 条）都只覆盖不带 --silent 的第二实例，该分支零覆盖。

**依据**：对照计划 §6.2 清单逐条核对，无一条以带 --silent 的参数再次启动运行中的应用。

**建议**：§6.2 增加一条：应用可见运行中，从终端以 --silent 再次启动 exe → 不产生第二进程且窗口不抢焦点；应用隐藏时同样操作 → 保持隐藏。

### GLM-05 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.6 / §6.1` |
| 类别 | test-gap ｜ 层次 plan |
| 置信度 | high |

**问题**：§3.6 识别出的老 JSON 反序列化失败 → unwrap_or_else 静默冲掉 server_url/psk 是本轮风险最高的一条（计划自己称之为「静默的用户数据损坏」），但 §6.1 的自动化只有 core::startup 单测，兼容性仅靠 §6.2 第 2 条手工验证。将来任何人移除 #[serde(default)]（或加 deny_unknown_fields）只有手工测试能拦住。

**依据**：读 lib.rs:35-44 确认 unwrap_or_else 回落路径真实存在；读 §6.1 清单确认无任何针对 AppSettings 反序列化的自动化测试。

**建议**：§6.1 增加一个 #[cfg(test)]：用改动前格式的 JSON 字面量（无新字段）反序列化 AppSettings，断言新字段取默认值且旧字段保真。

### GLM-06 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4.1 capabilities 行 vs §4.2` |
| 类别 | security ｜ 层次 plan |
| 置信度 | high |

**问题**：§4.2 明确「前端不直接调该插件的 JS API，全部经由 cmd_get/save_settings」，则 capabilities 里追加 autostart:default 是无人使用的授权——它放开的是 webview→插件 IPC 命令面（enable/disable/is-enabled），与自述设计矛盾且纯增攻击面（一旦 webview 被注入，可借此注册自启持久化）。Rust 侧经 manager 调用不需要该权限。

**依据**：读 capabilities/default.json 确认现有权限均为前端实际使用的项；计划 §4.1/§4.2 原文如上。

**建议**：删去该条，或在计划中说明为何保留（如近期就会接 JS API）；保持「能力只授予实际使用者」的现状原则。

### GLM-07 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan G2 / §4.2 文案（macOS）` |
| 类别 | correctness ｜ 层次 requirement |
| 置信度 | medium |

**问题**：「只在系统托盘常驻」在 macOS 上不成立：应用未设置 ActivationPolicy::Accessory（lib.rs 全文无 set_activation_policy，tauri.conf.json 亦无相关配置），默认 Regular 策略下隐藏窗口的应用仍有 Dock 图标并可 cmd-tab 唤起。用户开启「启动时最小化」后会在 Dock 看到图标，与文案预期有观感落差。

**依据**：通读 lib.rs 与 tauri.conf.json 确认未配置激活策略；基于 macOS 默认行为推断，未实测。

**建议**：在 §5 的 macOS 行注明此差异并明确取舍：接受 Dock 图标，或后续轮次评估 Accessory 策略（注意 Accessory 对手动启动场景的影响）。至少让验收者知道 Dock 图标存在不算失败。

## 认为正确的部分

- §1 现状核验全部属实：Cargo.toml:20 已依赖 single-instance 且 lib.rs:94-100 为首个插件；tauri.conf.json:23 visible:true；lib.rs:317-330 剪贴板监听仅 macOS、platform/mod.rs:176-181 三端导出（L1 属实）；rate_limit_mb 仅 5 处声明/初始化无消费点（L2 属实）；DESIGN.md:1282/1290 引用准确。
- §3.6 的 serde(default) 兼容性分析正确且关键：AppSettings 现无任何 default 属性（settings_cmd.rs:5-12），旧 JSON 反序列化失败会走 lib.rs:35-44 的 unwrap_or_else 静默全量回落，冲掉用户已存配置；加字段必须带 #[serde(default)] 的结论成立，且 serde 默认忽略未知字段使降级回旧版本也安全。
- OS 为事实源、读时覆盖、写后落库的方向（§3.1 读路径）正确，is_enabled 失败保留 SQLite 值的兜底合理。
- §3.4 visible:false + 托盘逃生口成立：托盘构建（lib.rs:332-386）确实先于显隐判定（388-391）。
- 显隐判定抽成纯函数并让 setup 与 second-instance 共用（§3.3）、second-instance 用 args 且窗口缺失记告警（§3.5）都是正确的加固。
- 不做安装包写注册表以规避双写冲突（§2 非目标）与 §4.3 对 client-ci.yml 已跑 cargo check/test 的描述（client-ci.yml:53-59）均属实且判断合理。
- 两处默认值字面量（lib.rs:37-43、L50-56）确实重复，顺手抽 default_config() 的范围判断恰当。

## 未覆盖范围（本侧盲区）

- .trivium/config.yaml 的 verify.build/verify.test 现状因禁读清单（.trivium/ 全目录）无法核实，§4.3 的前提仅采信文档自述。
- 未实际运行 cargo/pnpm/go 构建与测试（只读审查），所有判断基于源码静态阅读。
- tauri-plugin-autostart 2.0.0 底层 auto-launch crate 的平台细节（plist 精确路径、disable 对不存在项的返回值、tauri-plugin-single-instance 2.0.0 在 Linux 的支持情况）基于经验判断，未离线验证版本行为。
- 三端真机手工回归（§6.2）未执行；UI 视觉与交互细节未评审。
- 按背靠背规则，未考虑另一侧审查员的视角；安全评审（杀软误报、代码签名）超出本轮范围，计划已列为非目标。

## 实际查阅的项目文件

- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/tauri.conf.json`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/core/mod.rs`
- `client/src-tauri/src/platform/mod.rs`
- `client/src-tauri/capabilities/default.json`
- `client/src/types/index.ts`
- `client/src/components/SettingsModal.tsx`
- `client/src/App.tsx`
- `docs/需求.md`
- `docs/design/DESIGN.md`
- `.github/workflows/client-ci.yml`
- `CLAUDE.md`
- `README.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":17,"grep":2,"glob":1,"run_command":0,"project_reads":19}

---

*本文档由 TriviumCode 编排器从 `glm` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
