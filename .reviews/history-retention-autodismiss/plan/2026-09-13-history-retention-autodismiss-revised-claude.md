---
schema: trivium.disposition.v1
topic: history-retention-autodismiss
stage: plan
role: claude
kind: revised
run_id: 20260912T234849Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - AGY-04
  - AGY-05
  - GLM-01
  - GLM-02
  - GLM-03
  - GLM-04
  - GLM-05
  - GLM-06
---

# 修订计划：传输历史条数上限 + 完成卡片自动消失

- 主题：`history-retention-autodismiss`
- 阶段：plan（修订稿，含逐条裁决）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`2026-09-13-history-retention-autodismiss-gemini.md`、
  `2026-09-13-history-retention-autodismiss-glm.md`

---

## 0. 裁决摘要

两侧均判 `request-changes`，共 11 条意见，**全部接受**，无驳回、无暂缓。

11 条收敛为 6 处真实缺陷，其中 3 处被两侧从不同角度各自命中：

1. **保存设置后历史界面不刷新**（AGY-01 阻断 + GLM-01 重要）——修剪在后端发生了，
   前端 `HistoryPanel` 没有任何感知路径，原稿 §4.3 步骤 2 的验收条件按原改动清单
   **必然失败**。这是唯一的阻断项，也是原稿真正漏掉的一环。
2. **删除顺序自相矛盾**（AGY-03 重要 + GLM-06 吹毛求疵）——原稿 §2.3 与 §3.2
   写了两种相反的顺序，且 §3.2 那种会留下不可回收的磁盘孤儿。
3. **FAILED 卡片该不该自动消失**（AGY-04 + GLM-04）——原稿无差别处理全部终态，
   超出了需求「完成后」的字面范围，且未留决策痕迹。

另外 3 处各由一侧单独命中：

4. **终态触发点的锁时序含糊**（AGY-02）——原稿 §3.3 的措辞可以被读成
   「在 `update_history_status` 函数体内触发」，那样会自死锁。见 §1 的核验说明。
5. **「终态是唯一增长时刻」前提不成立 + 死行永久占额度**（GLM-02），
   以及**遗漏 `lib.rs` 的 TRANSFER_FAILURE 终态写入点**（GLM-03）。
6. **UI 显示上限与数据库淘汰上限未解耦**（AGY-05）、**数值校验不完整**（GLM-05）。

> 对 AGY-02 与 GLM 的分歧作了独立核验（两侧结论相反，不能直接照抄任一侧），
> 结论写在 §1，裁决表中 AGY-02 的理由据此给出。

---

## 1. 关于 AGY-02 与 GLM 的结论冲突 —— 独立核验

两侧在同一处给出了**相反**的判断，故不能直接采信任一侧，已自行核验：

- AGY-02 断言：在终态处触发修剪会因 `tokio::sync::Mutex` 不可重入而**自死锁**；
- GLM 在「认为正确的部分」第 4 条断言：各触发点 guard 均在作用域内释放，
  **修剪再取锁不会自锁**。

核验 `client/src-tauri/src/core/transfer_engine.rs:51-57`：

```rust
async fn update_history_status(app_handle: &AppHandle, session_id: &str, status: &str, error: Option<&str>) {
    let state = app_handle.state::<AppState>();
    let conn = state.db_conn.lock().await;   // ← guard 在此获取
    if let Err(e) = HistoryRepo::update_task_status(&conn, session_id, status, error) {
        log::warn!("Failed to update history status for {}: {}", session_id, e);
    }
}                                             // ← guard 在此释放
```

**两侧都对，只是前提不同**：guard 的作用域就是这个函数体。

- 在**函数体内**（`conn` 仍活着时）调用修剪 → 二次 `lock().await` 同一 Mutex → 永久死锁，
  AGY-02 成立；
- 在**调用点之后**（`update_history_status(...).await` 返回后，如 `transfer_engine.rs:506`）
  调用修剪 → 锁已释放 → 不会死锁，GLM 成立。

问题出在原稿 §3.3 的措辞「在任务落终态的位置触发」——这句话两种读法都通。
**含糊本身就是缺陷**，故接受 AGY-02，落点是把措辞改成不可误读的形式（见 §2 表与 §3.3）。

`lib.rs:270-276` 的 TRANSFER_FAILURE 分支同理：那里的 guard 在一个显式 `{ }` 块内，
块结束即释放，修剪必须放在块**之后**。

---

## 2. 逐条裁决

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | 阻断 | 接受 | 已核验属实：`HistoryPanel.tsx:38-49` 的 `refresh` 只由挂载、`transfer-progress` 终态事件、手动按钮触发；`App.tsx` 的 `handleSaveSettings` 保存成功后只调 `fetchInitialData()`，而它只拉 self_info / online_devices / settings，不碰历史。原稿 §4.3 步骤 2「保存后立即只剩 3 条」按原改动清单无法达成，§2.5 触发点 2 的动机落空。 | 新增 §3.7：后端修剪实际删除后 `emit("history-pruned", 删除条数)`，`HistoryPanel` 增加对该事件的监听并静默 `refresh(true)`；事件在**锁释放后**发出。 |
| AGY-02 | gemini | 重要 | 接受 | 死锁不是必然的（见 §1 核验：guard 的作用域是 `update_history_status` 函数体，调用点之后取锁安全），但原稿 §3.3「在任务落终态的位置触发」的措辞两种读法都通，落到函数体内就是永久死锁。含糊本身即缺陷，必须消解；AGY-02 关于「持锁做同步磁盘 IO 会阻塞」的第二点也成立。 | 改写 §3.3：明确写死「在 `update_history_status(...).await` **返回之后**触发，禁止写入其函数体内」，并在 §3.2 约定 `prune_history` 的 DB 事务与磁盘 IO 分两段、删文件时不得持有 `db_conn` 锁。 |
| AGY-03 | gemini | 重要 | 接受 | 属实且是原稿自身矛盾：§2.3 写「先 `remove_file` 再删行」，§3.2 却写「事务内删行 → 返回路径由调用方删文件」。后者一旦 `remove_file` 失败，文件即脱离 `cache_entries` 索引，而既有 `sweep_expired_and_lru` 以表行为遍历源（`cache_manager.rs:106-127`），永远扫不到它 ⇒ 不可回收的磁盘孤儿。反向的悬空行则可自愈（下轮修剪重试，且 `history_cmd.rs` 已处理文件缺失）。 | 重写 §2.3 与 §3.2 为统一顺序：**先删磁盘文件，确认后再在事务内删四张表的行**，与既有 sweep 一致。同时删除原 §2.3「先子后父」那段理由——它只适用于表间关系，被误用到了文件与行之间。 |
| AGY-04 | gemini | 次要 | 接受 | 属实。`docs/需求.md:14` 原文只说「传输任务完成后」，原稿把 FAILED 一并纳入定时移除，既超出需求字面范围，也未留决策痕迹。失败卡片是用户查看失败原因的主要入口（错误 toast 仅 4 秒且只出现一次）。 | §2.1 与 §2.7 改为：**仅 COMPLETED 注册定时器，FAILED 卡片不自动消失**，只能手动点 X 移除；在 §2.7 写明该决策与理由。§3.6 的判据同步从「终态」收窄为 `status === "COMPLETED"`。 |
| AGY-05 | gemini | 次要 | 接受 | 属实且原稿 §3.3 的方案会放大它：`history_max_entries = 0`（不限制）时 `cmd_list_history` 全量返回，而 `HistoryPanel.tsx:137-234` 无虚拟滚动、全量渲染 DOM，长期使用必然卡死。淘汰策略与显示策略本就是两回事，不该共用一个数。 | §3.3 改为解耦：`cmd_list_history` 使用独立的显示上限常量 `HISTORY_DISPLAY_CAP = 200`（不进设置面板），`history_max_entries` 只用于修剪。这同时消掉了原稿「0 要传 -1、否则 `LIMIT 0` 返回空集」的坑——`LIMIT` 参数不再来自设置。 |
| GLM-01 | glm | 重要 | 接受 | 与 AGY-01 同一缺陷、同一依据（两侧独立命中），不重复裁决，合并到同一落点。GLM 额外指明了可仿照的既有模式（`devices-updated`），采纳。 | 同 AGY-01 的落点 §3.7；事件命名与派发方式仿 `settings_cmd.rs:88` 的 `devices-updated` 模式。 |
| GLM-02 | glm | 次要 | 接受 | 已核验属实：`record_task` 在任务**开始**时即以 `TRANSFERRING` 落库（`lib.rs:193` 收、`clipboard_cmd.rs:149` 发），故原稿 §2.5「终态是唯一会让条数增长的时刻」是错的。进程中途被杀会留下永久停在 TRANSFERRING 的死行，而 §2.4 约束 A 每轮都跳过它 ⇒ 永久占用保留额度。另核验 GLM 关于「无续传路径」的论据成立：全仓 `BitmapRepo` / `chunk_bitmaps` 除建表语句外**无任何生产引用**，启动期复位不会破坏在途状态。 | §2.5 修正增长时刻表述（改为「任务开始时落库、终态时收敛」）；新增 §2.8：启动修剪**之前**先把库中残留的 `TRANSFERRING` / `PENDING` 行复位为 `FAILED`（error_message 记「进程异常退出」），使其可被后续修剪回收。§4.1 增一条对应单测。 |
| GLM-03 | glm | 次要 | 接受 | 已核验属实：`lib.rs:274` 的 TRANSFER_FAILURE 分支独立调用 `HistoryRepo::update_task_status(..., "FAILED", ...)`，不经过 `transfer_engine::update_history_status`。原稿 §3.3 只点名 `transfer_engine.rs`，会漏掉这条对端报错路径。 | §3.3 的触发点改为枚举**全部终态写入点**：`transfer_engine.rs` 的 `update_history_status` 调用簇（258/506/520/590/819/855）与 `lib.rs:270-276` 的 TRANSFER_FAILURE 分支，后者同样在其 `{ }` 块**之外**触发（见 §1）。 |
| GLM-04 | glm | 次要 | 接受 | 与 AGY-04 同一缺陷（两侧独立命中），不重复裁决。GLM 补充的量化对比（失败 toast 仅 4 秒 vs 卡片 30 秒后消失）强化了「FAILED 应豁免」的判断，采纳同一处置。 | 同 AGY-04 的落点 §2.1 / §2.7 / §3.6。 |
| GLM-05 | glm | 吹毛求疵 | 接受 | 属实：原稿 §3.5 的校验清单只列了空串 / 非数字 / 负数，漏了非整数与超 `u32::MAX`。输入 `2.5` 会以 JSON `2.5` 提交，Rust 侧 `u32` 反序列化失败 ⇒ `cmd_save_settings` 返回笼统的「保存设置失败」，用户看不出是哪个字段的问题。number input 默认允许小数。 | §3.5 校验收紧为「0 到 4294967295 的整数」，两个 input 补 `min=0 step=1`，并在前端给出字段级错误提示（不依赖后端的整单错误）。 |
| GLM-06 | glm | 吹毛求疵 | 接受 | 属实。其指出的孤儿风险与 AGY-03 同源，而按 AGY-03 改成「先删文件后删行」后，`remove_file` 失败时行会保留、下轮可重试，孤儿场景自然消解。但「单文件删除失败如何处理」仍需写明，否则实现时容易写成整批中断。 | 落到 §3.2 改写中：单个 `remove_file` 失败只记 `log::warn!` 并**跳过该会话的删行**（保留行以便下轮重试），不影响同批其余会话，与既有 sweep 忽略删除失败的容忍度一致但更保守。 |

**裁决统计**：接受 11 ｜ 驳回 0 ｜ 暂缓 0。

---

## 3. 修订后的计划（仅列相对原稿的变更）

原稿 `2026-09-13-history-retention-autodismiss-claude.md` 的 §1（现状核验）、§2.2
（serde default）、§2.4（两条硬约束）两侧均确认无误，**整体保留**。以下是变更部分。

### 3.1 §2.1 默认值表 —— 收窄 `transfer_card_retain_secs` 的适用范围（AGY-04 / GLM-04）

| 字段 | 类型 | 默认值 | 语义（修订后） |
|---|---|---|---|
| `history_max_entries` | `u32` | `100` | 传输历史最多保留的条数；`0` = 不限制 |
| `transfer_card_retain_secs` | `u32` | `30` | **COMPLETED** 卡片在界面保持的秒数；`0` = 不自动消失。**FAILED 卡片不受此设置影响，永不自动消失** |

设置面板对应文案需体现这一点，不能只写「完成任务在界面保持秒数」而让用户以为失败也算。

### 3.2 §2.3 / §3.2 删除顺序 —— 统一为「先文件后行」（AGY-03 / GLM-06）

`HistoryRepo::prune_to_limit` 拆成**两段**，中间释放锁：

1. **选取段**（持 `db_conn` 锁）：按 `created_at DESC, rowid DESC` 排序（与 `list_history`
   `history_repo.rs:118-119` 逐字一致），取第 `limit` 名之后的 `session_id`，
   过滤掉非终态（约束 A）与处于 2h 剪贴板免疫锁内（约束 B）的会话，
   连同它们的 `cache_entries.file_path` 一并返回。**返回即释放锁。**
2. **删文件段**（**不持锁**）：逐个 `fs::remove_file`。某个文件删除失败 → `log::warn!`
   并把该 `session_id` 从待删集合中**剔除**（保留其行，下轮重试）。
3. **删行段**（重新取锁，单事务）：对剩余 `session_id` 依次删
   `cache_entries` → `chunk_bitmaps` → `transfer_items` → `transfer_tasks`。

原 §2.3 中「先子后父，中途失败留下父在子少的可恢复状态」那段理由**删除**——
它描述的是表间顺序（第 3 段内部仍然成立），被原稿误用来论证文件与行的顺序。

> `chunk_bitmaps` 经核验当前恒为空表（无生产写入方），删它是防御性的，保留但不必测。

### 3.3 §3.3 触发点 —— 消解锁时序含糊并补齐遗漏（AGY-02 / GLM-03）

修剪的调用点**必须在锁释放之后**，逐处写死：

- `transfer_engine.rs`：在每个 `update_history_status(...).await` **返回之后**触发
  （258 / 506 / 520 / 590 / 819 / 855 六处）。
  **禁止**把修剪写进 `update_history_status` 函数体内 —— 那里 `conn` guard 还活着，
  再取同一 `tokio::sync::Mutex` 会永久死锁（§1）。
- `lib.rs:270-276` 的 TRANSFER_FAILURE 分支：在那个 `{ }` 块**之后**触发，同理。
- `cmd_save_settings`（`settings_cmd.rs`）：在设置落库与内存更新**之后**触发。
- `lib.rs` setup：启动时，先做 §3.6 的复位，再触发一次修剪。

`cmd_list_history` 的改动也在此节，但方向改了（见下）。

### 3.4 §3.3 显示上限与淘汰上限解耦（AGY-05）

- `cmd_list_history` **不再读** `history_max_entries`，改用模块内常量
  `HISTORY_DISPLAY_CAP: u32 = 200`（不进设置面板，纯防 DOM 爆炸的兜底）；
- `history_max_entries` 只用于 `prune_to_limit`。

如此原稿 §3.3 里「`0` 不能传给 `LIMIT`、否则返回空集」的坑随之消失（`LIMIT` 参数
不再来自用户设置），`list_history` 的 `limit: u32` 签名也不必改。

### 3.5 §3.5 数值校验收紧（GLM-05）

两个 number input 补 `min=0 step=1`；提交前校验「空串 / 非数字 / 负数 / 非整数 /
超 `4294967295`」五类，任一命中则**阻止提交并给出字段级错误提示**，
不依赖后端返回的整单「保存设置失败」。

### 3.6 新增 §2.8 启动期复位陈旧非终态行（GLM-02）

启动修剪**之前**执行一次：把 `transfer_tasks` 中状态为 `TRANSFERRING` / `PENDING`
的行复位为 `FAILED`，`error_message` 记「进程异常退出」。

安全性依据（已核验）：本仓库**不存在续传路径** —— `BitmapRepo` 与 `chunk_bitmaps`
除 `db.rs:62` 的建表语句外无任何生产引用，复位不会丢弃任何可恢复的在途状态。
进程刚启动时也不可能有真正在途的任务。

同时修正原 §2.5 的表述：历史行在**任务开始时**即以 `TRANSFERRING` 落库
（`lib.rs:193`、`clipboard_cmd.rs:149`），不是终态时才产生。

### 3.7 新增：修剪后的前端刷新（AGY-01 / GLM-01）

- **后端**：`prune_history` 在实际删除了 ≥1 条时，通过 `AppHandle::emit` 发出
  `history-pruned` 事件（payload 为删除条数）。派发**在锁释放之后**，仿
  `settings_cmd.rs:88` 的 `devices-updated` 模式。
- **前端**：`HistoryPanel.tsx` 在既有的 `transfer-progress` 监听旁增加
  `history-pruned` 监听，收到即 `refresh(true)`（静默刷新，复用现有函数，
  不显示 spinner）。卸载时与既有监听一并解绑。

不弹 toast —— 与原稿 §2.6 的判断一致：删掉的条目从列表消失本身就是反馈，
且约束 B 的静默跳过会让「我删了 N 条」的提示时准时不准。

### 3.8 §4 测试计划增补

§4.1 增三条 Rust 单测：

- 启动复位把 `TRANSFERRING` / `PENDING` 行改为 `FAILED`，且不触碰既有终态行（§3.6）；
- `prune_to_limit` 的选取段返回的 `session_id` 与 `list_history` 前 N 条**互斥且无交集**
  （守护两处排序键一致，§3.2 第 1 段）；
- 某文件删除失败时，该会话的行**被保留**而非一并删除（§3.2 第 2 段）。

§4.3 手工端到端增两步：

- 保存「保留条数 = 3」后，历史列表**无需手动刷新**即变为 3 条（验收 §3.7，
  这是原稿必然失败的那一步）；
- 制造一次失败传输，确认失败卡片**不会**自动消失，且能手动点 X 移除（验收 §3.1）。

### 3.9 §5「明确不做」增补

- 不为历史列表加虚拟滚动或分页 —— §3.4 的 `HISTORY_DISPLAY_CAP` 是本轮的兜底方案，
  真正的分页超出本轮范围；
- 不改 `HISTORY_DISPLAY_CAP` 为可配置项 —— 它是防御性常量，不是用户偏好。
