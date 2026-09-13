---
schema: trivium.disposition.v1
topic: local-store-account-scope
stage: plan
role: claude
kind: revised
run_id: 20260913T103846Z
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

# 修订计划：本地历史与缓存按账号分区

- 主题：`local-store-account-scope`
- 阶段：plan（修订稿，含逐条裁决）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`2026-09-13-local-store-account-scope-gemini.md`、
  `2026-09-13-local-store-account-scope-glm.md`

---

## 0. 总览

两侧均判 `request-changes`：Antigravity 5 条（阻断 1 ｜ 重要 2 ｜ 次要 2），
ZCode 6 条（重要 2 ｜ 次要 2 ｜ 吹毛求疵 2）。合计 11 条，**全部接受**，
无驳回、无暂缓。

**三条被两侧独立发现**，都是我的实质错误：保留窗口子查询漏过滤、
`record_task` 调用点写错文件、认领时未防非法账号。

两件比单条意见更值得记的事：

**其一：AGY-03 与 AGY-04 单看都是 major/minor，合起来是一条完整的事故链。**
切账号后前端历史面板不刷新（AGY-03，旧账号卡片残留）→ 用户点「重新装载」
（AGY-04，三个 IPC 命令不校验归属）→ 真的把上一个账号的文件写进了剪贴板。
这恰恰是本计划 Context 里描述的那个泄露场景——**我规划的 `list_history` 过滤
只堵住了"看得见"，没堵住"点得动"**。而 ZCode 在「认为正确的部分」里判定前端
入口安全，理由是「按钮只作用于可见行」——这个前提正被 AGY-03 推翻。两侧在这里
各自只看到一半。

**其二：GLM-02 指出我设计的测试测不出我自己要修的缺陷。** 这是本轮质量最高的
一条。外层单过滤的失败形态是「删掉 A 自己应保留的行」，而非「删到 B 的行」，
且只在两账号行时间交错时才显现——我列的
`prune_counts_per_account_not_globally` 与 `prune_never_touches_other_account_rows`
按其命名与守护描述都抓不住。上一轮我刚吃过「假绿测试」的亏（断言被兜底逻辑
覆盖），这次是同一类问题的另一副面孔。

---

## 1. 裁决表

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | 阻断 | 接受 | 与 GLM-02 同一缺陷，两侧独立发现，成立。`select_prune_candidates` 的保留窗口子查询 `NOT IN (SELECT session_id FROM transfer_tasks ORDER BY created_at DESC LIMIT ?1)` 直接扫全表，若不加账号条件，账号 B 的新行会占满窗口名额，导致账号 A 自己最新 N 条之内的行被判超限删掉。这不是「删到别人的行」，而是**静默删掉自己该留的行**，比我原本设想的失败形态更隐蔽。 | §2.1 明确两处过滤各自的位置与理由 |
| AGY-02 | gemini | 重要 | 接受 | 事实错误，已 grep 核实：`record_task` 的生产调用点是 `lib.rs:194`（RECEIVE）与 `clipboard_cmd.rs:254`（SEND），`transfer_engine.rs` 里一个都没有（它只调 `update_task_status`）。我写错了文件名，而且 `clipboard_cmd.rs` 全计划未提及。显式参数会让漏改成为编译错误、不至于静默出错，但改动面清单点错文件会让实施者先扑一次空。 | §2.3 更正步骤 8 |
| AGY-03 | gemini | 重要 | 接受 | 属实且是我的规划缺口。`HistoryPanel` 只监听 `history-pruned`，而 `prune_and_notify` 在 `Ok(0)` 时不广播；`handleSaveSettings` 保存后也不重拉历史。于是切到一个无需修剪的新账号后，界面上仍是旧账号的卡片——与我 §7 写的验证预期「历史面板应当为空」直接矛盾。更要紧的是它与 AGY-04 组成完整事故链（见 §0）。 | §2.2 |
| AGY-04 | gemini | 次要 | 接受 | **我认为它被低估了，按事故链的一环对待。** `cmd_inject_session` / `cmd_save_transfer_as` / `cmd_reveal_session` 直接拿前端传来的 session_id 读文件、写剪贴板、另存为，不校验归属。ZCode 判定这条路径安全，前提是「按钮只作用于可见行」——而 AGY-03 恰恰证明了切账号后可见行里就有别人的。纵深防御在这里不是冗余：前端状态的正确性不该成为文件访问控制的唯一依据。 | §2.2 |
| AGY-05 | gemini | 次要 | 接受 | 与 GLM-01 同一缺陷。认领用的 `initial_settings.account_id` 可能是空串（老库的非法账号能原样反序列化，这是上一轮我亲手加测试钉死的行为）或 `default_user`（settings 解析失败时回落）。 | §2.4 |
| GLM-01 | glm | 重要 | 接受 | 与 AGY-05 同一件事，但它把后果推到了底，这一层我和 AGY 都没看到：认领的幂等性建立在 `WHERE account_id IS NULL` 上，**行一旦被空串认领就再也不会被重新认领**，而 `validate_account_id` 又让空账号永远无法成为当前账号——存量历史就此在应用内永久不可见，只能手工改库。它还引用 `需求.md:88` 证明空账号在这个项目里真实存在过。 | §2.4 |
| GLM-02 | glm | 重要 | 接受 | 与 AGY-01 同一缺陷，但多出两层，都成立：**①** 我 §2.2 那句「`cache_entries` 子查询要经 `transfer_tasks` 关联才能知道账号」本身是错的——那个 `NOT EXISTS` 免疫子查询本来就以 `c.session_id = t.session_id` 与外层关联，外层加条件即覆盖，无需任何改写；真正承载每账号语义的是我没提到的保留窗口子查询。**②** 我列的两条修剪测试测不出这个缺陷（分析见 §0）。 | §2.1 重写该段并补测试 |
| GLM-03 | glm | 次要 | 接受 | 与 AGY-02 同一件事，合并处理。它额外给出了两处调用点各自的账号来源（`settings_ref` / `state.settings`），省了实施时再找一遍。 | 并入 §2.3 |
| GLM-04 | glm | 次要 | 接受 | **半对半错，但它对的那半我确实写错了，所以接受并拆分措辞。** 错的那半：它推测失败可能源于 SHA1 签名协商——实测错误文本是 `invalid peer certificate: Other(OtherError(UnsupportedCertVersion))`，rustls 明确报的是证书版本，不是签名方案，所以「解析阶段拒绝 v1」有实测依据（它自己标注了置信度 low 并说明未能实机复现，是诚实的）。对的那半：**insecure 档的 verifier 无条件放行，根本不查名称，所以 SAN 对勾选后的场景毫无作用。** 而我的文档把「v3」和「带 SAN」并列成同一个必要条件，这个机制表述确实会把排障引向错路。 | §2.5 |
| GLM-05 | glm | 吹毛求疵 | 接受 | 说中了我的自相矛盾：§4.4 我自己立的标准是「让误操作成为编译错误或测试红」，而 §2.3 的「`reset_stale_in_flight` 保持全局」只安排了注释——将来有人给它加上账号过滤，现有单账号测试不会红，死行滞留要等真实多账号用户撞上才发现。一条决策若只靠注释守护，就等于没有守护。 | §2.6 |
| GLM-06 | glm | 吹毛求疵 | 接受 | 属实。我在 §3 和 §2.1 两处承诺了要写文档，实施步骤里却一条文档项都没有。尤其 `history_max_entries` 从全局语义改成每账号，是**用户可见的设置含义变更**（多账号时库内总条数会超过设定值），`需求.md` 第 4 条与 USER_GUIDE 的现有表述都按全局理解。按 §4 逐条执行的话大概率漏掉。 | §2.7 |

---

## 2. 修订内容

### 2.1 `select_prune_candidates` 需要**两处独立**过滤（对应 AGY-01 / GLM-02）

替换原 §2.2 中关于该查询的整段描述。原文那句「`cache_entries` 子查询要经
`transfer_tasks` 关联才能知道账号，加一个条件即可」**删掉**——它既指错了子查询，
又把改动说小了。

该 SQL 是三段结构，账号化要动的是其中两段：

| 段 | 作用 | 改动 |
| :--- | :--- | :--- |
| 外层 `WHERE t.status IN (...)` | 圈定本账号的可删候选 | **加** `AND t.account_id = ?` |
| `NOT EXISTS` 剪贴板免疫子查询 | 排除刚装载过的会话 | **不动**——已由 `c.session_id = t.session_id` 与外层关联，外层收窄即自动收窄 |
| `NOT IN` 保留窗口子查询 | 「最新 N 条」的窗口本身 | **加** `WHERE account_id = ?`——这才是「每账号前 N 条」的承载点 |

漏掉第三段的失败形态要写进注释，因为它反直觉：**不是删到别人的行，而是删掉
自己该留的行**。账号 B 的新行占满全局窗口后，账号 A 的修剪会把 A 自己最新 N 条
之内的行判成超限，直接违反 `history_repo.rs` 既有测试守护的「候选集与可见前 N 条
不得相交」这一不变量。

**测试补一条** `prune_window_is_per_account_when_rows_interleave`：两个账号的行
在时间上**交错**（B 有若干更新的行占位），以 A 为当前账号修剪，断言 A 自己
保留窗口内的行一条都没少。GLM-02 指出这一点很关键——时间不交错时两种实现结果
相同，用例必须刻意构造交错才有鉴别力。原计划那两条测试保留，但它们守的是
另一件事（不越界删别人的行），不能替代这条。

### 2.2 切账号的前端刷新 + IPC 归属校验（对应 AGY-03 / AGY-04）

这两条合并处理，因为它们是同一条事故链的两端，各修一半都不够。

**前端刷新**：`cmd_save_settings` 在账号实际发生变化时广播一个事件
（如 `account-changed`），前端收到后重拉 `cmd_list_history`。不复用
`history-pruned`——那是「修剪删掉了东西」的语义，而切账号时可能一条都没删
（`prune_and_notify` 在 `Ok(0)` 时本就不广播，这正是 AGY-03 的触发条件）。

**IPC 归属校验**：`cmd_inject_session` / `cmd_save_transfer_as` /
`cmd_reveal_session` 在动文件之前，先核验该 session 的 `account_id` 与当前账号
一致，不一致则返回错误。

注释要写明为什么这不是冗余：前端状态的正确性不能成为文件访问控制的**唯一**
依据。界面残留、事件丢失、用户在刷新前抢先点击——任何一种都会让「按钮只作用于
可见行」这个前提失效，而失效的后果是把别人的文件写进剪贴板。校验本身是一次
按主键的查询，代价可忽略。

### 2.3 更正 `record_task` 的调用点（对应 AGY-02 / GLM-03）

原 §4 步骤 8 作废。实际要改的是：

- `client/src-tauri/src/lib.rs:194`（TRANSFER_OFFER 分支，RECEIVE 方向）——
  账号取自同循环内已在使用的 `settings_ref`；
- `client/src-tauri/src/commands/clipboard_cmd.rs:254`（`dispatch_offer`，SEND 方向）——
  账号取自 `state.settings`。

`transfer_engine.rs` 不在改动范围内（它只调 `update_task_status`，而那个函数
按 §1 表格不加账号参数）。

### 2.4 认领前必须校验账号合法性（对应 AGY-05 / GLM-01）

认领前先过 `validate_account_id`，非法（含空串）则**本次跳过**：NULL 行原样留着，
推迟到某个合法账号启动时再认领。推迟是安全的——认领的幂等性正建立在
`WHERE account_id IS NULL` 上，跳过一次不会丢失任何东西。

反过来则不可逆：一旦用空串认领，那些行既不会被再次认领（不再是 NULL），
也永远不会被看见（空账号无法通过 `validate_account_id`，因而永远不可能成为
当前账号）。这个不可逆性要写进注释——它是「跳过」而不是「用兜底值认领」的
全部理由。

`record_task` 写入账号时采用同一防御。

### 2.5 拆开 v3 与 SAN 两个条件（对应 GLM-04）

现有 USER_GUIDE 措辞把两者并列成同一个必要条件，机制上不对。修正为：

- **X.509 v3：两档都必须**。rustls 在解析阶段就拒绝 v1，早于任何校验逻辑，
  所以勾选「允许不安全连接」也救不回来。实测错误文本：
  `invalid peer certificate: Other(OtherError(UnsupportedCertVersion))`。
- **SAN：只有默认档需要**。它服务于主机名校验，而勾选后装载的
  `InsecureServerCertVerifier` 根本不查名称。

给出的 `openssl -addext` 命令保持不变——它一次同时满足两个条件，对两档都适用，
仍是推荐做法。改的只是「为什么」，不是「怎么做」。

### 2.6 给「保持全局」的决策配一条测试（对应 GLM-05）

§5 测试表补 `reset_stale_in_flight_covers_all_accounts`：库中两个账号各有一条
死行，以账号 B 为当前账号执行启动复位，断言**A 的死行也被复位为 FAILED**。

这样「刻意不加账号过滤」这个决策就从注释里的一句话变成了会红的断言。
上一轮的教训是测试可能假绿；这一轮的教训是**没有测试的决策等于没有决策**。

### 2.7 实施步骤补文档项（对应 GLM-06）

§4 增加一步（放在代码改动之后、验证之前）：

- `docs/USER_GUIDE.md`：`history_max_entries` 说明为**每账号**上限；
  缓存容量与 TTL 说明为**全局**；按 §2.5 拆开 v3/SAN。
- `docs/需求.md`：第 4 条的表述按全局条数理解，需要补注多账号下的实际语义
  （库内总条数可能超过设定值，因为每个账号各留 N 条）。
- 存量行归属「升级后首个启动的账号」这一模糊性，以及非法账号时跳过认领的
  行为，都要写进用户可见的文档，而不只是代码注释。

---

## 3. 未改动的部分

以下经两侧核实成立，保持原计划不变：§2.1 三条保留策略的作用域划分
（两侧都明确确认；ZCode 还补充论证了「全局 LRU 下 A 挤掉的是 B 的旧缓存文件
而非历史行，与今天单账号下旧文件被淘汰同构」，回答了我自己提出的饿死疑虑）；
§2.2 只给 `transfer_tasks` 加列；§2.3 `reset_stale_in_flight` 保持全局的**决策**
（补测试，决策不变）；§2.4 缓存不分账号目录；§3 认领时机的主线（启动后认领、
切账号不认领）；§1 函数表中「不加过滤」那批的安全性论证。

上一轮遗留的两处加固中，`install_default()` 移位经 ZCode 逐行核实正确；
USER_GUIDE 的 v3 说明按 §2.5 修正措辞后保留。

## 4. 两侧共同的盲区

两位审查员都是纯静态阅读，未运行任何测试。ZCode 额外声明它对 `69b9975` 的
diff 是通过 `.git` 内部文件还原的（其探索子代理无 shell 工具），而非
`git show` 的逐行 unified diff——这意味着关于那个 commit 的结论可靠性略低于
其余部分，不过它在 GLM-04 里得出的结论经我核对后确实有效。

两侧都未评估 `transfer_tasks.account_id` 是否需要索引。ZCode 指出在
`history_max_entries=0`（不限）且多账号长期使用的场景下未做量化判断——
这一条我也没有答案，实施时先不加索引，留到出现实际性能问题再说：
过早加索引会在一个行数受保留策略约束的表上付出写入代价。
