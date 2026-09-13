---
schema: trivium.disposition.v1
topic: local-store-account-scope
stage: code
role: claude
kind: response
run_id: 20260913T111246Z
addresses:
  - AGY-01
  - AGY-02
  - GLM-01
  - GLM-02
  - GLM-03
  - GLM-04
  - GLM-05
---

# 代码审查应答：本地历史与缓存按账号分区

- 主题：`local-store-account-scope`
- 阶段：code（应答稿，含逐条裁决）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被审补丁：相对 `47766fb` 的 13 个文件变更（含上一轮遗留的两处加固）
- 被裁决的审查：`2026-09-13-local-store-account-scope-gemini.md`、
  `2026-09-13-local-store-account-scope-glm.md`

---

## 0. 总览

Antigravity 判 `request-changes`（2 条：重要 1 ｜ 次要 1），
ZCode 判 `approve-with-nits`（5 条：次要 2 ｜ 吹毛求疵 3）。
合计 7 条，**全部接受**，无驳回、无暂缓。

本轮焦点问的三件事，两侧都确认成立：两处过滤到位且各有会红的测试钉住、
闸门接线正确、认领边界自洽。ZCode 还手工推演了
`prune_window_is_per_account_when_rows_interleave`，独立得出「去掉窗口子查询
过滤会多删 a_mid」——与我实际撤改动跑出来的结果一致。

真正有价值的是它们找出的**残余路径**。我修的是「列表看不见」和「卡片点不动」，
而两侧合起来指出这条链还有四个出口我没堵：

| 出口 | 谁发现 | 状态 |
| :--- | :--- | :--- |
| `App.tsx` 的传输卡片不随切账号清空 | AGY-01 / GLM-04 | 已修 |
| `cmd_inject_files` —— 注册在 IPC 上的**第四条**读会话文件路径 | GLM-01 | 已修 |
| 接收完成后的自动写剪贴板 | GLM-02 | 已修 |
| 闸门接线本身无测试保护 | GLM-05 | 已补 |

其中 GLM-01 最值得记：`cmd_inject_files` 在 `paths` 缺省时回退到
`get_session_files(&session_id)` 读缓存并写剪贴板，与 `cmd_inject_session`
是同一条事故链，只因当前前端不调用它而处于休眠。**我在计划里数出「三个 IPC
命令」，数漏了一个**，而注册在 `invoke_handler` 上就等于对外开放。

另一件要认的事：**我给「不在保存路径认领」写的注释是自相矛盾的**（AGY-02 /
GLM-03）。我一边写 `claim_is_idempotent_and_does_not_resteal` 证明认领动不了
已有归属的行，一边在注释里说在保存路径认领「会把上一个账号的历史搬过来」。
两者不可能同时为真，而对的是测试。

---

## 1. 裁决表

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | 重要 | 接受 | 与 GLM-04 同一缺陷，但两侧定级不同（AGY 判 major，GLM 判 nit「闸门兜底、纯 UX」）。**我按 AGY 处理**：闸门确实挡住了数据被读走，但卡片本身就带着上一个账号的文件名与预览摘要——那正是本轮要堵的泄露内容，不是纯 UX；且 `transfer_card_retain_secs=0` 时卡片永不消失。AGY 另外点出我漏了 `dismissTimersRef`：卡片清掉而定时器还在，触发时会对一个已不存在的 session 调 setState。 | §2.1 |
| AGY-02 | gemini | 次要 | 接受 | 与 GLM-03 同一件事。它把我的自相矛盾指了出来：`claim_unowned_history` 带 `WHERE account_id IS NULL`，已有归属的行搬不动，所以「在保存路径认领会搬走旧历史」这个理由在 SQL 语义上不成立。后果是实打实的：首次启动账号非法 → 跳过认领 → 用户在面板里改对 → **仍然不认领** → 必须重启才能看见老历史。 | §2.2 |
| GLM-01 | glm | 次要 | 接受 | **我认为它被低估了，按安全缺口处理。** 已核实：`cmd_inject_files` 注册在 `lib.rs:587` 的 `invoke_handler` 上，`paths` 缺省时回退读会话缓存、写剪贴板、打免疫标记，全程无闸门。当前前端无调用点（grep 确认）所以处于休眠，但 IPC 命令一旦注册就是对外开放的接口——「前端不调用」不是访问控制。这直接说明我在计划里数的「三个 IPC 命令」漏了一个。 | §2.3 |
| GLM-02 | glm | 次要 | 接受 | 已核实：接收完成后 TEXT/IMAGE 无条件写剪贴板、FILES 按 `auto_inject`，全程不看账号。接收任务在 spawn 时快照了 settings，而保存设置只重连控制面 actor，在飞的任务会继续跑完——用户若在传输途中切账号，内容会落进**新**账号的剪贴板，而历史行归属旧账号。它建议「至少写进注释」，**我选择实际收口**：代价只是一次按主键的查询，收益是「所有写剪贴板的路径都受账号约束」这句话可以成立，而不是带一个需要记住的例外。 | §2.4 |
| GLM-03 | glm | 吹毛求疵 | 接受 | 与 AGY-02 同一件事的文档面。`USER_GUIDE` 写「推迟到下次填了合法账号再进行」，读起来像在设置里填好就能触发，而实现只在启动认领——用户改对账号后历史仍不可见，会以为丢了。§2.2 补了保存路径的认领之后，文档那句话反而变成了真的，但措辞仍要写准。 | §2.2、§2.5 |
| GLM-04 | glm | 吹毛求疵 | 接受 | 与 AGY-01 同一件事，合并处理。它比 AGY 多给了一条有用的边界：`FAILED` 卡片只有移除按钮、没有装载按钮，所以影响面限于 `COMPLETED` 卡片——这让我确认清空整个 `transfers` 是足够的，不需要区分状态。 | §2.1 |
| GLM-05 | glm | 吹毛求疵 | 接受 | 说中了一个我自己没想到的缺口：闸门判断在 repo 层有充分单测，但「四个命令各自真的调了它」在删掉任意一处后不会让任何测试变红——而「点得动」这条防线完全依赖那几处调用在位。这与上一轮「假绿测试」、这一轮「测试测不出要修的缺陷」是同一族问题：**测了逻辑，没测接线**。 | §2.6 |

---

## 2. 修改内容

### 2.1 切账号清空传输卡片（对应 AGY-01 / GLM-04）

`App.tsx` 监听 `account-changed`，清空 `transfers` 并清掉 `dismissTimersRef`
里的全部定时器。注释写明为什么这里是「清空」而历史面板那边是「重拉」：
传输卡片是推送累积出来的本地状态，没有可重拉的数据源。

新增测试 `收到 account-changed 后清空卡片`，验证过鉴别力——删掉
`setTransfers([])` 即红。

### 2.2 保存设置时也认领（对应 AGY-02 / GLM-03）

`cmd_save_settings` 补一次 `claim_unowned_history`。账号到那里必然合法
（函数开头已 validate 并提前返回），所以不需要再判一次。

`lib.rs` 那段错误的注释改掉了，并把真实理由写进去：两处都做，是因为启动那次
可能因账号非法而跳过，而用户修正账号的地方正是设置面板。原注释声称的
「会把上一个账号的历史搬过来」明确标注为不成立，并指向
`claim_is_idempotent_and_does_not_resteal`——留下这条线索，免得后来人
重新犯一遍同样的推断。

### 2.3 第四条读文件路径加闸门（对应 GLM-01）

`cmd_inject_files` 开头加 `ensure_session_owned`。

**不只在回退分支加，整个命令都加**：即使 `paths` 显式给出，它仍然用
`session_id` 调 `mark_clipboard_injected`——那是在改另一个账号的缓存免疫状态。

没有选择 GLM 给的另一个选项（从 `invoke_handler` 摘除）：它是一个功能完整、
签名合理的命令，摘掉是删功能；而加一行闸门既保留了它，又让它和其余三个一致。

### 2.4 自动装载前复核归属（对应 GLM-02）

`transfer_engine` 的接收完成分支，在写剪贴板之前重读当前账号并核验会话归属，
不一致则跳过装载、只记 warn 并照常发通知。三处（TEXT / IMAGE / FILES）
共用同一个判定。

注释写明了为什么这条路径无法靠 `ensure_session_owned` 兜住：它不是用户点出来
的，而是接收任务跑完自己触发的，那时没有 IPC 调用可拦。

### 2.5 文档修正（对应 GLM-03 / GLM-01）

- `USER_GUIDE`：认领时机改为「在设置面板填入合法账号并保存后会立刻补做，
  老历史当场回来，不需要重启」——§2.2 之后这是准确描述。
- `需求.md`：「三个 IPC 命令」改为「四个」并列出各自名字，补上自动装载
  也会复核归属这一条。

### 2.6 闸门接线的守卫测试（对应 GLM-05）

新增 `every_session_file_command_keeps_the_ownership_gate`：用 `include_str!`
读自身源码，**按函数切片**逐个断言四个命令的函数体内含 `ensure_session_owned`。

按函数切片而不是数总数，是因为总数对得上但集中在一个函数里的话守卫就形同
虚设。`State<'_, AppState>` 需要完整 Tauri 运行时、单测构造不出来，所以这是
退而求其次的做法，注释里写明了它的脆弱性是**故意的**：任何触碰都该让人重新
确认闸门还在，而不是反过来改断言迁就代码。

验证过鉴别力：撤掉 `cmd_inject_files` 的闸门后，该测试红并打印
`pub async fn cmd_inject_files 缺少归属闸门 ensure_session_owned`。

---

## 3. 验证

```
client:  cargo test           88 passed（新增闸门接线守卫）
         npm test             39 passed（新增卡片清空）
         npx tsc --noEmit     干净
server:  go test ./...        7 个包全 ok（本轮不改服务端，确认未误伤）
```

本轮新增/修改的测试全部做过撤改动验证：撤掉被测逻辑会红，包括
`prune_window_is_per_account_when_rows_interleave`（实得 `["a_mid","a_old"]`，
`a_mid` 正是保留窗口内被误删的行）、两条 `list_history` 泄露用例、
三条刷新链路用例、卡片清空、闸门接线。

## 4. 两侧的盲区与一条需要澄清的记录

两侧都是纯静态阅读，未运行任何测试。

**ZCode 声明了一处方法论限制值得记下**：它本次无 shell，对 `69b9975` 的
diff 是通过 `.git` 内部文件还原的，并指出会话开始时工作树有未提交改动，
无法排除被审区域之外的漂移。这个自我披露是诚实的——被审快照
（`changes-20260913T111246Z.diff`）确实是编排器冻结的两点差异，与它读到的
工作树可能不完全同步。本轮结论未受影响（它读过的区域均一致），但下一轮若
再用 `--base` 指向更早的 commit，这一点值得提前说明。

ZCode 另外指出上一轮的 TLS 断言（rustls 双 provider 并存时返回 None 会 panic）
它未独立验证。那一条我在上一轮做过实机验证，结论与错误文本记录在
`multi-account-hardening` 的应答件 §5 里。
