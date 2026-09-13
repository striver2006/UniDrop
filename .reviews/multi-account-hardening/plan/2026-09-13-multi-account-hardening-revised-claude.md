---
schema: trivium.disposition.v1
topic: multi-account-hardening
stage: plan
role: claude
kind: revised
run_id: 20260913T082750Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - AGY-04
  - AGY-05
  - AGY-06
  - GLM-01
  - GLM-02
  - GLM-03
  - GLM-04
  - GLM-05
---

# 修订计划：多账号硬化 + TLS 证书校验修复

- 主题：`multi-account-hardening`
- 阶段：plan（修订稿，含逐条裁决）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`2026-09-13-multi-account-hardening-gemini.md`、
  `2026-09-13-multi-account-hardening-glm.md`

---

## 0. 总览

Antigravity 判 `request-changes`（6 条：重要 3 ｜ 次要 2 ｜ 吹毛求疵 1），
ZCode 判 `approve-with-nits`（5 条：次要 4 ｜ 吹毛求疵 1）。合计 11 条，**全部接受**，
无驳回、无暂缓。

两侧独立指向了同一个根因，这是本轮最有价值的产出：**原计划把 auth 条目的 5 分钟 TTL
与 pipe 的 60 秒空闲存活当成了两条互不相干的生命周期**，于是

- A3 改动 B 的「无 auth 条目 → 拒绝」会在诚实的长传输完成时误拒（AGY-01 / GLM-03）；
- A4 的 per-account 上限会因 `countAuthorizedForAccountLocked` 只数 auth 条目而在
  控制面放行、到数据面才硬失败（AGY-02 / GLM-02）。

我原计划把这个 TTL 错配**只当成 A4 要堵的一个容量窗口**（「pipe 活过 auth 条目」），
没意识到同一个错配会从 A3 改动 B 那一侧反噬成误拒。两侧各自从一端走到了同一处，
结论一致且我核实成立。修订的主干就是让这两条生命周期联动起来（见 §2.1）。

另有两条是我自己的事实性错误，两侧都抓到了：`NewRelayManager` 的调用点计数，
以及 session_id 铸造点引用到了 `#[cfg(test)]` 内的行号（AGY-06 / GLM-05）。

---

## 1. 裁决表

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | 重要 | 接受 | 已核实 `relay_manager.go:307-312` 的第二个循环无条件删除过期 auth 条目，不看 pipe 是否活跃；而 pipe 的回收条件是 60s 空闲。超过 5min 的诚实传输在 COMPLETE 时必然命中「无 auth 条目」。原计划的「pipe 只能由 `ValidateAndGetOrCreatePipe` 创建，而它必须有 auth 条目」只证明了**创建时**存在，不等于**拆除时**仍在——这是原论证的逻辑跳跃。 | §2.1 重写 A3 改动 B 的拒绝语义为三分支；A4 的 `SweepIdlePipes` 增加「pipe 活跃则不清其 auth 条目」 |
| AGY-02 | gemini | 重要 | 接受 | 同一 TTL 错配的另一面，已核实 `countAuthorizedForAccountLocked`（`relay_manager.go:143-154`）只统计未过期 auth 条目。长传输的条目被清后账号在途计数归零，ANSWER 阶段误判有名额并铸出 token，到 `/ws/data` 才被 per-account 上限以 `StatusPolicyViolation` 断开。原计划把 per-account 上限放在数据面，等于把一个**容量拒绝**放在了唯一没有用户可读通道的那一层。 | §2.1 让 auth 条目在 pipe 活跃期间不过期，使账号计数自然覆盖长传输，拒绝回到 ANSWER 阶段的既有双向 `TRANSFER_FAILURE` 通道 |
| AGY-03 | gemini | 重要 | 接受 | 已核实 `ConnectionActor` 结构体（`connection_actor.rs:76-80`）只有 `config` / `reconnect_notify` / `outgoing_rx`，不持有 `AppHandle`；`auth-failed` 是 lib.rs 收到 `incoming_tx` 的 AUTH_RESPONSE 后 emit 的（`lib.rs:334`）。TLS 握手失败发生在 WebSocket 建立之前，拿不到任何 envelope，原计划「在连接失败分支 emit 提示」没有可用通道。这是 B3 的实现前提缺失，不是文案问题。 | §2.2 补上错误上报通道设计 |
| AGY-04 | gemini | 次要 | 接受 | 「无需新增依赖」这句只在「不手动构造 `ClientConfig`」的前提下成立，而原计划恰恰写的是手动构造。`rustls-tls-webpki-roots` 是 tokio-tungstenite 的 feature，不会把 `webpki_roots` 这个 crate 名注入本包的 extern prelude。审查员给的第二方案（默认档传 `None`，由 tokio-tungstenite 自己加载根证书）更省事且不引依赖，采纳它。 | §2.2 B1 改为默认档传 `connector: None` |
| AGY-05 | gemini | 次要 | 接受 | 属实。`docs/需求.md` 第 4/5/8/9 条都有「已实现」注记与取舍说明，是这个项目记录需求状态的既定惯例，而我的文档同步清单里没有这个文件。本轮正是第 6、7 两条的直接落地，不回写等于让需求文档继续显示未实现。 | §2.3 文档同步清单增补 `docs/需求.md` 第 6、7 条 |
| AGY-06 | gemini | 吹毛求疵 | 接受 | 我的计数错了。实测 `NewRelayManager()` 为 1 处生产（`main.go:46`）+ **10 处**测试（`control_ws_test.go:26,59`、`limits_ws_test.go:43`、`e2e_test.go:37`、`relay_test.go:92,115,139,156,173,193`），原计划写的「5 个调用点，4 个在测试」两个数都不对。AGY 的 10 处与实测一致。 | §2.3 破坏性清单 #4 改为「1 处生产 + 10 处测试」 |
| GLM-01 | glm | 次要 | 接受 | 已核实 `connection_actor.rs:159` 在 AUTH_CHALLENGE 处理内 `Uuid::new_v4()`，每次握手（含 30s 退避后的重连）都新铸 nonce，随发客户端不存在复用同一 nonce 重试的路径。我那条「用户永远改不对」的故障叙事对自家客户端不成立。顺序决策本身两侧都确认正确，但按原稿写进代码注释会固化一条假理由——注释是要长期留在代码里的，这比计划文本错一句严重。 | §2.4 改写 A1 要写入注释的理由 |
| GLM-02 | glm | 次要 | 接受 | 与 AGY-02 同一根因，但它多给了一条我没考虑到的呈现细节：数据面拒绝经 `data_ws.go:70` 的 close reason 下发，客户端 `transfer_engine.rs` 只渲染成笼统的「数据通道连接失败」。它给的二选一里，「在授权阶段统一计数」与 AGY-02 的建议一致，合并处理。 | 并入 §2.1；拒绝统一回到 ANSWER 阶段 |
| GLM-03 | glm | 次要 | 接受 | 与 AGY-01 同一根因。**但它的建议我不照搬**：「过期视同不存在，直接放行拆除」会重新打开 A3 改动 B 要堵的洞——攻击者只需针对一个已过期的会话就能零凭据拆除。它真正有价值的是另外两点：一是指出「空闲回收是拒绝语义的安全网」这个前提我没写出来，二是警告把诚实拆除按可疑行为打 Warn 会淹没真正的越权告警。这两点都采纳。 | §2.1 三分支设计（其中「无条目且无 pipe」走 debug 而非 warn） |
| GLM-04 | glm | 次要 | 接受 | 属实且是我漏掉的一类用户。webpki 根不含企业私有 CA，这类部署既过不了默认校验、也不在我列举的「自签 / 直连 IP」里，按原计划的文案会被引导去勾选「允许不安全连接」——为了让一个**合法**证书通过而把校验整体关掉。另外它指出数据面证书错误不做专门提示这个取舍我没写明，属实。 | §2.2 B2/B3 文案改为三类场景；数据面取舍显式记录 |
| GLM-05 | glm | 吹毛求疵 | 接受 | 两个子点都成立：`transfer_engine.rs:956,974` 确认在 `mod tests`（932 行起）内，生产铸造点是 `clipboard_cmd.rs:267,299`，我引用错了位置。**但它给的替代计数「9 处测试」本身也不对**——它自己列出的位置清单正好是 10 个，实测也是 10 个，以 AGY-06 的数字为准。它对「计划大量依赖精确计数建立『改动面小』的论证，个别数字失准会削弱其余正确数字的可信度」这个判断我认同。 | §2.3 修正 A3 的 session_id 引用位置；计数按实测 10 处 |

---

## 2. 修订内容

### 2.1 A3 改动 B + A4：两条生命周期联动（对应 AGY-01 / AGY-02 / GLM-02 / GLM-03）

这是本轮改动最大的一处，替换原计划 A3 改动 B 与 A4 的相应段落。

**根因**：`auth` 条目 TTL 5 分钟（`control_ws.go:312`），`pipe` 的存活条件是 60 秒内有活动，
而 `SweepIdlePipes`（`relay_manager.go:298-320`）的第二个循环无条件清除过期 auth 条目，
不看对应 pipe 是否还活着。两条生命周期互不相干，于是长传输会进入
「pipe 活着但 auth 条目已消失」的状态。

**改动一：让 auth 条目在 pipe 活跃期间不过期。**
`SweepIdlePipes` 第二个循环的删除条件加一个前置判断：`m.pipes[id]` 存在则跳过。
注释要写明这不是「延长授权有效期」而是「授权的生命周期至少覆盖它授权出来的那条管道」——
把 `ExpiresAt` 理解成「这张票还能用来开新管道多久」，而不是「这条管道还能活多久」。

这一处同时解决 AGY-02 / GLM-02：`countAuthorizedForAccountLocked` 只数 auth 条目，
条目不再被提前清掉，账号在途计数就自然把长传输算进去，per-account 容量拒绝
回到 ANSWER 阶段复用既有的双向 `TRANSFER_FAILURE`，不再落到数据面那条
只能渲染成「数据通道连接失败」的路径上。

**改动二：`RemovePipeForSession` 的拒绝语义改为三分支**，取代原计划笼统的「无 auth 条目 → 拒绝」：

| 情形 | 处置 | 日志 | 理由 |
| :--- | :--- | :--- | :--- |
| 有 auth 条目，账号 / 设备对不符 | 拒绝 | `slog.Warn` cross-account teardown blocked | 真正的越权，这是本改动要堵的形态 |
| 有 auth 条目，归属相符 | 拆除 | — | 正常路径 |
| 无 auth 条目**且**无 pipe | no-op 返回 | `slog.Debug` | 迟到的重复信令，pipe 已被空闲回收，拆除本就无事可做。**不打 Warn**——GLM-03 正确指出，把它告警化会让诚实的长传输持续产生噪音，淹没上面那条真正的越权告警 |
| 无 auth 条目**但** pipe 存在 | 用 `pipe.AccountID` + `pipe.FromDevice/ToDevice` 校验，通过才拆除 | 不符才 Warn | 改动一之后这一支理论上不可达，但保留为兜底：`RelayPipe` 在 A4 里本来就要加 `AccountID` 字段，校验是免费的，且它让归属校验不依赖「改动一没有回归」这个前提 |

**为什么不采纳 GLM-03 的「过期视同不存在，直接放行」**：那等于把 A3 改动 B 想堵的洞
按时间开了一扇窗——攻击者针对任何一个已过期会话就能零凭据拆除。要写进注释，
免得后来人觉得三分支繁琐而「简化」回去。

**A4 的相应调整**：per-account 上限的**主**拦截点移到 `AuthorizeSessionIfUnderLimit`
（即复用既有的 `MaxConcurrentTransfers` 计数，改动一之后它已经准确）；
`ValidateAndGetOrCreatePipe` 里保留一道同值的兜底检查，注释写明它现在是纵深防御
而非主闸门，以及为什么仍要保留（数据面可以被直接拨号，不能假定一定走过控制面授权）。
原计划「per-account 检查必须排在已有 pipe 命中返回之后」的顺序要求不变，仍配测试。

### 2.2 范围 B 的三处修订（对应 AGY-03 / AGY-04 / GLM-04）

**B1 默认档不手动构造 `ClientConfig`**（AGY-04）：`create_tls_connector(allow_insecure: bool)`
在 `false` 时返回 `None`，由 `connect_async_tls_with_config` 走 tokio-tungstenite 自带的
webpki 根证书路径；`true` 时才返回 `Connector::Rustls` 包着 `InsecureServerCertVerifier`。
这样确实无需新增 Cargo 依赖，原计划那句「无需新增依赖」也就名副其实。
三个调用点的传参方式不变。

**B3 补上错误上报通道**（AGY-03）：`ConnectionActor` 不持有 `AppHandle`，而 TLS 握手失败
早于 WebSocket 建立、拿不到 envelope。落点：给 `ConnectionActor::new` 注入 `AppHandle`，
在 `run` 的连接失败分支识别 rustls 证书错误后直接 emit 一个**新事件**
（如 `tls-cert-failed`），前端复用 `App.tsx` 既有的顶部红框横幅渲染。

不采用「合成一条 AUTH_RESPONSE 送进 `incoming_tx`」的做法（AGY-03 建议里的第二方案）：
那是把传输层错误伪装成鉴权失败，会让 `auth-failed` 这个事件名从此名不副实，
也会污染 lib.rs 里那段 AUTH_RESPONSE 处理逻辑（它还负责存服务端限额）。

**B2/B3 文案覆盖三类场景**（GLM-04）：需要勾选该开关的部署是**自签证书、IP 直连、
非公共 CA 签发**三类，不是原计划写的两类。UI 文案、B3 的错误提示、`USER_GUIDE.md`
都按三类写。另外显式记录一条有意取舍：**数据面证书错误不做专门提示**——控制面连不上时
数据面不会启动，这条路径实际不可达，不值得为它再铺一条通知链路。
「加载系统根证书 / 自定义 CA」记为后续轮的可选项，本轮不做。

### 2.3 事实性修正（对应 AGY-05 / AGY-06 / GLM-05）

- **破坏性清单 #4** 改为：`relay.NewRelayManager` 签名变更影响 **1 处生产
  （`main.go:46`）+ 10 处测试**（`control_ws_test.go:26,59`、`limits_ws_test.go:43`、
  `e2e_test.go:37`、`relay_test.go:92,115,139,156,173,193`）。原写的「5 个调用点，
  4 个在测试」两个数都错。清单 #3 的「2 处生产调用点」与 #5 的「1 处生产调用点」
  经两侧独立核实无误，保持原样。
- **A3 开头的 session_id 引用**改为 `clipboard_cmd.rs:267,299`（生产铸造点）。
  原引用的 `transfer_engine.rs:956,974` 在 `#[cfg(test)]` 模块内（`mod tests` 起于 932 行），
  用测试代码论证生产行为是引用错误，结论（每次传输现铸 UUIDv4）不受影响。
- **文档同步清单增补 `docs/需求.md`**，标记为「必须」：第 6 条记多账号便利隔离硬化完成、
  保持单 PSK 模型不变；第 7 条记传输层 TLS 证书默认校验完成、E2EE 仍属 Phase 3。
  格式对齐该文件第 4/5/8/9 条既有的「已实现（日期）+ 取舍与边界」写法。

### 2.4 A1 写入注释的理由改写（对应 GLM-01）

顺序决策（格式校验排在 `VerifyWithSalt` 之前）**不变**，两侧都确认它正确。
改的是将要写进代码注释的**理由**，删掉对自家客户端不成立的那条：

- 删去：「格式非法但签名正确的请求若排在后面，重试会因同一 nonce 被判重放，
  错误漂成 replay detected，用户永远改不对」——`connection_actor.rs:159` 每次握手
  新铸 nonce，随发客户端走不到这个场景。
- 改为两条成立的理由：(a) **协议契约健壮性**——`DESIGN.md` 的 canonical string 是面向
  第三方实现的契约，第三方客户端可以在 60s 窗口内复用 nonce 重试，对它们而言
  「格式错不该烧掉重放防线」是真实收益；(b) **未鉴权输入先过零分配的廉价检查**，
  不为一个格式就非法的请求付 HMAC 计算。

`TestRejectedIdentityDoesNotBurnNonce` **保留**——它钉的是协议层顺序，
作为回归钉子仍然有价值，只是测试注释里要说明它守护的是第三方实现场景。

---

## 3. 未改动的部分

以下原计划内容经两侧核实成立，保持不变：A1 的顺序决策本身与格式文法；
A2 的复合键设计与「Get 2 处生产调用、Unregister 零生产调用」计数；
A3 保留扁平键 + 显式归属校验的取舍，以及「冲突检查必须排在
`countAuthorizedForAccountLocked` 之前」的顺序论证；A3 改动 C 的双向拒绝
（ZCode 专门推演过未发现新挂起反例）；A5 拒绝 per-account label 的基数论证与
三个聚合 gauge；A6 删除 `paired_devices`；「明确不做」对历史/缓存跨账号混用的
描述与延后决策。

两侧都确认 **A3 改动 B 是本轮最重的一条**（任何已鉴权客户端自报 session_id
即可零凭据拆除任意管道），与我的定级一致。

## 4. 两侧共同的盲区

两位审查员都是纯只读，未运行 `go test` / `cargo test` / `npm test`，
因此本计划所有改动的编译与测试可行性均未经验证。B3「识别 rustls 证书错误」
依赖 tungstenite 错误类型匹配，ZCode 明确指出其脆弱性未经深入评估——
实施时若发现错误类型不稳定，需要回头调整 §2.2 的方案而非硬凑。
