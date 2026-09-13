---
schema: trivium.review.v1
topic: history-retention-autodismiss
stage: plan
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260912T234849Z
rerun_index: 1
snapshot_hash: sha256:9fbbbdfabc637da1b7341feaf8528717574ef1d72d17019ecfc67b487daa030d
blind: true
started_at: 2026-09-12T23:48:50.516Z
finished_at: 2026-09-13T00:00:58.058Z
duration_s: 124
tool_calls:
  read: 20
  grep: 1
  glob: 0
  run_command: 0
  project_reads: 17
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
session_id: af6074bd-0944-4488-af7f-ae62e2f56cc4
---

# 计划审查：history-retention-autodismiss

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260912T234849Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

最危险的问题在于并发死锁与前端状态脱节：在 TransferEngine 终态中持有全局 db_conn 互斥锁时若嵌套调用 CacheManager 修剪，因 tokio Mutex 不可重入会直接造成进程永久死锁；同时设置保存后修剪数据库，前端 HistoryPanel 缺乏刷新机制，超额历史在界面上依然残留。此外，§2.3 与 §3.2 在事务与磁盘删除顺序上自相矛盾，先删 DB 行会遗留永久孤儿磁盘文件。建议解耦锁时序、补齐前端事件通知并纠偏删除顺序后再行实施。

**结论**：`request-changes`

## 审查意见（共 5 条：阻断 1 ｜ 重要 2 ｜ 次要 2）

### AGY-01 · 阻断（blocker）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.5, §3.1, §3.5` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：保存设置触发修剪后，前端 HistoryPanel 没有任何刷新与通知机制，导致历史界面仍然显示超出上限的旧条目，违背需求 4「超出的删除（界面提醒...）」要求，且导致 plan §4.3 的手工测试第 2 步必定失败。

**依据**：核对 client/src/components/HistoryPanel.tsx:38-49，组件仅在挂载及 transfer-progress 终态事件时调用 refresh()；核对 client/src/App.tsx:52-72 与 151-166，handleSaveSettings 在 cmd_save_settings 成功后仅调用 fetchInitialData()，而该函数只获取 self_info、online_devices 和 settings，完全不更新历史列表；plan §3.1/§3.5 未规划任何向前端派发历史刷新通知的机制。

**建议**：在后端修剪生效时（通过 Tauri emit 广播 history-updated 事件）或在前端 handleSaveSettings 保存成功后，显式触发 HistoryPanel 重新拉取历史列表，保证界面展示与数据库状态实时同步。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.5, §3.2, §3.3` |
| 类别 | concurrency ｜ 层次 plan |
| 置信度 | high |

**问题**：并发死锁隐患：TransferEngine 终态更新处于持有 db_conn 互斥锁的上下文中，若按计划在此处触发 CacheManager 封装的修剪，会因 tokio::sync::Mutex 的不可重入性发生自身死锁；且持有 DB 锁时执行同步磁盘 IO 会严重阻塞信令与并发读写。

**依据**：核对 client/src-tauri/src/app_state.rs:17，db_conn 为全局共享的 Arc<tokio::sync::Mutex<rusqlite::Connection>>；核对 client/src-tauri/src/core/transfer_engine.rs:51-57，update_history_status 在更新状态时已持有 conn 锁（并在第 258, 506, 520, 590, 819, 855 行被调用）；核对 client/src-tauri/src/core/cache_manager.rs:16，CacheManager 内部同样持有该锁。若在 update_history_status 内直接或间接调用 CacheManager::prune_history，将二次请求同一个 tokio Mutex。

**建议**：明确锁的作用域与释放时序：在释放 update_history_status 的锁之后再发起修剪调用；或将 DB 记录修剪与磁盘文件删除解耦，在独立事务提交并释放锁后，再在后台异步执行磁盘文件物理清理。

### AGY-03 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.3 对比 plan §3.2` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：方案前后自相矛盾且执行顺序倒置：§3.2 先在事务内彻底删除 cache_entries 等行再返回路径给外部删文件，一旦磁盘删除由于权限、占用或崩溃失败，这些文件将永久脱离数据库索引，日后任何 TTL/LRU sweep 也无法扫描到它们，沦为永久不可回收的磁盘孤儿垃圾。

**依据**：plan §2.3（第 118-125 行）声称「先取 file_path，fs::remove_file 删磁盘文件，再删行，全部包在一个事务里」；而 plan §3.2（第 205-211 行）则改为「事务内删四张表的行，返回待删磁盘文件路径列表由 CacheManager 删文件」；对比 client/src-tauri/src/core/cache_manager.rs:120-129，现有 sweep 均是先 remove_file 后再删 cache_entries。

**建议**：统一 §2.3 与 §3.2：采用与现有 CacheManager::sweep_expired_and_lru 一致的健壮顺序，先查出待删文件的路径并在磁盘执行删除，确认后再清理数据库表行；或者设计重试容错机制，禁止在无备份引用的情况下提前删掉数据库元数据。

### AGY-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §1.2, §2.7, §3.6` |
| 类别 | scope-drift ｜ 层次 plan |
| 置信度 | high |

**问题**：需求 5 明确限定为「传输任务完成后」，计划无差别地将 FAILED（传输失败）卡片也纳入 30 秒自动消失。传输失败包含网络中断等关键警示，自动消失会导致用户漏看失败原因并误以为已成功，且计划未对此业务偏离给出决策说明。

**依据**：核对 docs/需求.md:14，需求第 5 条逐字为「可以设置传输任务完成后在界面中保持多久，一段时间后自动在界面中消失」；核对 client/src/components/TransferProgress.tsx:60-65，失败任务展示了关键的红色警示与失败信息；plan §1.2 与 §3.6 均将所有终态（包含 FAILED）一律视为定时移除对象。

**建议**：严格对齐需求，仅对 COMPLETED 任务注册自动消失定时器，FAILED 任务保持由用户手动点击 X 关闭；若确实希望失败卡片也能自动消失，应在设计中显式说明决策依据并建议提供独立的更长保留时间或视觉区分。

### AGY-05 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.3` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | medium |

**问题**：混淆了「数据库修剪上限」与「UI 列表查询上限」：当 history_max_entries 设为 0（不限制）时，随着时间积累，cmd_list_history 将全量加载海量历史，导致前端一次性渲染数千条 DOM 节点而卡死；且当存在免疫锁导致库内条数暂超上限时，硬编码 LIMIT 会屏蔽用户刚操作的免疫会话。

**依据**：核对 client/src-tauri/src/commands/history_cmd.rs:12；核对 client/src/components/HistoryPanel.tsx:137-234（无虚拟滚动，全量 DOM 渲染）；plan §3.3 规定 cmd_list_history 直接读 history_max_entries 并在为 0 时传 -1。

**建议**：将 UI 呈现上限与数据库淘汰上限解耦：即便数据库保留条数设为 0（不限制淘汰），cmd_list_history 在无分页支持前仍应保留合理的兜底安全显示条数（如 100 条）。

## 认为正确的部分

- 准确辨析了需求 4（持久化列表与磁盘缓存）与需求 5（纯前端内存活跃卡片）的落点差异，避免了将前端卡片消失错误做成删库删缓存的严重方向性偏差。
- 准确核验了 SQLite 外键默认关闭及 chunk_bitmaps/cache_entries 无级联的事实，确立了显式成对清理四张表和磁盘文件的原则。
- 对 #[serde(default = "...")] 的剖析深入到位，指出了 u32 默认值 0 与业务语义冲突的陷阱，设计了与 default_config() 共享常量的自定义反序列化函数。
- 在修剪中设计了跳过非终态任务与尊重 2 小时剪贴板免疫锁的硬约束，与既有架构哲学高度自洽。
- 敏锐识别到了 React 18 空依赖数组 useEffect 闭包无法读取最新 settings 的隐患，设计了 useRef 方案。

## 未覆盖范围（本侧盲区）

- 未在大规模历史数据（如上万条历史记录）下实机压测修剪对 SQLite WAL 检查点与文件系统 IO 延迟的影响。
- 未在 Windows / Linux 系统下实际验证短倒计时卡片与高频推送事件混合时的 UI 重绘性能。
- 未测试极端网络波动下，并发落终态任务同时触发修剪时可能产生的锁竞争排队情况。

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `docs/需求.md`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/commands/history_cmd.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/app_state.rs`
- `client/src/types/index.ts`
- `client/src/App.tsx`
- `client/src/components/SettingsModal.tsx`
- `client/src/components/TransferProgress.tsx`
- `client/src/components/HistoryPanel.tsx`
- `.reviews/history-retention-autodismiss/plan/2026-09-13-history-retention-autodismiss-claude.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":20,"grep":1,"glob":0,"run_command":0,"project_reads":17}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
