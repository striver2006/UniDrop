---
schema: trivium.review.v1
topic: history-retention-autodismiss
stage: plan
role: glm
vendor: zcode
agent_cli: zcode/0.16.5
model_requested: GLM-5.3-Flash
model_effective: bigmodel-coding-plan/GLM-5.3-Flash
model_effective_source: trace
model_route: config_copy
run_id: 20260912T234849Z
rerun_index: 1
snapshot_hash: sha256:9fbbbdfabc637da1b7341feaf8528717574ef1d72d17019ecfc67b487daa030d
blind: true
started_at: 2026-09-12T23:48:50.516Z
finished_at: 2026-09-13T00:00:58.058Z
duration_s: 728
tool_calls:
  read: 18
  grep: 6
  glob: 0
  run_command: 0
  project_reads: 18
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 1
  minor: 3
  nit: 2
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: sess_0e8403b5-bf96-4577-86cc-3293a4c94059
---

# 计划审查：history-retention-autodismiss

> Critic-B · 智谱 ZCode ｜ 模型 `bigmodel-coding-plan/GLM-5.3-Flash`
> ｜ 运行 `20260912T234849Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

计划整体扎实：§1 的现状核验逐条属实，serde default、事务删行、stale closure 等高危点都已预判。最危险的一条：保存设置触发的修剪不会刷新已挂载的历史列表，计划自己的验收步骤 §4.3-2（保存后「立即」只剩 3 条）按 §3 改动清单无法达成，需补一条前端刷新机制。其次，「终态是唯一条数增长时刻」的前提不成立：历史行在任务开始时即以 TRANSFERRING 落库，崩溃遗留行永不被修剪也无启动复位，会永久占用保留额度。终态触发点还遗漏 lib.rs 的 TRANSFER_FAILURE 分支。其余为 FAILED 卡片自动消失未决策与两处校验/失败路径小项。

**结论**：`request-changes`

## 审查意见（共 6 条：重要 1 ｜ 次要 3 ｜ 吹毛求疵 2）

### GLM-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.5/§3.1、§4.3 步骤 2（对照 client/src/components/HistoryPanel.tsx:26-49）` |
| 类别 | test-gap ｜ 层次 plan |
| 置信度 | high |

**问题**：修剪在 cmd_save_settings 内执行后，前端已挂载的 HistoryPanel 不会感知。它的刷新点只有三处：挂载时、收到终态 transfer-progress 事件时、手动点刷新按钮；App.tsx 的 handleSaveSettings 只 setSettings 并重新拉取设备/设置，不重拉历史。按 §3 改动清单实施后，保存「保留条数=3」时后端已删、列表仍显示旧长度，要等下次传输终态或手动刷新才变短——§4.3 步骤 2 的「历史列表立即只剩 3 条」无法达成，§2.5 触发点 2 的动机（「立刻见效，否则看起来像设置没保存」）落空。

**依据**：读 HistoryPanel.tsx:26-49（refresh 仅由 mount / listen(transfer-progress 终态) / 按钮触发）、App.tsx:151-166（保存后无历史重拉，fetchInitialData 只拉设备与设置）、settings_cmd.rs:44-96（cmd_save_settings 仅 emit devices-updated，无修剪通知）。

**建议**：在 §3 补一个前端感知机制并指定落点：后端修剪发生后 emit 事件（仿 devices-updated 模式）由 HistoryPanel 监听静默刷新，或在保存设置成功的回调链上触发 HistoryPanel 的静默 refresh。

### GLM-02 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.5 前提句、§2.4 约束 A（对照 client/src-tauri/src/commands/clipboard_cmd.rs:149、lib.rs:193）` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：「任务落到终态之后是唯一会让条数增长的时刻」不成立：SEND/RECEIVE 历史行在任务开始时即以 TRANSFERRING 落库（clipboard_cmd.rs:149、lib.rs:193）。进程在传输中途被杀或崩溃时，该行永远停在 TRANSFERRING：lib.rs setup 无任何启动期状态复位逻辑，且按约束 A 每轮修剪都跳过它。这类死行会永久占用「最新 N 条」额度并让库体积无上限增长——恰是需求 4 要收敛的对象。另核实本代码库不存在实际续传路径（BitmapRepo 仅被自身测试引用，无生产调用点，chunk_bitmaps 无写入方），启动期把陈旧非终态行复位为 FAILED 不会破坏任何在途状态。

**依据**：读 clipboard_cmd.rs:141-149、lib.rs 全文（setup 无复位逻辑）、history_repo.rs:36-77（record_task 以 TRANSFERRING 插入）；Grep 全仓 bitmap_repo/BitmapRepo 仅命中 storage/bitmap_repo.rs 自身与 storage/mod.rs 再导出。

**建议**：计划补一条决策：启动修剪前把 TRANSFERRING/PENDING 行复位为 FAILED（无续传路径，复位安全），并修正 §2.5 的增长时刻表述；若不做复位，也应写明死行永久占额度的已知取舍。

### GLM-03 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.3（对照 client/src-tauri/src/lib.rs:255-296）` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：§2.5 说修剪挂在「update_task_status 到终态的调用点」，但 §3.3 只点名 transfer_engine.rs。lib.rs 的 TRANSFER_FAILURE 分支（对端报告失败，lib.rs:274）同样写 FAILED 终态，按 §3.3 实施会漏掉它，该路径产生的终态行要等下一次保存设置、传输或重启才被修剪。

**依据**：读 lib.rs:255-296（TRANSFER_FAILURE 处理器内调用 HistoryRepo::update_task_status(..., "FAILED", ...)）；transfer_engine.rs:51-57（update_history_status 是另一组终态调用点的公共入口）。

**建议**：§3.3 把触发点改为「全部终态写入点」并枚举两处（transfer_engine 的 update_history_status 调用簇 + lib.rs TRANSFER_FAILURE 分支），或说明统一收敛到某个终态写入口再触发。

### GLM-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.1/§2.7/§3.6（对照 docs/需求.md 第 5 条、client/src/components/TransferProgress.tsx:60-79）` |
| 类别 | correctness ｜ 层次 requirement |
| 置信度 | medium |

**问题**：计划把自动消失无差别应用于一切终态卡片（COMPLETED 与 FAILED），但需求原文只说「传输任务完成后」。失败卡片约 30 秒后自动消失，而错误 toast 仅 4 秒且只出现一次，失败原因最醒目的入口就没了（历史列表仍有失败条目，但视觉权重低得多）。计划没有记录这个取舍，实现后若产品上不接受需要返工。

**依据**：读 docs/需求.md:14（第 5 条原文）、App.tsx:45-50 与 117-121（toast 4 秒、失败仅一次提示）、TransferProgress.tsx:60-79（失败卡片样式与手动移除按钮）、types/index.ts:26（前端终态枚举仅 COMPLETED/FAILED）。

**建议**：在计划中显式决策并写明理由：FAILED 卡片是否豁免自动消失（仅 COMPLETED 计时，或 FAILED 用更长保留/仅手动关闭均可），留痕即可。

### GLM-05 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.5（对照 client/src/components/SettingsModal.tsx:79-103）` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：校验清单（空串/非数字/负数）未覆盖非整数与超上限：输入「2.5」会以 JSON 2.5 提交，Rust 侧 u32 反序列化失败，cmd_save_settings 返回笼统的「保存设置失败」且无字段级提示；「99999999999」（超 u32::MAX）同理。计划未约定 number input 加 step=1，浏览器默认允许小数。

**依据**：读 SettingsModal.tsx:79-103（handleSubmit 直接透传 form 数值，无整数/上限校验）、settings_cmd.rs:44-64（反序列化失败即整单 Err）、types/index.ts:9-17（AppSettings 数值字段定义）。

**建议**：§3.5 校验收紧为「0 到 4294967295 的非负整数」，input 加 min=0 step=1；或由后端对两字段做范围校验并返回字段级错误信息。

### GLM-06 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.3/§3.2（对照 client/src-tauri/src/core/cache_manager.rs:104-165）` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | medium |

**问题**：计划的顺序是「事务内删行 → 调用方删文件」。若某个 remove_file 失败（文件被占用/权限），行已提交删除，该文件成为孤儿；而既有 sweep 以 cache_entries 表行为遍历源，不会再回收无行文件。这与既有 sweep「忽略删除失败」的容忍度同构，方向本身可接受，但计划未写明单文件删除失败时的处理与后果。

**依据**：读 cache_manager.rs:104-165（sweep 先删文件后删行、错误均忽略，且以表行为遍历源）；plan §2.3/§3.2 规定行删先于文件删且文件删除在事务外。

**建议**：§3.2 写明：单文件删除失败仅记 warn 日志、不影响事务结果（与既有 sweep 一致），并注明该文件将成为行级 sweep 不会回收的孤儿。

## 认为正确的部分

- §1 的现状核验逐条属实：cmd_list_history 硬编码 100（history_cmd.rs:12）、外键未开启致级联不发生（db.rs:15、db.rs:55）、update_task_status 静默 0 行更新（history_repo.rs:86-93）、sweep 只按时间/容量（cache_manager.rs:104-165）、设置整份 JSON 单行存储与 start_minimized 的 serde 教训（settings_cmd.rs:6-34、lib.rs:53-66）。个别行号有两三行漂移，不影响结论。
- §2.2 的分析正确且必要：u32 的 Default=0 与「0=不限制」语义相撞，裸 #[serde(default)] 会让老库升级后两个需求静默失效；共用常量 + §4.2 的红测试是正确的守护方式。
- §2.3/§3.2 的分层（HistoryRepo 保持纯 SQL、返回路径交 CacheManager 删文件）与现有架构一致；不开 PRAGMA foreign_keys 的取舍我认同——record_task 的 INSERT OR REPLACE 在 FK 开启时会级联清掉 transfer_items 子行，风险判断准确。
- §2.4 约束 B 与既有 sweep 的免疫判据一致（cache_manager.rs:109-111），触发点够密（终态/保存/启动），TEXT/IMAGE 完成即注入即免疫意味着突发场景下修剪最多延迟约 2 小时后收敛，取舍成立；触发点取锁模式可行（db_conn 是 Arc<Mutex<Connection>>，各触发点既有 guard 均在块/函数作用域内释放，修剪再取锁不会自锁）。
- §3.6 的 stale closure 分析正确：transfer-progress 监听器注册在空依赖的 useEffect（App.tsx:74-149），闭包读 settings 永远拿到 defaultSettings，useRef 方案是对的；§3.5 指出的 Number("")===0 空串陷阱也是真实的。
- §2.6 对「界面提醒」的解读（指历史列表条目本身，删条目即消失、不弹 toast）是需求括号文本最自洽的读法，且与约束 B 的静默跳过规则自洽。

## 未覆盖范围（本侧盲区）

- 只读审查，未运行 cargo test / npm build / tauri dev，§4 测试计划本身的可执行性未实测。
- 未通读 clipboard_cmd.rs 全文（仅核验 record_task 调用点与锁模式相关行）、SendModal.tsx、connection_actor.rs、platform/*；server/（Go 中继）与本需求无关，未审。
- 未核实 main.tsx 是否启用 StrictMode——§2.7 的双挂载论据未验证，但其清理逻辑无论 StrictMode 与否都正确。
- Windows/Linux 上 remove_file 失败模式（文件占用）未做平台差异评估。
- 另一位审查员的结论按背靠背规则未读、未引用。

## 实际查阅的项目文件

- `.reviews/history-retention-autodismiss/plan/2026-09-13-history-retention-autodismiss-claude.md`
- `CLAUDE.md`
- `README.md`
- `docs/需求.md`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/commands/history_cmd.rs`
- `client/src-tauri/src/commands/clipboard_cmd.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/storage/bitmap_repo.rs`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/app_state.rs`
- `client/src/App.tsx`
- `client/src/types/index.ts`
- `client/src/components/SettingsModal.tsx`
- `client/src/components/TransferProgress.tsx`
- `client/src/components/HistoryPanel.tsx`

> 编排器从工具轨迹中记录到的读取次数：{"read":18,"grep":6,"glob":0,"run_command":0,"project_reads":18}

---

*本文档由 TriviumCode 编排器从 `glm` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
