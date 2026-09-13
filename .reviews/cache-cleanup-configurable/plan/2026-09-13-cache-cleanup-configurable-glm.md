---
schema: trivium.review.v1
topic: cache-cleanup-configurable
stage: plan
role: glm
vendor: zcode
agent_cli: zcode/0.16.5
model_requested: GLM-5.3-Flash
model_effective: bigmodel-coding-plan/GLM-5.3-Flash
model_effective_source: trace
model_route: config_copy
run_id: 20260913T021317Z
rerun_index: 1
snapshot_hash: sha256:4ae560d94fcad5dd27ff52748e4e16fdd0bc8dcfda3d95c5baaa52f27582e364
blind: true
started_at: 2026-09-13T02:13:18.320Z
finished_at: 2026-09-13T02:26:09.806Z
duration_s: 771
tool_calls:
  read: 15
  grep: 3
  glob: 1
  run_command: 0
  project_reads: 16
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 1
  minor: 2
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: sess_29c53dea-fd8f-4a36-918d-56a796ce046b
---

# 计划审查：cache-cleanup-configurable

> Critic-B · 智谱 ZCode ｜ 模型 `bigmodel-coding-plan/GLM-5.3-Flash`
> ｜ 运行 `20260913T021317Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

最危险的一条：§2.6/§3.3 选用的 loop { sleep(间隔); sweep(); } 把首次清理推迟一个完整间隔，静默丢掉现行 interval 首跳立即返回的「启动即扫」（计划 §1.1 自己写明了这一点），与 §2.1「升级后行为零变化」直接矛盾；用户若按 §3.5 允许的上限设 1440 分钟且设备每日重启，sleep 永远睡不满，sweep 可能一次都不执行。其余：MB→字节换算未定宽，u32 回绕会让默认 10240MB 被当成 ~2GiB 触发过度删除；§2.2「后端校验一次」在改动清单无落点且 §4.2 的间隔钳制测试按 §3.3 写法测不到；sweep 重写未对齐同文件 prune_history 已论证的「IO 不持锁」纪律，1TB 上限放大持锁停顿。事实盘点（死常量、字面量位置、parseU32 签名）逐行核实全部属实，收敛方向与 0 值语义设计正确。

**结论**：`request-changes`

## 审查意见（共 4 条：重要 1 ｜ 次要 2 ｜ 吹毛求疵 1）

### GLM-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.6/§3.3（对照 client/src-tauri/src/lib.rs:345-355）` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：选用的「loop { sleep(当前间隔); sweep(); }」把清理整体后移一个间隔：现行 tokio::time::interval 首跳立即返回，启动时会先扫一次（计划 §1.1 第 30 行自己写明），改成 sleep 先行后该行为静默消失——默认配置下启动后 60 分钟内不清理；若按 §3.5 允许的上限把间隔设为 1440 分钟，叠加开机自启 + 每日重启的使用形态，sleep 几乎永远睡不满，sweep 可能一次都不执行。这与 §2.1「默认值全部沿用现行行为，已有用户升级后行为零变化」的承诺直接矛盾，属于计划内部自相矛盾。

**依据**：读了 client/src-tauri/src/lib.rs:345-355（现行循环 interval.tick() 首跳立即返回，启动即 sweep）、client/src-tauri/src/core/cache_manager.rs:172（现行 sweep 签名）；被审计划 §1.1 自述「启动时会先扫一次」、§2.1 第 79 行承诺「行为零变化」、§2.6 第 133 行给出 sleep 先行的结构，三处互相冲突。

**建议**：循环改为「先 sweep 后 sleep」（每轮 sweep 完成后按当前间隔休眠），既保留启动即扫，又保住「每轮开始时读设置」的可变间隔语义；并在 §2.6 的取舍说明里补上这一点。

### GLM-02 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.1/§3.1/§3.2（cache_max_size_mb → max_size_bytes 的换算未定宽）` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | medium |

**问题**：设置字段 cache_max_size_mb 是 u32，§3.5 允许最大 1,048,576 MB（2^20），换算字节要乘 2^20，积为 2^40，超出 u32。计划规定了 LRU_LOW_WATERMARK_PERCENT: u64，但没有规定 RetentionPolicy.max_size_bytes 的类型与「先升位再乘」的运算顺序。若实现按 u32 乘：debug 构建溢出 panic；release 构建静默回绕——默认 10240 MB 回绕成 ~2 GiB，低水位随之错到 ~1.7 GiB，LRU 开始大规模删除用户想保留的缓存；4096 MB 的整数倍会回绕成 0，恰好落入计划定义的「0 = 不限容量」，语义完全反转。§4.1 计划的「上限 100MB → 低水位 80MB」测试值太小（104,857,600 < u32::MAX），分辨不出这个宽度错误，测试会照常绿。

**依据**：读了被审计划 §2.1（u32/10240）、§3.1（仅 LRU_LOW_WATERMARK_PERCENT 标了 u64，max_size_bytes 类型未定）、§3.5（上限 1048576）；对照 client/src-tauri/src/core/cache_manager.rs:10-11（现行 MAX_CACHE_SIZE_BYTES 为 u64 常量）、client/src-tauri/src/commands/settings_cmd.rs（既有字段全为 u32，无换算先例可抄）。

**建议**：在 §3.2 明确 max_size_bytes: u64，并写明「MB 先 as u64 再乘 1048576」；§4.1 增一条大值换算单测（如 1048576 MB → 2^40 字节、10240 MB → 10,737,418,240），让宽度错误当场红。

### GLM-03 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §2.2/§3.3/§4.2（对照 client/src-tauri/src/lib.rs:345、commands/settings_cmd.rs:80-137）` |
| 类别 | test-gap ｜ 层次 plan |
| 置信度 | high |

**问题**：§2.2 承诺间隔下限「前端与后端各校验一次」，但 §3.4 的 settings_cmd 改动清单里没有任何校验落点，唯一的后端防线是 §3.3 调度循环内的内联 max(MIN_SWEEP_INTERVAL_MINUTES, x)。两个后果：其一，「后端校验」与实际机制（调度侧兜底钳制）名实不符，实现者可能误以为已有保存期校验，或重复加码；其二，§4.2 第三条测试「间隔存进 0 仍取 ≥1」按 §3.3 的写法无法落地——钳制逻辑内联在 lib.rs 的 spawned 循环里，该文件没有任何测试基建，单测触达不到，这条验收标准按现计划写不出来。

**依据**：读了被审计划 §2.2 第 93 行、§3.3 第 187-188 行、§4.2 第 232 行；对照 client/src-tauri/src/lib.rs:345-355（循环体在 setup 的 spawn 内，无测试入口）、client/src-tauri/src/commands/settings_cmd.rs（cmd_save_settings 对既有数值字段均无保存期校验，先例是 client/src-tauri/src/core/history_pruner.rs:33 在运行时解释 0 = 不限制）。

**建议**：把钳制定义为 retention.rs 里的具名纯函数（输入 AppSettings → 有效间隔 Duration），调度循环与单测共同指向它；§2.2 措辞对齐为「调度侧兜底钳制」，或明确在 cmd_save_settings 落库前钳制——二选一写死，不要让「校验」二字悬空。

### GLM-04 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §3.2（对照 client/src-tauri/src/core/cache_manager.rs:172-233）` |
| 类别 | concurrency ｜ 层次 plan |
| 置信度 | medium |

**问题**：§3.2 重写 sweep 的签名与 SQL，但未提锁纪律：现行 sweep 从取到 db_conn 锁起一路持有到函数尾，文件删除循环（fs::remove_file）在锁内做；而同一文件里 prune_history（cache_manager.rs:103-116 的注释）与 history_pruner.rs 的模块头注都专门论证过「磁盘 IO 段必须不持锁，否则堵塞整条信令链路」。容量上限开放到 1TB 后，单轮 LRU 可能删数千个文件，持锁停顿窗口被同比放大。属既有行为的放大而非新引入，但本轮既然重写该函数，是顺手对齐或留痕的时机。

**依据**：读了 client/src-tauri/src/core/cache_manager.rs:173（sweep 开头取锁直至函数结束）、同文件 103-116（prune_history 的三段式与不持锁理由）、client/src-tauri/src/core/history_pruner.rs:1-16（锁纪律总纲：tokio::sync::Mutex 不可重入、持锁做同步 IO 的代价）。

**建议**：在 §3.2 明确：sweep 采用与 prune_history 相同的「选取（持锁）→ 删文件（不持锁）→ 删行（重取锁）」三段式；若决定维持现状，写明理由留痕即可。

## 认为正确的部分

- §1.2 常量/字面量盘点逐行核实全部属实：DEFAULT_TTL 与 CLIPBOARD_LOCK_DURATION 确为死代码（全仓检索仅剩定义处），86400/7200 裸字面量位于 cache_manager.rs:178/179/207、间隔字面量位于 lib.rs:346，全部对号；「改常量不生效且无编译警告」的陷阱判断成立，收敛是本轮真正的价值。
- §2.7 serde(default = "函数") 的论据与 settings_cmd.rs 既有两个字段的模式、注释与测试先例完全一致，老库 JSON 反序列化测试可直接扩展。
- 0 值语义设计正确：TTL/容量 0 = 关闭该段（与上一轮确立的约定一致），§3.2 明确「ttl_hours == 0 跳过 TTL 段而不是绑 0 秒」，防住了把全部缓存立刻删光的反向 bug；间隔锁死 ≥1 且有调度侧兜底，对 tokio::time::interval(Duration::ZERO) panic 的引用无误。
- §2.4 剪贴板免疫期不开放配置的论证成立（正确性保障而非用户偏好），收敛到单一常量 + 参数绑定与 history_repo.rs:45 现状吻合；落点执行后全仓不再有第二处 7200 定义。
- §1.3 与上一轮的分工核验成立：prune_history 条数驱动、动 transfer_tasks，sweep 时间/容量驱动、只删文件与 cache_entries 行；§3.4 保存后不立即 sweep 的偏差有明确论证且给出备选落点，属合理决策。
- §3.5 对 parseU32 现签名（raw, max = U32_MAX）的判断准确，加 min 参数的方向正确；三个默认值（24 小时/10240 MB/60 分钟）与现行行为逐一核对无误，字段命名 snake_case 与前端 types/index.ts 既有约定一致。

## 未覆盖范围（本侧盲区）

- 被审文件的内容哈希（sha256:4ae560d9…）未能独立重算：本审查环境处于计划模式、无 shell 可用；已通读盘上原文并确认与任务书节选逐字一致，指纹核验依赖编排器。
- transfer_engine.rs、connection_actor.rs、platform 层未读（与缓存清理策略无直接交集）。
- 未运行 cargo test / vitest，§4 测试计划的可执行性仅按静态判断。
- last_accessed_at 除 register_entry 写入外无任何更新路径（所谓 LRU 实为注册序）属既有行为，本轮计划未触及，未评估是否应在本轮修正。
- 前端 App.test.tsx 的 baseSettings 夹具需随 AppSettings 类型补三个新字段，计划未列（编译期即暴露，未单列为意见）。
- DESIGN.md 未读，§3.6 的文档改动只核对了 docs/需求.md 的第 9 条原文。

## 实际查阅的项目文件

- `.reviews/cache-cleanup-configurable/plan/2026-09-13-cache-cleanup-configurable-claude.md`
- `.reviews/history-retention-autodismiss/plan/2026-09-13-history-retention-autodismiss-revised-claude.md`
- `CLAUDE.md`
- `README.md`
- `docs/需求.md`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src-tauri/src/core/history_pruner.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/app_state.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src/components/SettingsModal.tsx`
- `client/src/types/index.ts`
- `client/src/App.tsx`

> 编排器从工具轨迹中记录到的读取次数：{"read":15,"grep":3,"glob":1,"run_command":0,"project_reads":16}

---

*本文档由 TriviumCode 编排器从 `glm` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
