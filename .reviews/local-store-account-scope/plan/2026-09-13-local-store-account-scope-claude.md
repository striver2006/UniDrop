---
主题: local-store-account-scope
阶段: plan
角色: claude
日期: 2026-09-13
---

# 本地历史与缓存按账号分区 — 计划

## Context

上一轮（`multi-account-hardening`）把服务端的跨账号串台修干净了，但在「明确不做」
里留下一条实质得多的泄露，两位审查员都确认属实：

**本地传输历史与磁盘缓存完全没有账号概念。** `transfer_tasks` /
`transfer_items` / `chunk_bitmaps` / `cache_entries` 四张表都没有 `account_id`
列（`client/src-tauri/src/storage/db.rs`），`history_repo.rs` 的所有查询也没有
任何账号条件。后果是在同一台机器上切换账号后：

- 历史面板会列出前一个账号的文件名、大小、预览摘要；
- 「重新装载」按钮能把前一个账号的缓存文件写进系统剪贴板。

这不是理论风险——它是切一次账号就必然发生的行为。上一轮删掉的
`paired_devices` 是张空表，而这四张是有数据的。

**本轮同时要审上一轮遗留的两处加固**（commit `69b9975`，标题已标
「未经双审，待下一轮」）：`connection_actor.rs` 的 `install_default()` 位置，
与 `USER_GUIDE.md` 的 X.509 v3 证书说明。启动双审时 `--base 47766fb`，
diff 就恰好是「那两处 + 本轮改动」。

---

## 1. 改动面（比预想小）

先说一个决定范围的事实：**所有 SQL 都以 `session_id` 定位，而 session_id 是
客户端现铸的 UUIDv4**，跨账号不会撞。所以泄露只发生在**列表类**查询上，
按 session_id 精确定位的那些函数根本不需要账号参数。

| 函数 | 定位方式 | 是否加账号过滤 |
| :--- | :--- | :--- |
| `list_history` | 全表扫 + LIMIT | **必须**——这是泄露主路径 |
| `select_prune_candidates` | 全表扫 | **必须**，且语义要变（见 §2.1） |
| `record_task` | 写入 | **必须**——写入时记归属 |
| `get_task_brief` | `WHERE session_id = ?` | 不加 |
| `update_task_status` | `WHERE session_id = ?` | 不加 |
| `delete_sessions` | `WHERE session_id IN (...)` | 不加 |
| `filter_recently_injected` | `WHERE session_id IN (...)` | 不加 |
| `reset_stale_in_flight` | 全表扫 | **刻意不加**（见 §2.3） |

「不加」的那几个要在注释里写明理由，否则后来人会以为是漏了：它们的
session_id 来自本机流程或同账号对端（服务端已保证信令不跨账号），
不是用户可输入的值；给它们加一个用不上的参数，只会让调用点被迫
到处捞 account_id，反而增加出错面。

## 2. 三个需要定调的设计张力

### 2.1 `history_max_entries` 改为每账号，`cache_*` 保持全局

这是本计划最容易做错的地方，三个保留策略的**作用域并不相同**：

| 设置项 | 作用域 | 理由 |
| :--- | :--- | :--- |
| `history_max_entries` | **每账号** | 它是「界面列表保留多少条」。若保持全局，账号 A 传 100 个文件就会把账号 B 的历史全部挤掉——分区做了等于没做 |
| `cache_max_size_mb` | **全局** | 磁盘是物理共享资源。按账号各给 10 GB，N 个账号就是超卖 N 倍，最后谁都写不进去 |
| `cache_ttl_hours` | **全局** | 时间与账号无关，一个 24 小时没人碰的文件不会因为它属于谁而更该留着 |

也就是说 `prune_history`（条数）要带账号，`sweep`（TTL + 容量）不带。
两个函数紧挨在一起且形状相似，所以各自的注释里都要写明为什么一个带一个不带，
并互相指向——否则「统一一下」是非常自然的下一步误操作。

### 2.2 只给 `transfer_tasks` 加列，不给四张表都加

`transfer_items` / `chunk_bitmaps` / `cache_entries` 都以 `session_id` 为外键，
归属由父表唯一决定。四张表各存一份 `account_id` 会产生四份**可能互相矛盾**的
归属记录，而它们本该恒等——多出来的三份只是等着谁写漏一处。

代价是 `select_prune_candidates` 里那条 `cache_entries` 子查询要经 `transfer_tasks`
关联才能知道账号。该查询已经是关联形态，加一个 `t.account_id = ?` 条件即可。

### 2.3 `reset_stale_in_flight` 保持全局

它在启动时把崩溃残留的非终态行复位为 FAILED。**不能**按账号过滤：
按账号过滤的话，上次崩溃时属于账号 A 的死行，在用户改用账号 B 之后
永远不会被复位，会永久占住 A 的保留额度并在 A 的历史里显示成「传输中」。
这一条要写进注释，它看起来像漏了过滤。

### 2.4 缓存文件不分账号目录

文件名已带 session_id（UUID），跨账号不会撞；DB 过滤已足以阻止跨账号可见。
按账号分目录需要迁移存量文件——一次可能失败到一半、且失败后文件脱离
`cache_entries` 索引成为不可回收孤儿的操作——风险显著大于收益。
这个取舍要写进 `cache_manager` 的注释。

## 3. 迁移：存量行如何归属

`init_database` 加一条宽松迁移（与既有的 `ALTER TABLE transfer_tasks ADD COLUMN
preview_summary` 同处）：

```rust
let _ = conn.execute_batch("ALTER TABLE transfer_tasks ADD COLUMN account_id TEXT;");
```

**列留 NULL，不在这里回填。** 因为 `init_database` 跑在读取 settings 之前
（见 `lib.rs` 的 `run()`），此刻还不知道当前账号是谁。

回填放在读到 settings 之后，作为一次「认领」：

```rust
UPDATE transfer_tasks SET account_id = ?1 WHERE account_id IS NULL
```

需要在计划里明说的一个**无法消除的模糊**：我们无从知道存量历史原本属于谁，
只能归给升级后首次启动时的当前账号。若用户升级后先切了账号再打开历史，
旧历史会被归到新账号名下。这是可接受的——那些数据本来就是全局混在一起的，
认领只是给它们一个确定的归属，而不是声称这个归属是历史真相。文档里要写。

认领之后不应再有 NULL 行，因此 `list_history` 用 `WHERE t.account_id = ?1`
不会漏行。但认领本身要幂等（`WHERE account_id IS NULL` 天然幂等）。

## 4. 实施步骤

1. **`db.rs`**：`create_schema` 的 `transfer_tasks` 加 `account_id TEXT` 列；
   `init_database` 加 ALTER 迁移（忽略重复列错误，与既有风格一致）。
2. **`db.rs`** 新增 `claim_unowned_history(conn, account_id)`，执行上面那条 UPDATE。
3. **`lib.rs`**：读到 `initial_settings` 之后立刻调用认领；
   `cmd_save_settings` 切换账号时**不**调用——切账号不该把旧账号的历史搬过来。
4. **`history_repo.rs`**：`record_task` 增 `account_id` 参数并写入；
   `list_history` / `select_prune_candidates` 增 `account_id` 参数与过滤条件。
   每个函数把 `account_id: &str` 作为**显式首参**，不从全局读——
   显式参数让「忘了过滤」成为编译错误，读全局只会让它成为运行期的静默串台。
   这与上一轮 registry 改复合键是同一条理由。
5. **`cache_manager.rs`**：`prune_history` 增 `account_id` 参数并透传；
   `sweep` 不动，两处注释互相指向说明作用域差异。
6. **`history_pruner.rs`**：`prune_and_notify` 已经在取 settings 拿 `limit`，
   同一处顺带取 `account_id`。
7. **`history_cmd.rs`**：`cmd_list_history` 从 `state.settings` 取账号后传入。
8. **`transfer_engine.rs`**：`record_task` 的调用点补账号（它已能拿到 settings）。

## 5. 测试

沿用既有风格：中文 `t.Fatalf` / `assert!` 文案，每个用例上方注释说明
「这条断言被删掉之后会退回成什么缺陷」。全部落在 `history_repo.rs` 与
`cache_manager.rs` 的 `mod tests`。

| 用例 | 守护什么 |
| :--- | :--- |
| `list_history_only_returns_current_account` | **核心负例**，去掉过滤即红 |
| `list_history_does_not_leak_other_account_previews` | 断言连 preview_summary 都取不到——泄露的正是文件名与摘要 |
| `prune_counts_per_account_not_globally` | A 账号狂传不得挤掉 B 的历史。这条钉死 §2.1 的决定 |
| `prune_never_touches_other_account_rows` | 修剪不得删到别人的行 |
| `sweep_quota_stays_global` | **正例**：容量清理仍按全局配额，不因账号而分片 |
| `claim_assigns_legacy_rows_to_current_account` | 迁移认领：老库 NULL 行归当前账号 |
| `claim_is_idempotent_and_does_not_resteal` | 二次认领不得把已归属的行改走——否则切账号就会搬走旧账号的历史 |
| `switching_account_hides_but_keeps_history` | 切账号后旧历史不可见但**仍在库里**，切回来还能看到 |
| `delete_sessions_still_clears_all_four_tables` | **既有正例**，确认改造没破坏级联清理 |

## 6. 明确不做

- **不加密本地库**。静态加密是独立议题，与分区无关。
- **不给缓存文件分账号目录**（理由见 §2.4）。
- **不迁移 E2EE / 配对**，仍属 Phase 3。
- **不改服务端**，本轮纯客户端。

## 7. 验证

```bash
cd client/src-tauri && cargo test
cd client && npm test && npx tsc --noEmit
cd server && go test ./...      # 应无变化，确认没误伤
```

手工端到端（自动化覆盖不到「真的切一次账号」）：

1. 账号 A 传几个文件 → 历史里可见。
2. 设置里切到账号 B → 历史面板应当**为空**，且「重新装载」无从触发 A 的文件。
3. 切回账号 A → 历史**原样回来**（不是被删了）。
4. 用老版本建的库升级后首次启动 → 存量历史归到当前账号且可见。

## 8. 给审查员的重点提示

1. §2.1 的三个作用域划分是否站得住——特别是 `cache_max_size_mb` 保持全局
   是否会让一个账号占满磁盘后饿死其他账号，以及那是否可接受。
2. §2.2 只给父表加列，`select_prune_candidates` 的关联改写是否有遗漏路径。
3. §2.3 `reset_stale_in_flight` 保持全局的论证是否成立。
4. §3 的认领时机：放在 `lib.rs` 读 settings 之后、而不放在 `cmd_save_settings`
   的切换路径里，是否真能避免「切账号搬走旧历史」。
5. 「不加过滤」的那批函数（§1 表格下半）是否真的安全——尤其
   `get_task_brief`，它的 session_id 来自对端信令。
6. **本轮同时覆盖上一轮遗留的两处加固**（`connection_actor.rs` 的
   `install_default()` 位置、`USER_GUIDE.md` 的 v3 证书说明），它们在
   `--base 47766fb` 的 diff 里，未经任何双审。
