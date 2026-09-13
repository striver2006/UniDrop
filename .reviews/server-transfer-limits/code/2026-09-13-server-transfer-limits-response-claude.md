---
schema: trivium.disposition.v1
topic: server-transfer-limits
stage: code
role: claude
kind: response
run_id: 20260913T065303Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - AGY-04
  - GLM-01
  - GLM-02
  - GLM-03
  - GLM-04
  - GLM-05
---

# 代码审查应答：server-transfer-limits

- 主题：`server-transfer-limits`
- 阶段：code（应答稿，含逐条裁决与修改方案）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`2026-09-13-server-transfer-limits-gemini.md`、
  `2026-09-13-server-transfer-limits-glm.md`

---

## 0. 总览

两侧均判 `approve-with-nits`，共 9 条（次要 7 ｜ 吹毛求疵 2），
**零 blocker、零重要**。**9 条全部接受**，无驳回、无暂缓。

### 0.1 两侧高度重合，实为 4 组 + 1 条独占

| 组 | AGY | GLM | 缺陷 |
|---|---|---|---|
| A | AGY-01 | GLM-02 | `pending_inbound` 未清理 |
| B | AGY-02 | GLM-03 | `rejectToSelf` 注释与实现相反 + 未用 `ctx` |
| C | AGY-03 | GLM-01 | 并发计数与授权之间的 TOCTOU |
| D | AGY-04 | GLM-04 | 并发拒绝路径与计数函数零测试覆盖 |
| — | — | GLM-05 | 注释把守护测试指向了错误的文件 |

四组各自独立发现同一缺陷，这种重合本身是强信号——四条都不是风格分歧。

送审时点名的六个焦点（env 三态解析、`None` 退化与兜底常量未合并、预检时序与
目录跳过口径、两条拒绝路径的回送方向、`pending_outbound` 清理、并发统计）
两侧**逐条核实通过**；GLM 还额外核实了 `config` 死字段删除后全仓无残留引用。
缺陷全部落在这六条之外——说明焦点指对了地方，但也说明我对焦点之外的地方
看得不够。

### 0.2 我独立复核的三条事实

接受之前逐条读源码复核，不采信转述：

| 意见 | 复核结论 |
|---|---|
| A 组 | 属实。`pending_inbound` 插入在 `lib.rs:185`，**唯一**删除点在 `lib.rs:227`（token 回显路径）。我新增的失败分支只删了 `pending_outbound` |
| B 组 | 属实，且两处都反。`session.Send`（`registry/session.go:59-69`）是向 **256 容量** chan 的非阻塞入队、满即丢弃并打 warn；我的注释却说「直写 socket、不经 SendChan、不会被丢」。`ctx` 只出现在签名里 |
| C 组 | 属实。`CountAuthorizedForAccount` 持 RLock 计数后释放，`AuthorizeSession` 另取 Lock，两段之间无共同临界区 |

### 0.3 一处 A 组与 C 组之外的观察：两条修复存在依赖顺序

**C 组的修复会改变 D 组该测什么。** 把「计数 + 判定 + 铸 token」折叠进
`RelayManager` 的单一方法后，`CountAuthorizedForAccount` 要么消失、要么降为
内部辅助——此时再按 D 组的建议给它补导出级单测就落了空。

因此实施顺序固定为：**先 C 后 D**，D 的测试对象是折叠后的新方法
（含「同账号并发调用不越界」这条只有折叠后才写得出的断言）。
两侧都没提这层依赖，但按任一侧的建议单独施工都会返工。

---

## 1. 裁决表

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
|---|---|---|---|---|---|
| AGY-01 | gemini | 次要 | 接受 | 已复核：接收端 `pending_inbound` 在并发拒绝路径上必然残留，且残留的是含完整 `items` 的 offer | `lib.rs` 的 TRANSFER_FAILURE 分支无条件双清 |
| AGY-02 | gemini | 次要 | 接受 | 已复核：注释断言的投递保障代码并不提供，且「直写 socket」这个方向本身会与写泵 goroutine 竞态 | 改写注释为如实描述；删除未使用的 `ctx` 形参 |
| AGY-03 | gemini | 次要 | 接受 | 已复核：计数与授权两次独立加锁，跨连接窗口真实存在 | 折叠为 `RelayManager` 单一方法，在同一把写锁内完成 |
| AGY-04 | gemini | 次要 | 接受 | 并发拒绝路径与计数逻辑零覆盖；按本项目「这条测试要怎样才会失败」的标准不合格 | 补 WS 级并发拒绝测试 + 折叠后方法的表驱动单测 |
| GLM-01 | glm | 次要 | 接受 | 与 AGY-03 同一条。GLM 额外指出这正是该配置项要防的场景（失控客户端多设备并发），窗口不是纯理论 | 同 AGY-03 |
| GLM-02 | glm | 次要 | 接受 | 与 AGY-01 同一条。GLM 额外指出 `HashMap::remove` 对不存在的 key 是 no-op，故可无条件双清 | 同 AGY-01 |
| GLM-03 | glm | 次要 | 接受 | 与 AGY-02 同一条。GLM 额外指出照注释去「修」会修出并发写竞态——比注释本身错更危险 | 同 AGY-02 |
| GLM-04 | glm | 次要 | 接受 | 与 AGY-04 同一条。GLM 点明方向接反时 CI 依旧全绿，而那恰是我自己注释标为最易写反的一行 | 同 AGY-04 |
| GLM-05 | glm | 吹毛求疵 | 接受 | 属实。注释把守护测试指向 `settings_cmd`，实际在 `clipboard_cmd` 自己的 tests 模块。它是「不得合并」整段论证的落点，指错会让人以为守护不存在 | 注释改指本文件的 `fallback_constants_are_pinned_independently` |

**统计**：接受 9 ／ 驳回 0 ／ 暂缓 0。

---

## 2. 需要展开说明的裁决

### 2.1 A 组（AGY-01 / GLM-02）：我修了一半的对称性

本轮我给 TRANSFER_FAILURE 分支补了 `pending_outbound` 清理，理由写得很清楚：
「本轮让服务端主动拒绝成为常规路径，这条泄漏随之从罕见变高频」。

**这个理由对 `pending_inbound` 一字不差地成立，而我没做。**

接收端在 `lib.rs:185` 收到 offer 时就插入 `pending_inbound`，唯一的删除点在
`lib.rs:227` 的 token 回显路径。而我新增的并发拒绝**恰恰保证 token 回显不会
发生**——服务端拒掉 ANSWER 后 `continue`，既不授权也不转发。于是每一次被
并发拒绝的接收，都在 map 里永久留下一份含完整 `items` 的 offer。

我只想到了「发送端被拒」这半边，没想到并发拒绝是**双向**的——
而双向回送正是我自己在同一轮里刻意设计的。

修法按 GLM 的建议：无条件双清。`HashMap::remove` 对不存在的 key 是 no-op，
所以不需要先判断本机是发送方还是接收方——判断本身才是出错的来源。

### 2.2 B 组（AGY-02 / GLM-03）：注释断言了一个代码不提供的保障

我写的注释是：

> The write goes directly to the socket rather than through session.SendChan
> because the refusal happens inside the read loop and should not be dropped
> if the send queue is momentarily full.

**两处都与事实相反**：代码调用的是 `session.Send`，它向 256 容量的 `SendChan`
非阻塞入队、满即丢弃（`registry/session.go:59-69`）；紧邻的下两行我自己还在
记录 queue full 的 warn——注释与它正下方的代码自相矛盾。

成因我能复盘：那段注释是我在**决定怎么写之前**先写下的意图，
落笔实现时改用了 `session.Send`（这是对的——写泵 goroutine 独占该连接，
`coder/websocket` 不允许并发 Write），却没有回头改注释。

GLM 指出的后果比「注释写错」更重：这条注释断言了一个投递保障，
后来者要么误信它，要么**照着它去「修」出一个并发写竞态**。
一条把人引向错误方向的注释，比没有注释坏。

修法：注释改为如实描述——经 SendChan 排队，队列满时丢弃并留 warn；
并说明为什么不能绕开队列直写。同时删掉未使用的 `ctx` 形参
（Go 不对未用参数报警，所以编译与 vet 都没拦住）。

### 2.3 C 组（AGY-03 / GLM-01）：容量闸门的 TOCTOU

`control_ws.go:303` 计数（RLock，随即释放）→ `:304` 判定 →
`:313` `AuthorizeSession`（另取 Lock）。三段之间没有共同临界区。

同账号的两台接收设备各自有独立的读循环 goroutine（每连接一个），
可以同时读到 `inFlight=7`、同时通过判定、同时授权，最终在途 9 个。

单连接内部串行，所以单设备不自竞争——但跨连接窗口必然存在。
GLM 的补充是决定性的：**这正是该配置项要防的场景**（一个失控客户端多设备
并发开传输），所以这个窗口不是纯理论。全局 200 管道上限只是遥远的第二道闸。

修法取两侧一致的建议：把「按账号计数 + 判定 + 铸 token」折叠为
`RelayManager` 的单一方法，在同一把 `m.mu.Lock` 内完成。
按 GLM 的提醒，`maxConcurrent` 作为**参数**传入而非让 `relay` 去依赖 `limits`，
避免为一个数字引入包依赖。

### 2.4 D 组（AGY-04 / GLM-04）：最易写反的一行没有任何自动守护

`limits_ws_test.go` 的五个用例全在 OFFER 路径。而 ANSWER 拒绝的回送方向与
OFFER **恰好相反**（OFFER 的发送方是当前 session；ANSWER 的当前 session 是
接收方、`ToDevice` 才是发送方）——这一点重要到我专门写了一段注释警告，
却没有给它配一条测试。

GLM 用本项目的标准给出了判据：「现在方向接反或排除逻辑损坏，CI 依旧全绿，
用户复现的恰是本轮要消灭的静默挂起。」

这一条我接受得毫无保留。本轮我对 OFFER 路径做了四次回退注入验证，
唯独没对 ANSWER 路径做——因为它根本没有测试可注入。
**「我警告过它容易错」不能替代「我证明了它错了会被发现」。**

修法（须在 C 组之后，理由见 §0.3）：

1. WS 级测试：`MaxConcurrentTransfers=1`，第二个会话的 ANSWER 被拒时断言
   **接收方与发送方各收到带 `session_id` 的 TRANSFER_FAILURE**、
   原 ANSWER 未被转发、且未铸出 token；
2. 折叠后方法的表驱动单测：异账号不计、排除自身 `session_id`、已过期不计；
3. 两条都做回退注入验证（把双向回送改成单向、把方向对调），确认能精确打红。

### 2.5 GLM-05：指错文件的指针，比一般 nit 严重

`clipboard_cmd.rs` 里兜底常量上方的注释结尾写着「`settings_cmd` 的测试把这两个
数分别钉死」，但真正钉死它们的 `fallback_constants_are_pinned_independently`
在 `clipboard_cmd.rs` **自己的** tests 模块里；`settings_cmd.rs` 里 grep 不到
任何 `FALLBACK` 或对应数值的断言。

成因同 B 组：注释写在决定测试放哪之前，之后没回头改。

判为「比一般 nit 严重」的理由是 GLM 给的：这段注释是「兜底常量不得与服务端
默认值合并」整段论证的落点，而那正是本轮最隐蔽的一个陷阱（文本两个 4 MB
相等纯属巧合）。指针指错，后来者去错误的地方找不到守护测试，
很可能直接得出「没有守护」的结论，然后动手合并。

---

## 3. 修改方案汇总

| # | 文件 | 改动 | 对应意见 |
|---|---|---|---|
| 1 | `server/internal/relay/relay_manager.go` | 新增 `AuthorizeSessionIfUnderLimit`，在单一写锁内完成计数、判定、铸 token；`maxConcurrent` 作为参数传入 | AGY-03 / GLM-01 |
| 2 | `server/internal/controller/control_ws.go` | ANSWER 分支改调新方法，消费其结果；`rejectToSelf` 注释改为如实描述并删除未用 `ctx` | AGY-02/03 · GLM-01/03 |
| 3 | `server/internal/relay/relay_test.go` | 新方法的表驱动单测（异账号 / 排除自身 / 已过期 / 同账号并发不越界） | AGY-04 / GLM-04 |
| 4 | `server/internal/controller/limits_ws_test.go` | ANSWER 并发拒绝的 WS 级测试，断言双向回送、不转发、不铸 token | AGY-04 / GLM-04 |
| 5 | `client/src-tauri/src/lib.rs` | TRANSFER_FAILURE 分支无条件双清 `pending_outbound` 与 `pending_inbound` | AGY-01 / GLM-02 |
| 6 | `client/src-tauri/src/commands/clipboard_cmd.rs` | 兜底常量注释改指本文件的守护测试 | GLM-05 |

全部为收敛性改动，不改变本轮已确立的语义（限额分层、三态解析、
兜底常量取值、两条拒绝路径的方向）。

---

## 4. 两侧盲区的交集

两侧声明的盲区里有两处重合，即本轮结束后仍然无人验证：

1. **两侧都没运行过任何测试。** GLM 明说「未运行 go test / cargo test / vitest」，
   AGY 的工具轨迹 `run_command: 0`。两套新测试的通过与否，
   只有我这边的执行记录（Go 7 包全过、Rust 70、前端 27）；
2. **端到端与真实 UI 均未触及。** 计划 §7.4 的六项人工验收仍是唯一手段，
   且本轮新增了第 7 项（三项行为变更逐项确认）。

GLM 另外单独记了一条值得留档的观察（未列为缺陷，我同意其定性）：
`control_ws_test.go` 与 `e2e_test.go` 用 `limits.FromEnv()` 构造，
会继承运行环境里的 `UNIDROP_*` 变量，极端环境下可能导致测试漂移。
本轮不改——它是测试环境卫生问题而非产品缺陷，但记在此处，
日后若出现「只有某人机器上红」的现象，这里是第一个该看的地方。

---

## 5. 结论

9 条全部接受。四组重合缺陷中，A 组（对称性只修一半）与 D 组（最易错的一行
没有守护）是我这一轮实打实的疏漏；B 组与 GLM-05 同源——注释写在决定之前，
实现改了却没回头改注释，留下两处断言与事实相反的文字。

修改方案已列出，等待人工审批后实施。
