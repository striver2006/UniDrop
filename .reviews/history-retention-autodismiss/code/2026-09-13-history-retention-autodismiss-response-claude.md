---
schema: trivium.disposition.v1
topic: history-retention-autodismiss
stage: code
role: claude
kind: response
run_id: 20260913T002939Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - GLM-01
  - GLM-02
---

# 代码审查应答：传输历史条数上限 + 完成卡片自动消失

- 主题：`history-retention-autodismiss`
- 阶段：code（逐条应答与修改方案）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`code/2026-09-13-history-retention-autodismiss-gemini.md`、
  `code/2026-09-13-history-retention-autodismiss-glm.md`

---

## 0. 应答摘要

两侧均判 `approve-with-nits`，共 5 条意见，**全部接受**，无驳回、无暂缓。
无 blocker、无 major —— plan 阶段裁决的四个焦点（锁时序、删除顺序、FAILED 豁免、
前端刷新链路）两侧独立核验后都确认已正确落地。

5 条收敛为 4 处待办：

1. **删文件段零测试覆盖**（AGY-01 + GLM-01，两侧独立命中）——
   修订计划 §3.8 白纸黑字承诺了「单文件删除失败时该会话行被保留」这条单测，
   我没写。这是**我对自己计划的执行遗漏**，不是审查员的额外要求。
2. **`setTimeout` 32 位溢出**（AGY-02）—— 前端校验放行到 `u32::MAX`，
   而 `setTimeout` 延迟上限是 2147483647 ms。填大于约 2147483 秒（24.86 天）
   的值会溢出并被降级为 1ms，完成卡片瞬间闪退 —— 恰好是设置项语义的反面。
3. **免疫判据的 TOCTOU 窗口**（GLM-02）—— 三段式为了不持锁做磁盘 IO
   引入了一个无锁窗口，用户在窗口内点「装载」会让免疫保护失效。
4. **循环内重复 prepare**（AGY-03）—— 可读性与效率，顺手改掉。

---

## 1. 关于 GLM-02 的独立核验与补充

GLM-02 是本轮唯一需要我自己验证的一条，已核验 `clipboard_cmd.rs:212-252`：

`cmd_inject_session` 确实全程不与 `select_prune_candidates` 互斥 ——
它先取锁读 `data_type`（块内释放），再 `get_session_files`、`spawn_blocking`
写剪贴板，最后才 `mark_clipboard_injected` 取锁打标记。交错成立：

```
修剪第 1 段：选中 s1（此刻 s1 无免疫标记）→ 释放锁
                    ↓（无锁窗口）
用户点装载 s1：读文件 → 写剪贴板 → mark_clipboard_injected
                    ↓
修剪第 2 段：删掉 s1 的文件
修剪第 3 段：删掉 s1 的行
```

**两点补充，两侧都没说，但影响修法的取舍**：

1. **危害只对 `FILES` 类型成立。** `TEXT` / `IMAGE` 的装载是把**内容本身**
   （文本串、PNG 字节）写进系统剪贴板，文件删掉之后剪贴板照样能粘贴。
   只有 `FILES` 放的是文件**引用**，才会因删文件而失效。
2. **GLM 建议的复核只能减轻、不能根治。** 建议是在第 3 段删行前复核免疫标记，
   但那时**文件已经在第 2 段被删掉了**，复核只救得回「行」，救不回文件。
   结果从「行和文件都没了」变成「行在、文件没了」—— 后者是既有代码已经处理的
   可自愈状态（`cmd_save_transfer_as` / `cmd_reveal_session` 都会报
   「该会话在缓存中无文件（可能已被清理）」），历史条目也不会在用户刚用过之后
   凭空消失。

即便如此仍然接受：成本是一条 SQL，收益是把不可自愈状态降级为可自愈状态，
且保住了「用户刚碰过的记录不会消失」这个可见行为。
彻底根治要么持锁做 IO（正是三段式要避免的），要么引入会话级锁（本轮不值当）。
**实现时必须把这个局限写进注释**，否则后人会误以为这个窗口已经关死了。

---

## 2. 逐条裁决

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | 次要 | 接受 | 属实，且这是我对自己计划的执行遗漏：修订计划 §3.8 明确承诺「某文件删除失败时，该会话的行被保留而非一并删除」这条单测。实现时 9 条测试全落在 `history_repo`（纯 SQL 层），`cache_manager.rs` 至今没有 `#[cfg(test)]` 模块，三段式的第 2 段（`all_removed` 判定、失败剔除、`deletable` 为空提前返回）零覆盖。而这段正是 AGY-03/GLM-06 在 plan 阶段争出来的容错逻辑，没测试守护等于随时可能被改回去。 | 新增 `cache_manager.rs` 的 `#[cfg(test)] mod tests`：用 `tempfile` 建临时目录与内存库，构造两个候选会话，其一的 `file_path` 指向一个**目录**（`remove_file` 对目录必然失败），断言失败会话的行保留、成功会话的行被删、返回值为 1。 |
| AGY-02 | gemini | 次要 | 接受 | 属实且是真实可触发的用户可见缺陷。`parseU32` 放行到 4294967295，而 `setTimeout` 的延迟以 32 位有符号整数存储，上限 2147483647 ms。填任何大于 2147483 秒（约 24.86 天）的值，`secs * 1000` 溢出后被降级为 1ms，完成卡片在出现的瞬间就消失——用户想要的是「留久一点」，得到的却是「立刻消失」，语义完全反转，且没有任何报错提示。 | 双保险：①`SettingsModal.tsx` 给保持秒数加业务上限 `86400`（24 小时），超出给字段级错误提示；②`App.tsx` 的 `scheduleAutoDismiss` 里对延迟做 `Math.min(secs * 1000, 2147483647)` 截断兜底，防止上限校验日后被绕过。两处都写明理由。 |
| AGY-03 | gemini | 吹毛求疵 | 接受 | 属实。`select_prune_candidates` 在 `for session_id in session_ids` 循环体内反复 `conn.prepare` 同一条 SQL，每轮迭代 prepare 一次又随作用域 drop，候选多时是纯浪费。改掉的成本极低，没有保留理由。 | `history_repo.rs` 的 `select_prune_candidates`：把 `prepare` 提到循环外复用同一个 `Statement`。**不**改用 JOIN——JOIN 后要在 Rust 侧按 `session_id` 分组重组，反而比现在这种「先拿 id 列表、再逐个取路径」更难读，且无候选文件的会话会因 INNER JOIN 丢失、需要 LEFT JOIN 兜底，得不偿失。 |
| GLM-01 | glm | 次要 | 接受 | 与 AGY-01 同一缺陷、同一依据（两侧独立命中），不重复裁决，合并到同一落点。GLM 额外给出了更具体的构造方式（让路径指向一个目录使 `remove_file` 必然失败），采纳——这比模拟只读文件更可靠，不受运行用户权限（如 root）影响。 | 同 AGY-01 的落点。测试构造采用 GLM 的「路径指向目录」方案。 |
| GLM-02 | glm | 次要 | 接受 | 已独立核验属实（见 §1）：`cmd_inject_session` 全程不与选取段互斥，免疫判据只在第 1 段判定一次，第 2、3 段都不复核。既有 `sweep_expired_and_lru` 因 guard 贯穿全函数而天然免疫，是三段式为了不持锁做 IO 才重新引入这个窗口——这个因果链条 GLM 说得准确。接受，但需记录两点局限：危害只对 FILES 类型成立（TEXT/IMAGE 写的是内容本身，删文件不影响粘贴），且复核只能救回「行」、救不回已在第 2 段删掉的文件，属于把不可自愈状态降级为可自愈状态，不是根治。 | `cache_manager.rs` 的 `prune_history` 第 3 段：重新取锁后、调 `delete_sessions` 前，用同一免疫判据对 `deletable` 复核一次，剔除窗口期内被装载的会话；复核与删行处在同一段连续持锁区间内（`mark_clipboard_injected` 需要同一把锁，无法再交错）。新增 `HistoryRepo::filter_recently_injected`（纯 SQL）承载该判据，与选取段共用 `CLIPBOARD_LOCK_SECS` 常量。**注释必须写明**：本复核关的是「行被删」，文件已在第 2 段删除，无法挽回。 |

**裁决统计**：接受 5 ｜ 驳回 0 ｜ 暂缓 0。

---

## 3. 修改方案（待审批后执行）

按契约，闸门期内不动代码。以下是审批通过后要做的全部改动。

### 3.1 `client/src-tauri/src/storage/history_repo.rs`

- `select_prune_candidates`：`prepare` 提到循环外复用（AGY-03）；
- 新增 `filter_recently_injected(conn, session_ids) -> Result<Vec<String>, rusqlite::Error>`：
  返回**不在**免疫窗口内的会话（即仍可安全删行的），判据与选取段逐字一致，
  共用 `CLIPBOARD_LOCK_SECS`（GLM-02）；
- 新增单测：窗口期内被打上 `clipboard_injected_at` 的会话会被 `filter_recently_injected` 剔除。

### 3.2 `client/src-tauri/src/core/cache_manager.rs`

- `prune_history` 第 3 段：取锁后先 `filter_recently_injected` 复核再 `delete_sessions`，
  注释写明「本复核只防行被误删；文件已在第 2 段删除，救不回来」（GLM-02）；
- 新增 `#[cfg(test)] mod tests`（AGY-01 / GLM-01），至少两条：
  - 某会话的登记路径指向目录 ⇒ `remove_file` 失败 ⇒ 该会话行保留、返回值只计成功的那个；
  - 窗口期内被装载的会话在复核后不被删行。

`tempfile` 已在 `[dev-dependencies]` 中的话直接用；没有则本轮加入（仅 dev）。

### 3.3 `client/src/components/SettingsModal.tsx`

保持秒数的校验增加业务上限 `86400`（24 小时），超出给字段级错误
「最多 86400 秒（24 小时）」。保留条数不设此上限——它不喂给 `setTimeout`，
`u32` 范围内都合法（AGY-02）。

### 3.4 `client/src/App.tsx`

`scheduleAutoDismiss` 的延迟改为 `Math.min(secs * 1000, 2147483647)`，
注释说明这是防 32 位溢出的兜底，与 SettingsModal 的上限校验互为双保险（AGY-02）。

### 3.5 验证

- `cargo test`（预期 33 → 36 条左右全绿）；
- `npm run build` + `npx tsc --noEmit`；
- 手工核对：保持秒数填 `99999999` 应被前端拦下并给出字段级提示，而不是保存成功后卡片闪退。

---

## 4. 仍未覆盖（如实记录）

- **端到端手工验证一次都没做过**（修订计划 §4.3 的 7 步）。两侧审查员也都把
  「未实际启动 Tauri 窗口做人工交互验证」列为本侧盲区。这部分需要人工在真机上跑。
- GLM-02 的窗口按 §1 只是**收窄**而非关闭：FILES 类型在窗口期被装载后，
  剪贴板里的文件引用仍会失效，只是历史条目不再凭空消失。
