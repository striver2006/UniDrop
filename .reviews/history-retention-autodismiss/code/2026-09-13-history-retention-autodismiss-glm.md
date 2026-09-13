---
schema: trivium.review.v1
topic: history-retention-autodismiss
stage: code
role: glm
vendor: zcode
agent_cli: zcode/0.16.5
model_requested: GLM-5.3-Flash
model_effective: bigmodel-coding-plan/GLM-5.3-Flash
model_effective_source: trace
model_route: config_copy
run_id: 20260913T002939Z
rerun_index: 1
snapshot_hash: sha256:84298a27d52ec42a7e70cc95302eb61fbcaa46b8076a180bfccb162b8facb6ff
blind: true
started_at: 2026-09-13T00:29:40.556Z
finished_at: 2026-09-13T00:39:50.758Z
duration_s: 610
tool_calls:
  read: 21
  grep: 1
  glob: 0
  run_command: 0
  project_reads: 19
evidence_ok: true
verdict: approve-with-nits
findings_count:
  blocker: 0
  major: 0
  minor: 2
  nit: 0
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: sess_62e4c799-1723-45f2-bdcd-70366e7ac3e4
---

# 代码审查：history-retention-autodismiss

> Critic-B · 智谱 ZCode ｜ 模型 `bigmodel-coding-plan/GLM-5.3-Flash`
> ｜ 运行 `20260913T002939Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

四个焦点全部核实成立：锁时序（六处 finalize + lib.rs 两处块后触发 + cmd_save_settings 各锁独立块）无死锁路径；删除顺序（先文件后行、失败剔除、四表先子后父单事务）与 sweep 同序且外键论证属实；FAILED 卡片豁免前后端一致；history-pruned → HistoryPanel 静默刷新链路完整。两条 minor：修订计划 §3.8 承诺的「文件删除失败保留该会话行」单测未落地，prune_history 删文件段零测试覆盖；三段式修剪在无锁 IO 窗口内可与用户「装载」交错，免疫标记只在选取段判定一次，删行段不复核，存在 TOCTOU。

**结论**：`approve-with-nits`

## 审查意见（共 2 条：次要 2）

### GLM-01 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/cache_manager.rs:128-153（prune_history 删文件段）` |
| 类别 | test-gap ｜ 层次 code |
| 置信度 | high |

**问题**：修订计划 §3.8 明确承诺增补「某文件删除失败时，该会话的行被保留而非一并删除」的单测，但实现中不存在：cache_manager.rs 全文没有任何 #[cfg(test)] 模块，history_repo.rs 新增的 9 个测试全部只覆盖三段式的第 1 段（select_prune_candidates）与第 3 段（delete_sessions），第 2 段的 all_removed 判定、失败剔除、deletable 为空时提前返回这条关键容错路径完全无测试守护。

**依据**：读 .reviews/history-retention-autodismiss/plan/2026-09-13-history-retention-autodismiss-revised-claude.md §3.8（第 207 行）与 §2 裁决表 GLM-06 行；读 client/src-tauri/src/core/cache_manager.rs 全文（无 tests 模块）；读 client/src-tauri/src/storage/history_repo.rs 测试区（L335-L523，均为纯 SQL 层测试，无文件 IO 场景）。

**建议**：为 CacheManager::prune_history 补一条带临时目录的单测：构造两个候选会话，令其中一个的登记路径指向必然 remove_file 失败的目标（如一个目录路径），断言失败会话的行保留、成功会话的行被删，且返回值为 1。方向即可，不必在本评审内给出补丁。

### GLM-02 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/cache_manager.rs:128-157（prune_history 第 2 段无锁窗口与第 3 段删行之间）` |
| 类别 | concurrency ｜ 层次 code |
| 置信度 | medium |

**问题**：剪贴板免疫（约束 B）只在第 1 段选取时判定一次；第 2 段删文件期间 db_conn 锁已释放，此时用户若在 HistoryPanel 对某个候选会话点「装载」（cmd_inject_session 读文件 → 写剪贴板 → mark_clipboard_injected），免疫标记落在选取之后，而第 2、3 段均不复核：文件被删、行也被删。FILES 会话的剪贴板持有的是指向已删路径的文件引用，粘贴失败，且历史条目在用户刚使用后消失，不可自愈。既有 sweep_expired_and_lru 因 conn guard 贯穿全函数而天然免疫此交错；三段式为避免持锁做 IO 重新引入了这个窗口。

**依据**：读 client/src-tauri/src/core/cache_manager.rs:117-158（第 2 段注释明言不持锁；sweep_expired_and_lru L161-222 的 guard 贯穿全函数）；读 client/src-tauri/src/storage/history_repo.rs:197-243（免疫 NOT EXISTS 仅存在于选取 SQL）；读 client/src-tauri/src/commands/clipboard_cmd.rs:212-252（cmd_inject_session 全程不与第 1 段互斥，mark_clipboard_injected 在 L232/240/248）；读 client/src-tauri/src/commands/history_cmd.rs:26-90（save-as 对「行在文件没了」有兜底，佐证反向可自愈，而本场景行也没了）。

**建议**：第 3 段重新取锁后、删行前，对 deletable 用同一免疫判据复核一次（一条 SQL 即可），剔除窗口期内被装载的会话；复核与删行在同一连续持锁区间内完成即可关闭该窗口（mark_clipboard_injected 需要同一把锁，无法再交错）。是否值得为此加复核由 Driver 权衡，触发需要用户动作与修剪窗口重叠。

## 认为正确的部分

- 锁时序核验通过：finalize_history_status 把「修剪必须在锁释放后」固定在一处，六个终态调用点全部切换（transfer_engine.rs 278/526/540/610/839/875）；lib.rs TRANSFER_FAILURE 分支与启动收敛的 conn guard 均在显式块内释放后才调 prune_and_notify；cmd_save_settings 各锁为独立块；grep 证实 lib.rs:274 是唯一绕过 update_history_status 的生产终态写入且已配修剪，无遗漏触发点。
- 删除顺序正确且论证属实：先删文件后删行与 sweep 同序；db.rs:15 确实只开 WAL 未开 PRAGMA foreign_keys，delete_sessions 逐一显式删四表（先子后父、单事务）是必要的而非多余的；delete_sessions_clears_all_four_tables 测试真实守护这一点。
- 启动先 reset_stale_in_flight 再修剪的顺序正确，复位仅触碰 PENDING/TRANSFERRING 且不覆盖既有 error_message（COALESCE），有测试守护且与「无续传路径」的现实一致。
- serde 用 default = 函数而非裸 default 的选择正确，两条测试（缺字段取 100/30、显式 0 不被改写）都有真实失败条件，能拦截「老库静默变成不限制」的回归。
- FAILED 卡片豁免链路完整：App.tsx 只对 COMPLETED 调 scheduleAutoDismiss，TransferProgress 对终态卡片提供手动 X，SettingsModal 文案与 docs/需求.md 均写明豁免及理由，决策留痕。
- 前端刷新链路成立：prune_and_notify 只在确有删除时 emit history-pruned，HistoryPanel 常驻挂载、监听并在卸载时解绑，保存设置触发的修剪因此能即时反映到列表。
- select_prune_candidates 的排序键（created_at DESC, rowid DESC）与 list_history 逐字一致，并有 prune_candidates_never_overlap_visible_head 互斥测试守护；免疫判据（CURRENT_TIMESTAMP + strftime %s，7200s）与 sweep 既有 SQL 同一写法。
- 前端定时器按 session_id 记账防堆积、手动关闭时清定时器、settingsRef 解決 useEffect([]) 闭包陈旧值，处理细致；parseU32 把 u32 反序列化整单失败拦在前端并给字段级错误。

## 未覆盖范围（本侧盲区）

- 未执行 cargo test / cargo build / 前端构建（审查任务书禁止写命令，编译与测试结论未验证，测试是否存在是通过读源码确认的）。
- server/（Go 中继）未审计，本次变更不涉及。
- docs/design/DESIGN.md 仅核对了 diff 中的注释行变更，未通读全文核对其余章节是否需要同步。
- 前端运行时行为未做端到端验证：webview 隐藏/后台节流下 setTimeout 的触发时机、转移卡片在窗口长期隐藏时的表现未验证。
- client/src/platform/（各平台剪贴板 FFI）与 SendModal/DeviceList 未读，与本次变更无直接交集但未逐一追踪调用方。
- 启动收敛任务与连接信令循环在极端调度下（信令先于复位处理到 TRANSFER_OFFER）的理论交错未深究：复位会把刚落库的在途行短暂改写为 FAILED，终态时会被 finalize 覆盖，影响限于瞬时状态显示。

## 实际查阅的项目文件

- `.reviews/history-retention-autodismiss/code/_meta/changes-20260913T002939Z.diff`
- `.reviews/history-retention-autodismiss/plan/2026-09-13-history-retention-autodismiss-revised-claude.md`
- `CLAUDE.md`
- `README.md`
- `docs/需求.md`
- `client/src-tauri/src/app_state.rs`
- `client/src-tauri/src/commands/clipboard_cmd.rs`
- `client/src-tauri/src/commands/history_cmd.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src-tauri/src/core/history_pruner.rs`
- `client/src-tauri/src/core/mod.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src/App.tsx`
- `client/src/components/HistoryPanel.tsx`
- `client/src/components/SettingsModal.tsx`
- `client/src/components/TransferProgress.tsx`
- `client/src/types/index.ts`

> 编排器从工具轨迹中记录到的读取次数：{"read":21,"grep":1,"glob":0,"run_command":0,"project_reads":19}

---

*本文档由 TriviumCode 编排器从 `glm` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
