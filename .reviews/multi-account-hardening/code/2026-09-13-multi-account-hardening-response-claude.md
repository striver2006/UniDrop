---
schema: trivium.disposition.v1
topic: multi-account-hardening
stage: code
role: claude
kind: response
run_id: 20260913T093116Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - AGY-04
  - GLM-01
  - GLM-02
  - GLM-03
---

# 代码审查应答：多账号硬化 + TLS 证书校验修复

- 主题：`multi-account-hardening`
- 阶段：code（应答稿，含逐条裁决）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`2026-09-13-multi-account-hardening-gemini.md`、
  `2026-09-13-multi-account-hardening-glm.md`

---

## 0. 总览

Antigravity 判 `request-changes`（4 条：阻断 1 ｜ 重要 2 ｜ 次要 1），
ZCode 判 `request-changes`（3 条：重要 1 ｜ 次要 1 ｜ 吹毛求疵 1）。
合计 7 条，**全部接受**，无驳回、无暂缓。

**两侧在同一块代码上给出了相反的结论，而错的一方是 ZCode。** 我给的三个焦点里，
ZCode 逐条论证「焦点 1/2/3 全部成立、核不出问题」，Antigravity 却在同一处报了
本轮唯一的 blocker（AGY-01）。我写测试实际复现了 AGY-01 描述的攻击链，
**它成立**。

值得记下的是我的焦点本身有引导性错误：我问的是「联动会不会让 auth 条目
**永久滞留**」，ZCode 就严格回答了这个问题（答案是不会，泄漏 pipe 会被空闲回收）。
但真正的缺陷不在滞留，而在**留下来的条目是过期的、因而是无效的**——我只让条目
活着，却没让任何判定承认它还有约束力。Antigravity 跳出了我的提问框架，
ZCode 被我框住了。这是提问方式的失败，不是 ZCode 的失职，但结论上要以
Antigravity 为准。

---

## 1. 裁决表

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | 阻断 | 接受 | **已写测试实际复现，攻击链完整成立。** 我的生命周期联动只做了一半：`SweepIdlePipes` 保留了活跃管道的 auth 条目，但冲突检查（`now.Before(existing.ExpiresAt)`）与在途计数（同款判断）仍只看 `ExpiresAt`。于是超过 5 分钟的长传输留下一个「在场但过期」的条目——既不计入配额，更要命的是**看起来可以自由覆盖**：攻击者用受害者的 session_id 重新授权即成为该会话的持有者，再发一条 CANCEL 就拆掉了对方正在传输的管道。计划阶段我为 `RemovePipeForSession` 设计的 case 4 兜底在这里救不了场，因为 auth 条目此刻确实存在，只是已经易主。 | 引入 `authLiveLocked`（未过期 **或** 其管道仍活着），统一用于冲突检查、在途计数、sweep、`CheckAuthorization`、`ValidateAndGetOrCreatePipe` 五处；新增 `TestExpiredAuthOfLivePipeStaysBinding` 五项断言钉死 |
| AGY-02 | gemini | 重要 | 接受 | 与 GLM-01 同一缺陷，两侧独立发现。属实且是我的疏漏：我在 `connection_actor` 里 emit 了 `tls-cert-failed`，却没在 `App.tsx` 里接。后果不是少一个提示那么轻——本轮把证书校验改成默认开启，自签/IP 直连/内部 CA 三类部署升级后会直接断连，而用户界面上只有静默重连。`USER_GUIDE.md` 已经白纸黑字许诺了「界面提示『无法验证服务器证书』」，等于文档承诺了一个不存在的东西。 | 见 §2.2 |
| AGY-03 | gemini | 重要 | 接受 | 属实，但需要把它的两半分开说。**前半（客户端缺 else 分支）成立且已修**：`lib.rs` 只处理 `answer.accepted` 为真的一支，接收方明确拒收时发送方的 `pending_outbound` 永不释放、卡片停在等待状态，是个不需要任何异常就能稳定复现的静默挂起。它是既有缺陷而非本轮引入，但本轮 A3 改动 C 的整个主张就是「不再有静默挂起」，只堵服务端不肯授权那一半、留着对端不愿接收这一半，那个主张就只对一半。**后半（服务端在反序列化失败时 `rejectToSelf`）不可行**：反序列化失败意味着拿不到 `session_id`，而 `buildFailure` 明确要求它非空才能让回执落到卡片上——硬发一条只会变成到不了任何地方的推送。 | 前半见 §2.3；后半以 AGY-04 的 session_id 校验覆盖：非法就丢弃并记日志，不再进入授权 |
| AGY-04 | gemini | 次要 | 接受 | 与 GLM-02 同一缺陷，两侧独立发现。`grep` 确认 `ValidateSessionID` 在整个 server 里零生产调用方——我在计划 §A3 写了「顺带对 sessionID 调 `auth.ValidateSessionID`」，实现时漏了。GLM-02 补充的那一层更值得记：函数注释声称它保护 map 键卫生，而这条防线根本没接上，后来者会以为该路径已被覆盖。AGY-04 另外指出的「空 session_id 会让双向拒绝因 `buildFailure` 失败而静默丢弃」也属实。 | 见 §2.4 |
| GLM-01 | glm | 重要 | 接受 | 与 AGY-02 同一件事，合并处理。它多给了一条我会踩的坑：该事件**每次退避重试（≤30s）都会重发**，所以提示必须是幂等的常驻横幅，不能是 toast——否则会堆成一串。这个提醒直接改变了我的实现选择。 | 见 §2.2，实现时对证书失败刻意不发 toast |
| GLM-02 | glm | 次要 | 接受 | 与 AGY-04 同一件事。它给的二选一（接线 / 删掉并修正注释）里我选接线：计划阶段本就是这么定的，而且这个校验确实提供真实保护（session_id 会成为 `authSessions` 与 `pipes` 的 map 键，而该字段只受控制面 512 KiB 读上限约束）。它那句「不要保留一个声称有防线而未接线的函数」是对的。 | 见 §2.4 |
| GLM-03 | glm | 吹毛求疵 | 接受 | 属实。`RemovePipeForSession` 返回 false 后再调 `OwnerOfSession` 是两次独立加锁，中间可插入一次新授权，于是一条良性的迟到信令会被误报成 warn 级「cross-account pipe teardown blocked」——恰好与该分支压制噪音的目标相悖。GLM 自己判「可接受现状」，我仍然修：它给的方向（持锁期间一并返回原因）顺带消掉一次多余加锁，并让 `OwnerOfSession` 这个只服务单一调用点的导出函数可以删掉。 | 见 §2.5 |

---

## 2. 修改内容

### 2.1 AGY-01：授权条目的有效性判定（本轮唯一 blocker）

**复现**（修复前实际运行，日志原文）：

```
!! 覆盖成功 —— 攻击者已成为该 session 的授权持有者
!! 攻击者成功拆除了受害者正在传输的管道（AGY-01 成立）
```

根因是「条目在场」与「条目有效」被我当成了一回事。修复引入统一判定：

```go
// 未过期，或者它授权出来的那条管道仍然活着
func (m *RelayManager) authLiveLocked(sessionID string, auth *SessionAuth, now time.Time) bool {
	if now.Before(auth.ExpiresAt) {
		return true
	}
	_, pipeAlive := m.pipes[sessionID]
	return pipeAlive
}
```

五处统一改用它：冲突检查、`countAuthorizedForAccountLocked`、`SweepIdlePipes`、
`CheckAuthorization`、`ValidateAndGetOrCreatePipe`。

后两处是顺带修掉的同族问题，注释里写明了：长传输途中的数据面**断线重连**
原本也会因 auth 过期而被拒——即 AGY-01 之外另一种「传到一半莫名中断」。
token 校验不变，所以对没有 token 的人没有放宽任何东西。

回归测试 `TestExpiredAuthOfLivePipeStaysBinding` 断言五件事：不得被他人覆盖、
攻击者拆不掉、仍计入本账号在途、原持有者 token 仍可用、参与方仍能正常拆除。

### 2.2 AGY-02 / GLM-01：证书失败提示接线

`App.tsx` 的 `authError: string | null` 改为
`connError: { kind: "auth" | "tls"; message: string } | null`。原横幅把
「鉴权失败:」和「检查密钥」写死了，而证书问题改密钥没有用——两类错误的修复动作
不同，文案与引导按钮都要分开。状态栏指示器同步区分「连接未授权」与「证书不受信任」。

按 GLM-01 的提醒，证书失败**只更新常驻横幅、不发 toast**：该事件随退避每 ≤30s
重发一次，toast 会堆叠，而重复 set 同一横幅内容天然幂等。

新增 4 条前端测试，并验证了鉴别力——把监听名改掉后 4 条全红：

```
× TLS 证书失败提示 > 注册了 tls-cert-failed 监听
× TLS 证书失败提示 > 证书失败时显示常驻横幅，并引导去连接设置
× TLS 证书失败提示 > 重复收到同一事件不会堆叠横幅
× TLS 证书失败提示 > 连接恢复后横幅消失
```

### 2.3 AGY-03 前半：接收方拒收不再让发送方挂起

`lib.rs` 的 `TRANSFER_ANSWER` 处理补上 `else` 分支，复用 `TRANSFER_FAILURE`
那条路径的清理形态：双删 `pending_outbound` / `pending_inbound`、历史行置 FAILED
（错误文案「对方拒绝了本次传输」）、emit 一条 FAILED 的 `transfer-progress`、
修剪历史。

### 2.4 AGY-04 / GLM-02：session_id 校验接线

`TRANSFER_ANSWER` 分支在进入授权前对 `answer.SessionID` 调
`auth.ValidateSessionID`。非法时**不转发、也不回执**——注释写明了理由：
`buildFailure` 依赖 session_id 匹配客户端卡片，空或畸形的 id 本来就没有卡片可落，
硬发一条只会变成一次到不了任何地方的推送。只能记日志并丢弃。

这同时覆盖了 AGY-03 后半所担心的那种畸形输入。

### 2.5 GLM-03：拆除结果改枚举，消除 TOCTOU

`RemovePipeForSession` 的返回从 `bool` 改为 `(TeardownResult, string)`，
在持锁期间一并定下拒绝原因与归属账号。`OwnerOfSession` 随之删除——
它只有那一个调用点，留着就是一个「看起来通用、实则服务单处」的导出函数。

新增 `TestTeardownDeniedCarriesOwnerAccount`，并把
`TestRemovePipeForSessionOnUnknownSessionIsNotAnError` 的断言从「不成功」
收紧为「必须是 `TeardownNotFound` 且不带归属账号」——调用方正是据此
把它记成 debug 而非越权告警。

---

## 3. 验证

```
server:  go test ./...        全部 ok
         go test -race ./...  全部 ok，无 DATA RACE
         go vet ./...         干净
client:  cargo test           78 passed
         npm test             36 passed（新增 4 条）
         npx tsc --noEmit     干净
```

## 4. 两侧共同的盲区

两位审查员都未运行任何测试，结论全部基于静态阅读。ZCode 明确列出
`is_cert_error` 对 tokio-tungstenite 0.23 实际错误文案的命中率未实测——
这一条我也没有实机验证，它决定的是证书失败时用户看到的是专门提示还是
通用的「连接失败」，漏判后果轻（退回通用提示，不会误导用户去关校验），
但仍是一处未经验证的假设。同理，**默认档（传 `None` 的 connector）是否真的
执行了 webpki 校验，本轮没有用自签证书实机验证过**——若该假设不成立，
整个范围 B 等于没做。这是目前最值得补一次手工验证的地方。

## 5. 审批后的实机验证与补充改动（**并入下一轮审查**）

> 本节记录的改动发生在 code 阶段**审批之后**，因此不在本轮双审的快照里。
> 用户已决定不为此单开一轮复审，而是并入下一轮（历史 / 缓存按账号分区）一起审。
> 下一轮的审查员看到这两处时，它们不是来路不明的漂移。

§4 列为「最值得补一次手工验证」的那条已经验证完毕：用自签证书起本地 TLS
服务端 + badssl.com 的公网端点，对两档各跑一遍。

**结论：范围 B 的核心假设成立。** 服务端日志是决定性证据：

| 档位 | 服务端 | 客户端 |
| :--- | :--- | :--- |
| 默认（connector=None） | `HANDSHAKE REJECTED BY PEER: TLSV1_ALERT_UNKNOWN_CA` | `invalid peer certificate: UnknownIssuer` |
| 不安全（Rustls + Insecure verifier） | `HANDSHAKE OK TLSv1.3 TLS_AES_256_GCM_SHA384` | 过 TLS，止步于 WS 握手（预期行为） |

默认档对四种无效证书全部拒绝：self-signed / expired / wrong-host
（badssl.com）加本地自签。顺带解答了两侧都列为盲区的那条——
`is_cert_error()` 对这四种错误文本**全部命中**，没有漏判。

验证过程中我出过两次错，记下来因为它们都是方法论问题：

1. 一度断言「默认档没有拒绝自签证书，范围 B 等于没做」。那是假的：
   `openssl s_server`（LibreSSL 3.3.6）与 rustls 0.23 协商不上，握手卡住，
   而我的探测程序把 8 秒超时写成了「TLS 握手已通过」。**用超时推断握手成功
   是错误的判据**，换成 Python 的 ssl 服务端后真相才出来。
2. 一度报告「默认档会 panic，真实客户端一样会发生」。前半对、后半错：
   `lib.rs:47` 在 `run()` 开头就装了 CryptoProvider，是我的验证程序缺了这一步。

**两处补充改动**：

- `client/src-tauri/src/core/connection_actor.rs`：`install_default()` 移到
  两档都覆盖的位置。今天它是冗余的（`lib.rs` 已装），但默认档不该依赖一个
  远在别处的副作用才能不 panic——而默认档恰恰是绝大多数用户走的那条路。
  rustls 0.23 在 ring 与 aws-lc-rs 共存时（本项目正是）无法自动选定 provider，
  失败形态是 panic 而不是 Err，发生在连接 actor 的 task 里会让整个重连循环死掉。
- `docs/USER_GUIDE.md`：补上一条实测发现——**自签证书必须是 X.509 v3 且带
  SAN**，否则勾选「允许不安全连接」也连不上。rustls 在**解析阶段**就拒绝 v1
  证书，那一步早于「是否校验签发者」的判断，所以开关对它无效。macOS 自带
  LibreSSL 的 `openssl req -x509` 默认生成的正是 v1，文档给出了带扩展的正确命令。
