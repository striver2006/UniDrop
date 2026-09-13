---
schema: trivium.review.v1
topic: local-store-account-scope
stage: plan
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260913T103846Z
rerun_index: 1
snapshot_hash: sha256:0a2fa47536c17e15b7dca642811e480455f38bb2449c8ba22440a5fb9a4181f7
blind: true
started_at: 2026-09-13T10:38:47.686Z
finished_at: 2026-09-13T10:51:34.010Z
duration_s: 145
tool_calls:
  read: 14
  grep: 4
  glob: 0
  run_command: 0
  project_reads: 12
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 1
  major: 2
  minor: 2
  nit: 0
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: 48c335a3-1d1c-4f45-baca-19fe0b6595d2
---

# 计划审查：local-store-account-scope

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260913T103846Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

最危险的问题是 select_prune_candidates 中计算保留集的子查询遗漏了 account_id 条件，会导致另一账号活跃时当前账号历史被当成超限全部删光（AGY-01）。
此外，计划步骤 8 将 record_task 调用点错记在 transfer_engine.rs，漏掉了真正的生产调用点（AGY-02）；且未规划切账号后的前端历史重拉机制，导致主界面残留显示旧账号记录（AGY-03）。
保留策略的作用域划分、冷启动全局复位与启动期首次认领时机整体成立，但计划需修复上述逻辑漏洞后再行实施。

**结论**：`request-changes`

## 审查意见（共 5 条：阻断 1 ｜ 重要 2 ｜ 次要 2）

### AGY-01 · 阻断（blocker）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.2 (对应 client/src-tauri/src/storage/history_repo.rs:215-219)` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：select_prune_candidates 内部用于计算保留集的子查询未规划 account_id 过滤。若子查询保持全局，另一账号产生新记录后，当前账号的记录将被挤出全局前 N 名，导致当前账号在修剪时所有历史被无差别全部删光，直接击穿 §2.1 保留策略按账号分区的初衷。

**依据**：查阅 plan §2.2 与 client/src-tauri/src/storage/history_repo.rs:215-219，原 SQL 中保留集判定为 t.session_id NOT IN (SELECT session_id FROM transfer_tasks ORDER BY created_at DESC, rowid DESC LIMIT ?1)。若该子查询未按 account_id 过滤，其它账号的活跃记录将霸占子查询保留位，致使当前账号所有记录被误判为超限。

**建议**：明确改写 select_prune_candidates 的两处 SQL：不仅主表过滤加 t.account_id = ?，子查询计算保留集也必须加 WHERE account_id = ?；并在单测计划中补充跨账号修剪负例。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4 步骤 8 (对应 client/src-tauri/src/commands/clipboard_cmd.rs:254 与 client/src-tauri/src/lib.rs:194)` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | high |

**问题**：计划实施步骤 8 声称在 transfer_engine.rs 补 record_task 的账号参数，但这属于事实错误。该文件中并无调用点，真正的生产调用点位于 clipboard_cmd.rs 和 lib.rs，指引错误将导致实施者扑空且遗漏真实改动点。

**依据**：全仓检索 HistoryRepo::record_task，生产调用点仅存在于 client/src-tauri/src/commands/clipboard_cmd.rs:254（发送端）与 client/src-tauri/src/lib.rs:194（接收端），client/src-tauri/src/core/transfer_engine.rs 中并无任何 record_task 调用。

**建议**：将步骤 8 更正为在 clipboard_cmd.rs（发送端）和 lib.rs（接收端）中传入 account_id，并注明分别从 state.settings 与 settings_for_actor 获取。

### AGY-03 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4 步骤 3 与 plan §7 (对应 client/src/components/HistoryPanel.tsx:48-50 与 client/src-tauri/src/commands/settings_cmd.rs:215)` |
| 类别 | scope-drift ｜ 层次 plan |
| 置信度 | high |

**问题**：切换账号后未规划前端历史面板的主动重拉机制。当切换到新账号且新账号历史数量无需修剪时（pruned = 0），后端不发 history-pruned 事件，前端历史面板仍将残留显示前一账号的历史卡片与文件名摘要，与计划 §7 的验证预期不符。

**依据**：查看 client/src/components/HistoryPanel.tsx:48-50，HistoryPanel 仅监听 history-pruned；而 client/src-tauri/src/core/history_pruner.rs:38-44 中 Ok(0) => {} 不广播事件；client/src/App.tsx:225-234 的 handleSaveSettings 在保存后也未重新请求历史。

**建议**：在计划中增加账号变更通知机制（如 cmd_save_settings 保存后广播事件，或前端保存设置时直接触发 HistoryPanel 刷新），确保切账号后界面历史立即呈现当前账号数据。

### AGY-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §1 表格与正文 (对应 client/src-tauri/src/commands/clipboard_cmd.rs:353 与 client/src-tauri/src/commands/history_cmd.rs:26,94)` |
| 类别 | security ｜ 层次 plan |
| 置信度 | high |

**问题**：cmd_inject_session、cmd_save_transfer_as 与 cmd_reveal_session 是前端直接调用的 IPC 命令。计划未对其规划账号校验，若前端界面残留或被传入旧账号 session_id，用户仍可跨账号装载或导出前一账号的缓存文件，缺乏纵深防御。

**依据**：查阅 client/src-tauri/src/commands/clipboard_cmd.rs:353 与 client/src-tauri/src/commands/history_cmd.rs:26,94，三个 IPC 命令均直接通过前端传入的 session_id 读文件/写剪贴板/另存为，未校验 session_id 是否归属当前登录账号。

**建议**：在三个命令执行核心操作前，增加对 session 归属账号与当前 account_id 的一致性核验，防止越权装载或导出。

### AGY-05 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3 (对应 client/src-tauri/src/commands/settings_cmd.rs:470-487 与 client/src-tauri/src/lib.rs:53-66)` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | medium |

**问题**：启动迁移认领逻辑未防御 initial_settings.account_id 为空串等非法值的情况。若老库反序列化得到非法账号，存量历史被认领给空串账号后，后续用户在 UI 上更正为合法账号时，由于切账号路径不认领，存量历史将永久不可见。

**依据**：查阅 client/src-tauri/src/commands/settings_cmd.rs:470-487，老库升级时 account_id 允许为空串且能反序列化成功；若此时在 lib.rs 执行 UPDATE，会将所有历史打上空串账号。

**建议**：在计划中增加认领的前置校验：仅当 validate_account_id(&initial_settings.account_id).is_ok() 时才执行 UPDATE 认领。

## 认为正确的部分

- 三个保留策略的作用域划分（§2.1）站得住脚：history_max_entries 设为每账号能有效避免活跃账号冲垮低频账号历史；cache_max_size_mb 保持全局符合物理磁盘总配额现实；cache_ttl_hours 全局淘汰符合时间维度的通用生存期。
- 冷启动全局执行 reset_stale_in_flight（§2.3）成立：冷启动阶段无真实活跃任务，全表将非终态在途死行置为 FAILED 能彻底防止非当前账号的残留死行发生状态永久挂起与额度泄漏。
- 数据模型仅给 transfer_tasks 加 account_id（§2.2）设计合理：transfer_items、chunk_bitmaps、cache_entries 均能由 session_id 唯一确定父任务归属，避免了多表反范式冗余及多处维护的一致性风险。
- 存量历史认领时机（§3）放在 lib.rs 启动读取 settings 之后、且不在 cmd_save_settings 重复认领的决策正确，杜绝了后续切账号时将旧账号历史错搬给新账号的风险。
- 遗留加固一（connection_actor.rs 中 create_tls_connector 内部防御性设置 default_provider）与遗留加固二（USER_GUIDE.md 针对 LibreSSL 默认生成 v1 证书的 X.509 v3 详细指引）均严密准确，质量优良。

## 未覆盖范围（本侧盲区）

- Windows 与 Linux 特定平台下高频清理缓存文件的文件锁并发争用细节。
- 传输卡片（TransferProgress 组件）在账号切换时的前端保留或清空生命周期。
- Phase 3 计划中的 E2EE 本地密钥与配对表迁移。

## 实际查阅的项目文件

- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src-tauri/src/core/history_pruner.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/commands/history_cmd.rs`
- `client/src/components/HistoryPanel.tsx`
- `client/src/App.tsx`
- `client/src-tauri/src/core/connection_actor.rs`
- `docs/USER_GUIDE.md`
- `client/src-tauri/src/commands/clipboard_cmd.rs`

> 编排器从工具轨迹中记录到的读取次数：{"read":14,"grep":4,"glob":0,"run_command":0,"project_reads":12}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
