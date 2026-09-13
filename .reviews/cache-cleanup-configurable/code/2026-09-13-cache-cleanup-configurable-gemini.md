---
schema: trivium.review.v1
topic: cache-cleanup-configurable
stage: code
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260913T024250Z
rerun_index: 1
snapshot_hash: sha256:2502e3613a7d3e708718174e9418da296993d72cf5c9e5e4243c0addd96c5d7c
blind: true
started_at: 2026-09-13T02:42:51.327Z
finished_at: 2026-09-13T02:56:19.217Z
duration_s: 155
tool_calls:
  read: 22
  grep: 6
  glob: 0
  run_command: 0
  project_reads: 19
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 1
  minor: 1
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: 084653eb-dbe4-4964-904a-0ebf46a75e69
---

# 代码审查：cache-cleanup-configurable

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260913T024250Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

本次代码变更总体质量较高，彻底收敛了分散的清理与免疫窗口常量，落实了先升位后相乘的 u64 容量换算，且调度循环严格贯彻了先 sweep 后 sleep 与三段式锁纪律。
但在核心清理函数 sweep 中发现一处主要缺陷：当 TTL 与 Quota 均启用时，LRU 容量段未预先扣除 TTL 已选中释放的条目体积，导致本已降至低水位的缓存继续无辜误删未过期的活跃条目。
此外 settings_cmd.rs 内部单元测试遗漏了对老库反序列化三个新字段默认值的断言守护。需修正后方可合入。

**结论**：`request-changes`

## 审查意见（共 3 条：重要 1 ｜ 次要 1 ｜ 吹毛求疵 1）

### AGY-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/cache_manager.rs:202-236` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：TTL 淘汰与容量配额（LRU）同时启用时，LRU 段未将 TTL 段已选出文件的体积从 remaining 预先扣除，且未在进入循环前检查 TTL 释放后是否已降至低水位，导致本无需清理的未过期活跃缓存文件被错误驱逐（Over-eviction）。

**依据**：在 client/src-tauri/src/core/cache_manager.rs:184-236 的 sweep 第 1 段中，若同时开启 TTL 与 Quota：第 185 行收集了 expired 文件加入 victims；第 203 行读取当前全部缓存总大小 total；第 224 行让 remaining = total，并未减去 TTL 阶段已选出的条目体积，也未判断扣除 TTL 后的剩余量是否已满足低水位；随后第 226-235 行遍历 candidates 时，只要当前条目不在 victims 中就追加进去并扣减 remaining。这导致即使超期文件释放后已满足低水位，LRU 仍会把访问时间靠前的未过期正常缓存文件错误加入 victims 驱逐。现有单测 sweep_removes_files_past_ttl 仅传入 policy(24, 0)，sweep_enforces_quota_down_to_low_watermark 仅传入 policy(0, 10)，两项独立测试均将另一项关闭为 0，漏掉了两者同时开启的复合场景。

**建议**：在第 1 段中记录 TTL 阶段选出条目的总体积 ttl_freed_bytes；进入 Quota 阶段时令 remaining = total.saturating_sub(ttl_freed_bytes)；若 remaining <= low 则无需再从 candidates 中选取；若仍超限，遍历 candidates 时跳过已在 victims 中的条目且不重复扣减 remaining，直到 remaining <= low。并补充 TTL 与 Quota 同时启用（如 policy(24, 10)）的集成单测。

### AGY-02 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/commands/settings_cmd.rs:252-322` |
| 类别 | test-gap ｜ 层次 code |
| 置信度 | high |

**问题**：settings_cmd.rs 的单元测试未对新增的三个缓存清理字段在老配置兼容（缺字段走 default）、显式 0 值保留以及 round-trip 场景进行断言守护，若后续意外改写 default 属性将无法被既有测试拦截。

**依据**：在 client/src-tauri/src/commands/settings_cmd.rs:40-58 中，新增了 cache_ttl_hours、cache_max_size_mb、cache_sweep_interval_minutes 三个字段并使用了自定义 default；但查看该文件内部 tests 模块（第 252-322 行），legacy_settings_json_deserializes_without_data_loss 仅对 history_max_entries 和 transfer_card_retain_secs 做了缺省断言，完全漏掉了对这三个新字段默认值（24、10240、60）的断言；explicit_zero_is_preserved_not_defaulted 亦未验证 cache_ttl_hours: 0 和 cache_max_size_mb: 0；current_settings_json_round_trips 亦未断言三个新字段。

**建议**：在 settings_cmd.rs 的 legacy_settings_json_deserializes_without_data_loss 测试中补全对 cache_ttl_hours (24)、cache_max_size_mb (10240)、cache_sweep_interval_minutes (60) 的断言；并在 explicit_zero_is_preserved_not_defaulted 中覆盖 0 值的保留断言。

### AGY-03 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/cache_manager.rs:108` |
| 类别 | maintainability ｜ 层次 code |
| 置信度 | high |

**问题**：注释残留已废弃的方法名 sweep_expired_and_lru，容易给后续代码维护者带来困扰。

**依据**：client/src-tauri/src/core/cache_manager.rs:108 的 prune_history 函数文档注释中写有：『先删文件后删行，与 sweep_expired_and_lru 同序』。而本轮变更已将 sweep_expired_and_lru 重构为 sweep，全仓已无其它代码调用旧方法名。

**建议**：将注释中的 sweep_expired_and_lru 更新为 sweep。

## 认为正确的部分

- 换算宽度与防溢出机制落实严密：RetentionPolicy::from_settings 强制 (mb as u64) * 1024 * 1024 先升位后相乘，并在 retention.rs 针对 4GB/8GB/1TB 回绕为 0 的语义反转提供了完整的防御性单元测试。
- 清理循环「先 sweep 后 sleep」结构正确兑现：lib.rs 的后台清理循环采用 sweep 前置、sleep 后置，保住了冷启动即刻清理旧缓存的特性，同时每轮重新读取设置保障了参数可变性，且在调用 sweep 前释放了 settings 锁。
- 三段式锁纪律结构清晰：sweep 严格按照「持锁选取 -> 无锁删文件 -> 重新持锁单事务删行」执行，避免长 IO 阻塞 SQLite 锁与控制面信令通道，删文件失败保留行以防磁盘孤儿。
- 常量收敛彻底：删除了旧代码未使用的死常量（如 DEFAULT_TTL 等），消除了 SQL 中硬编码的 7200/86400/3600 等数字，保留策略与剪贴板免疫判据的唯一事实源收敛至 core/retention.rs 并通过 SQL 参数绑定生效。
- 前端表单验证与类型契约完善：SettingsModal.tsx 针对 min>0（清理间隔）与 min=0（TTL/容量）实施了针对性文案和严格的边界检查，并提供了完备的交互测试。

## 未覆盖范围（本侧盲区）

- 未在 Windows 与 Linux 宿主机上实际拉起 Tauri 原生桌面窗口验证多平台运行时行为。
- 未针对上万个真实文件、接近 1TB 容量规模的超大文件系统进行真实磁盘 I/O 延迟压测。
- 未覆盖 SQLite 在低内存或磁盘已满极端异常环境下的事务提交失败处理。

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `client/src-tauri/src/core/retention.rs`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src-tauri/src/core/mod.rs`
- `client/src-tauri/src/core/history_pruner.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/app_state.rs`
- `client/src/components/SettingsModal.tsx`
- `client/src/components/SettingsModal.test.tsx`
- `client/src/types/index.ts`
- `client/src/App.tsx`
- `client/src/App.test.tsx`
- `docs/需求.md`
- `docs/design/DESIGN.md`
- `.reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-revised-claude.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":22,"grep":6,"glob":0,"run_command":0,"project_reads":19}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
