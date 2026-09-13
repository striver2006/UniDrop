---
schema: trivium.review.v1
topic: cache-cleanup-configurable
stage: plan
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260913T021317Z
rerun_index: 1
snapshot_hash: sha256:4ae560d94fcad5dd27ff52748e4e16fdd0bc8dcfda3d95c5baaa52f27582e364
blind: true
started_at: 2026-09-13T02:13:18.320Z
finished_at: 2026-09-13T02:26:09.806Z
duration_s: 171
tool_calls:
  read: 25
  grep: 7
  glob: 0
  run_command: 0
  project_reads: 16
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 3
  minor: 1
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: 5c07b89d-c5e9-4b3c-aa52-1adf37139a37
---

# 计划审查：cache-cleanup-configurable

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260913T021317Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

最危险的问题在于清理调度循环改为先 sleep 再 sweep 会导致冷启动丢失首次清理，若用户单次开机时长短于间隔将导致缓存永远不被清理；其次 cache_max_size_mb（u32）在转字节时存在整数乘法溢出风险（默认 10GB 即可溢出导致 panic）；此外后端保存接口遗漏下限校验，未能兑现承诺的双端防护；手工端到端测试也与延迟生效机制存在冲突。建议调整调度结构、封装防溢出转换并补齐后端校验后再实施。

**结论**：`request-changes`

## 审查意见（共 5 条：重要 3 ｜ 次要 1 ｜ 吹毛求疵 1）

### AGY-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `.reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md:§2.6, §3.3` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：清理循环改为先 sleep 再 sweep 导致冷启动丢失首次清理，且单次短时间运行的客户端将永远无法执行清理。

**依据**：查阅 .reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md §1.1 明确指出现行实现依赖 tokio::time::interval 的首次 tick() 立即返回保证启动时立即扫描；但在 §2.6 和 §3.3 中，计划将清理循环改为 loop { sleep(当前间隔); sweep(); }。核对 client/src-tauri/src/lib.rs，这一改动导致应用冷启动时必须先阻塞休眠一个完整间隔（默认 60 分钟）才会执行首次 sweep，若用户单次运行时间短于清理间隔，缓存将永远得不到清理。

**建议**：调整清理循环生命周期逻辑，保证冷启动时立即触发一次 sweep（例如在进入 sleep 循环前先执行一次 sweep，或在循环体内采用先 sweep 后 sleep 的顺序）。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `.reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md:§3.2` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：cache_max_size_mb（u32）在转为字节时存在整数乘法溢出（integer overflow）风险，默认值 10GB 即可打爆 u32。

**依据**：查阅 .reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md §2.1 与 §3.2，cache_max_size_mb 为 u32，默认值为 10240，前端支持填入高达 1048576（1TB）。若在构造 RetentionPolicy { max_size_bytes } 时直接使用 u32 进行字节乘法（settings.cache_max_size_mb * 1024 * 1024），默认值 10240 * 1024 * 1024 = 10,737,418,240 即已远超 u32::MAX（4,294,967,295），将在 Debug 模式下直接 panic，在 Release 模式下发生回绕截断为 2GB。计划未对该类型转换与防溢出作任何规范。

**建议**：在类型定义或转换时显式要求先将 cache_max_size_mb 提升为 u64 再进行乘法运算；建议在 RetentionPolicy 上封装构造函数（如 RetentionPolicy::from_settings）集中进行防溢出转换。

### AGY-03 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `.reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md:§2.2, §3.4` |
| 类别 | contract ｜ 层次 plan |
| 置信度 | high |

**问题**：计划承诺「前端与后端各校验一次」，但后端持久化入口 cmd_save_settings 遗漏了清理间隔最小值的校验与规范化。

**依据**：查阅 .reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md §2.2，计划声称「下限锁死为 1 分钟，前端与后端各校验一次」。但核对改动清单 §3.3、§3.4 与 client/src-tauri/src/commands/settings_cmd.rs，后端仅在 lib.rs 调度消费侧通过 .max(MIN_SWEEP_INTERVAL_MINUTES) 兜底防 panic，持久化入口 cmd_save_settings 完全未对 cache_sweep_interval_minutes 进行校验或钳制。若外部调用传入 0，非法值仍会被直接持久化存库并在 cmd_get_settings 中返回。

**建议**：在 cmd_save_settings 中对 clean_settings.cache_sweep_interval_minutes 进行显式校验拒绝或下限钳制（.max(MIN_SWEEP_INTERVAL_MINUTES)），守住持久化边界。

### AGY-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `.reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md:§2.6, §4.4` |
| 类别 | test-gap ｜ 层次 plan |
| 置信度 | high |

**问题**：手工端到端测试步骤 2 与计划自身设计的「延迟一个周期生效」机制存在直接逻辑矛盾。

**依据**：查阅 .reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md §2.6，方案选择「延迟一个周期生效」并承认改小间隔时需等待当前长周期结束。但 §4.4 步骤 2 却设定「设置『清理间隔 = 1 分钟』→ 等待约 1 分钟，日志出现清理记录」。在当前协程正在执行 60 分钟休眠的情况下，1 分钟内绝不可能触发下一次清理，手工测试必然失败。

**建议**：调整方案使间隔修改能够通过通知机制（如复用或仿照已有的 Notify）被调度任务感知并立即重建休眠，或者修正手工测试用例使其包含等待或重启的前置条件。

### AGY-05 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `.reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md:§3.1, §3.5` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | medium |

**问题**：改动清单遗漏了 core/mod.rs 的模块声明，且前端 parseU32 扩展下限时未解耦空串错误文案。

**依据**：查阅 client/src-tauri/src/core/mod.rs 与 client/src/components/SettingsModal.tsx。计划 §3.1 提出新建 core/retention.rs 但清单遗漏了在 core/mod.rs 中注册模块；此外既有 parseU32 的空串提示硬编码为「不能为空（不限制请填 0）」，若直接用于 min=1 的字段将误导用户填入 0。

**建议**：在改动清单补充 core/mod.rs 的模块导出，并在前端 parseU32 扩展 min 参数时针对 min > 0 的情况解耦空串提示与下限错误提示。

## 认为正确的部分

- 识别出既有代码中 DEFAULT_TTL 与 CLIPBOARD_LOCK_DURATION 为死代码、实际行为由 SQL 字面量决定的陷阱，并制定了常量收敛策略。
- 配置项默认值（24 小时、10240 MB、60 分钟）严格对齐既有行为，确保升级后零行为漂移。
- 0 值语义清晰，ttl_hours 为 0 时显式跳过 TTL 段，避免了向 SQL 传入 0 秒导致清空全部文件的反向 bug。
- 严格沿用 serde(default = "...") 自定义反序列化兜底函数，杜绝旧配置反序列化默认值为 0 造成策略失效或 panic 的隐患。
- 坚持剪贴板免疫锁与 LRU 低水位线不对外开放配置，合理收敛了配置复杂度并保障了剪贴板引用的正确性。

## 未覆盖范围（本侧盲区）

- 未在多平台（Windows / Linux）真实环境中验证各 OS 默认缓存目录的路径解析。
- 未针对极端大文件或高频并发写入场景下的 LRU 清理执行性能与锁竞争进行压力测试。

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/commands/history_cmd.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/app_state.rs`
- `client/src-tauri/src/core/history_pruner.rs`
- `client/src-tauri/src/core/mod.rs`
- `client/src/App.tsx`
- `client/src/types/index.ts`
- `client/src/components/SettingsModal.tsx`
- `docs/需求.md`
- `.reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md`
- `.reviews/history-retention-autodismiss/plan/2026-09-13-history-retention-autodismiss-revised-claude.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":25,"grep":7,"glob":0,"run_command":0,"project_reads":16}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
