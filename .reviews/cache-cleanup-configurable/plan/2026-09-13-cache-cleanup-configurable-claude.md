# 磁盘缓存清理策略可配置化 — 实施计划

- 主题：`cache-cleanup-configurable`
- 阶段：plan（原稿）
- 日期：2026-09-13
- 角色：Driver (claude)
- 任务目标：把磁盘缓存的保留时长、容量上限、清理间隔做成可配置项（均有默认值），
  并收敛「常量与 SQL 字面量两套值」的陷阱。对应 `docs/需求.md` 20260912 第 9 条。

---

## 1. 背景与现状核验

需求原文只有一句：

> 9. 磁盘缓存文件要定时删除。

**核验结论：定时删除早已实现**，本轮不是从零做，而是把写死的策略参数开放出来。
若照字面理解去「新增定时删除」，会做出与既有 `sweep_expired_and_lru` 重复的第二套清理。

### 1.1 既有清理机制

`client/src-tauri/src/core/cache_manager.rs:171-232` 的 `sweep_expired_and_lru()`：

1. **TTL 淘汰**：`created_at` 超过 24 小时的缓存文件删除；
2. **LRU 配额**：总量超 10GB 时按 `last_accessed_at` 升序删到 8GB 以下；
3. 两条都**尊重 2 小时剪贴板免疫锁**（`clipboard_injected_at`）。

调度在 `client/src-tauri/src/lib.rs:343-354`：`tokio::time::interval(3600s)` 循环。
注意 `interval` 的首次 `tick()` **立即返回**，所以启动时会先扫一次，之后每小时一次。

### 1.2 真正的缺陷：同一个值有两套定义，改常量不生效

`cache_manager.rs:8-11` 定义了四个常量，但**只有两个真正被用到**：

| 常量 | 值 | 是否被引用 |
|---|---|---|
| `DEFAULT_TTL` | 24h | **否**——死代码 |
| `CLIPBOARD_LOCK_DURATION` | 2h | **否**——死代码 |
| `MAX_CACHE_SIZE_BYTES` | 10GB | 是（`cache_manager.rs:204`） |
| `SAFE_LOW_WATERMARK_BYTES` | 8GB | 是（`cache_manager.rs:219`） |

真正决定行为的是 SQL 里的**裸字面量**：

- `cache_manager.rs:178` —— `> 86400`（TTL）
- `cache_manager.rs:179`、`cache_manager.rs:207` —— `> 7200`（剪贴板免疫）
- `lib.rs:346` —— `from_secs(3600)`（清理间隔）

于是 `7200` 这一个值在全仓库有**三处**定义：`CLIPBOARD_LOCK_DURATION`（死的）、
两处 SQL 字面量（活的）、以及 `history_repo.rs:45` 的 `CLIPBOARD_LOCK_SECS`（活的，
上一轮 `history-retention-autodismiss` 新增）。

**这是个会骗人的陷阱**：谁想调 TTL，最自然的动作是改 `DEFAULT_TTL`——改完毫无效果，
而且没有任何编译警告（`pub const` 不触发 dead_code lint）。本轮必须一并收敛。

### 1.3 与上一轮的关系

上一轮 `history-retention-autodismiss` 做的是**按条数修剪传输历史**（删数据库行 +
对应缓存文件），本轮是**按时间与容量清理缓存文件**。两者正交，都在 `CacheManager` 内：

- `prune_history(limit)` —— 条数驱动，删「行 + 文件」；
- `sweep_expired_and_lru()` —— 时间/容量驱动，只删文件与 `cache_entries` 行，
  **不动 `transfer_tasks`**（历史记录仍在，只是显示「缓存已清理，无法重新装载」）。

这个分工是对的，本轮维持。

---

## 2. 设计决策

### 2.1 三个新设置字段

| 字段 | 类型 | 默认值 | 语义 |
|---|---|---|---|
| `cache_ttl_hours` | `u32` | `24` | 缓存文件保留小时数；`0` = 不按时间清理 |
| `cache_max_size_mb` | `u32` | `10240` | 缓存总量上限（MB，10240 = 10GB）；`0` = 不限容量 |
| `cache_sweep_interval_minutes` | `u32` | `60` | 后台清理间隔（分钟）；**最小 1**，不允许 0 |

默认值全部沿用现行行为，**已有用户升级后行为零变化**。

单位选择的理由：
- TTL 用**小时**而非秒——用户心智是「留一天」「留三天」，秒级精度没有意义；
- 容量用 **MB** 而非字节或 GB——字节数字太长易错，GB 又不够细（有人想设 500MB）；
- 间隔用**分钟**——小时太粗（想 15 分钟扫一次很合理），秒太细。

### 2.2 `0` 的语义：与上一轮保持一致，但间隔是例外

上一轮已确立「`0` = 关闭该限制」，本轮沿用于 TTL 与容量。

**但 `cache_sweep_interval_minutes` 不能为 0**：`tokio::time::interval(Duration::ZERO)`
会 **panic**（tokio 明确要求 period 非零）。若允许 0 并理解为「不清理」，则需要在
调度侧加分支；而「不清理」已经可以用 `ttl=0 且 max_size=0` 表达，再加一条通路
只会让两处语义打架。因此**下限锁死为 1 分钟**，前端与后端各校验一次。

### 2.3 低水位线按比例算，不单独开配置

现行是 10GB → 8GB（80%）。本轮**不**把低水位做成第四个配置项：

- 它与上限强耦合，用户单独调容易配出 `低水位 > 上限` 的非法组合；
- 多一个旋钮，多一份解释成本。

改为 `low_watermark = max_size * 80%`（整数运算，向下取整）。
`SAFE_LOW_WATERMARK_BYTES` 常量随之删除。

> 留出水位差是必要的：若删到刚好等于上限就停，下一个文件写入立刻又超限，
> 会变成每次 sweep 都在删——LRU 的抖动。

### 2.4 剪贴板免疫期**不**开放配置

`7200`（2 小时）保持写死，但收敛到**单一常量**。理由：

- 它不是「用户偏好」，而是**正确性保障**——文件还在系统剪贴板里被引用时删掉，
  粘贴就会失败。用户调小它等于给自己制造 bug；
- 开放它需要在 UI 上解释「为什么删文件要看剪贴板」，成本高于收益。

落点：把 `history_repo.rs:45` 的 `CLIPBOARD_LOCK_SECS` 提升为**全局唯一定义**
（移到 `cache_manager.rs` 或新的 `core/retention.rs`），删掉死常量
`CLIPBOARD_LOCK_DURATION`，两处 SQL 改为参数绑定。

### 2.5 SQL 一律参数绑定，禁止字面量

所有时间/容量阈值改用 `?n` 绑定传入，不再出现裸数字。这既是本轮收敛的目的，
也让参数可配置成为可能。

`strftime('%s', ...)` 的比较仍在 SQL 内完成（避免把时间计算搬到 Rust 侧后
与 SQLite 的时区处理产生偏差——既有代码全程用 `CURRENT_TIMESTAMP` 存 UTC）。

### 2.6 间隔改变后如何生效

`tokio::time::interval` 的周期在创建时固定，改设置不会影响已创建的 interval。
三种做法：

1. **重建 interval**（选用）：清理循环改为 `loop { sleep(当前间隔); sweep(); }`，
   每轮开始时从 `AppState.settings` 重新读间隔。改设置后**最迟下一轮生效**。
2. 用 `Notify` 唤醒并重建——能立即生效，但要处理「唤醒后是否立刻扫一次」的歧义；
3. 保存设置时重启整个清理任务——需要任务句柄管理，最复杂。

选 1 的理由：清理是后台维护动作，延迟一个周期生效完全可接受；
而它不需要任何额外的同步原语，代码量最小。

> 副作用：把间隔从 24 小时改成 5 分钟时，要等最多 24 小时才切换到新节奏。
> 这个取舍写进设置项的说明文字，不让用户困惑。

### 2.7 `serde(default = "函数")`，不能用裸 default

与上一轮完全相同的陷阱：`u32` 的 `Default` 是 `0`，而 `0` 在 TTL 与容量上表示
「不限制」。老库 JSON 没有这三个字段，裸 `#[serde(default)]` 会让升级用户静默变成
「永不清理缓存」——磁盘会被无声占满。间隔字段更糟：`0` 会让
`tokio::time::interval` **panic**，应用直接崩在启动路径上。

三个字段各配一个 default 函数，与 `default_config()` 共用常量。

---

## 3. 改动清单

### 3.1 保留策略常量集中（新增 `client/src-tauri/src/core/retention.rs`）

把散落各处的阈值收敛到一处：

```rust
pub const DEFAULT_CACHE_TTL_HOURS: u32 = 24;
pub const DEFAULT_CACHE_MAX_SIZE_MB: u32 = 10 * 1024;
pub const DEFAULT_SWEEP_INTERVAL_MINUTES: u32 = 60;
pub const MIN_SWEEP_INTERVAL_MINUTES: u32 = 1;
/// 剪贴板免疫窗口：正确性保障，不开放配置（见 §2.4）
pub const CLIPBOARD_LOCK_SECS: i64 = 7200;
/// LRU 删到上限的百分之多少为止（见 §2.3）
pub const LRU_LOW_WATERMARK_PERCENT: u64 = 80;
```

同时**删除** `cache_manager.rs:8-11` 的四个旧常量，
并把 `history_repo.rs:45` 的 `CLIPBOARD_LOCK_SECS` 改为引用本模块，消灭第三处定义。

### 3.2 `cache_manager.rs`

- `sweep_expired_and_lru()` 改签名为 `sweep(&self, policy: RetentionPolicy)`，
  `RetentionPolicy { ttl_hours, max_size_bytes }` 由调用方从设置构造；
- 两条 SQL 的 `86400` / `7200` 改为参数绑定；
- `ttl_hours == 0` 时**跳过 TTL 段**（不是传 0 秒——那会把所有文件立刻删光，
  是个能造成数据丢失的反向 bug）；
- `max_size_bytes == 0` 时跳过 LRU 段；
- 低水位按 §2.3 由上限算出。

### 3.3 `lib.rs` 清理循环

改为每轮读设置、按当前间隔 sleep（§2.6）。间隔从设置取，经
`max(MIN_SWEEP_INTERVAL_MINUTES)` 兜底——即使配置被外部改成 0 也不会 panic。

### 3.4 `settings_cmd.rs`

三个新字段 + 三个 default 函数；`default_config()` 补齐。
`cmd_save_settings` **不**需要立即触发 sweep（与历史修剪不同：条数超限是用户
当场可见的列表长度，而缓存清理没有对应的界面反馈，等下一轮即可）。

> 这一条与上一轮的处理不同，是刻意的，理由如上。若审查认为应当立即执行，
> 落点在 `cmd_save_settings` 末尾，与 `prune_and_notify` 并列。

### 3.5 前端

- `types/index.ts` 补三个字段；`App.tsx` 的 `defaultSettings` 同步；
- `SettingsModal.tsx` 新增三个数字输入，复用既有 `parseU32(raw, max)`：
  - 保留小时数：`0`–`8760`（1 年），`0` = 不按时间清理；
  - 容量上限 MB：`0`–`1048576`（1TB），`0` = 不限容量；
  - 清理间隔分钟：`1`–`1440`（1 天），**最小 1**，需要 `parseU32` 支持下限
    （现有签名只有 `max`，要加 `min` 参数）。

设置项已经有 7 项，弹窗会更长——上一轮刚改的滚动布局正好承接，无需再动布局。

### 3.6 文档

`docs/需求.md` 第 9 条标注已实现与决策要点；`DESIGN.md` 的 `SettingsModal.tsx`
说明追加缓存策略。

---

## 4. 测试计划

### 4.1 Rust 单测（`cache_manager.rs` 的 `mod tests`，已有临时目录夹具可复用）

- TTL 段：`created_at` 超期的文件被删、未超期的保留；
- `ttl_hours == 0` 时**一个文件都不删**（守护 §3.2 那个会删光数据的反向 bug）；
- LRU 段：超上限时按 `last_accessed_at` 升序删到低水位以下；
- `max_size_bytes == 0` 时跳过 LRU；
- 剪贴板免疫期内的文件在两段中都不被删；
- 低水位计算：上限 100MB → 低水位 80MB。

### 4.2 Rust 单测（`settings_cmd.rs`）

- 老库 JSON（无三个新字段）反序列化后得到 24 / 10240 / 60，**不是 0**；
- 显式 `0` 在 TTL 与容量上被保留；
- 间隔字段即使存进 `0`，调度侧取值仍 ≥ 1（守护 panic）。

### 4.3 前端测试（vitest，复用上一轮基建）

- 三个输入的校验：空串、负数、小数、超上限；
- 间隔输入 `0` 被拒绝并给出字段级提示。

### 4.4 手工端到端

1. 设置「保留小时数 = 0」→ 观察日志，确认 sweep 不再按时间删；
2. 设置「清理间隔 = 1 分钟」→ 等待约 1 分钟，日志出现清理记录；
3. 造一批超期缓存文件（直接改 `cache_entries.created_at`）→ 下一轮被清理，
   且磁盘文件确实消失；
4. 装载某会话到剪贴板后立刻把 TTL 设为 0 小时 → 该会话文件**不被删**（免疫期）。

---

## 5. 明确不做

- 不新增第二套清理机制（§1 已核验既有机制可用）；
- 不开放剪贴板免疫期配置（§2.4）；
- 不开放 LRU 低水位配置（§2.3）；
- 不做「立即清理」按钮——需求未提，且 sweep 无界面反馈，加了也看不出效果；
- 不改版本号。
