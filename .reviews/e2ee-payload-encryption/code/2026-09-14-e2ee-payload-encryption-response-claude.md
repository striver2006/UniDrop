---
schema: trivium.disposition.v1
topic: e2ee-payload-encryption
stage: code
role: claude
kind: response
run_id: 20260913T163647Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - GLM-01
  - GLM-02
  - GLM-03
---

# 代码审查应答：E2EE 载荷加密

- 主题：`e2ee-payload-encryption`
- 阶段：code（应答稿，含逐条裁决）
- 日期：2026-09-14
- 角色：Driver (claude)
- 被审补丁：相对 `119ce540` 的 19 个文件变更，103.2 KB
- 被裁决的审查：`2026-09-14-e2ee-payload-encryption-gemini.md`、
  `2026-09-14-e2ee-payload-encryption-glm.md`

---

## 0. 总览

两侧均判 `request-changes`：Antigravity 3 条（阻断 1 ｜ 重要 1 ｜ 次要 1），
ZCode 3 条（重要 1 ｜ 次要 1 ｜ 吹毛求疵 1）。合计 6 条，**全部接受**，
无驳回、无暂缓。

本轮焦点问的四件事，两侧独立核实后都判成立：三条成帧路径全部经 `build_data_frame`
加密（ZCode 另外确认 `BinaryHeader::new_data` 在生产代码里只剩这一个调用点，
不存在第四条路径）、四处块长同源无遗漏、协商前置确实绕开了 ANSWER 重建、
AAD 取空的论证站得住。

**但功能在 HEAD 上整体不可用**，两侧独立发现同一处：

| | 我写的 | 实际后果 |
| :--- | :--- | :--- |
| `E2EE_MIN_VERSION_TUPLE` | `(0, 3, 0)` | 两台从当前 HEAD 构建的**新**客户端互相判为「不支持」 |
| `Cargo.toml` version | `0.2.0` | 每次传输都回落明文 + 弹一次「未加密」提示 |

而 `app_version` 恰恰取自 `env!("CARGO_PKG_VERSION")`——我给一个尚未存在的版本
设了门槛，却没有任何机制把门槛与实际包版本绑在一起。

**最刺眼的是我的测试把这个矛盾固化了**：`peer_capability_gate` 里那行
`assert!(!peer_supports_e2ee(Some("0.2.0")))` 断言的正是当前版本不支持加密。
测试写得"对"，功能却是死的——**测试证明了门槛逻辑正确，但没有任何一条测试问过
「这个门槛对我们自己成立吗」**。ZCode 给的守卫（`assert!(peer_supports_e2ee(
Some(APP_VERSION)))`）一行就能堵住，而且它今天就会红。

第二处也是我自己测出来的盲区：`seal_offer_metadata` 原地抹空了 offer，
而那个 offer 随后被写进发送端的 `pending_outbound` 与历史库。我的
`encrypted_offer_leaks_no_filename_or_hash` 验证了**安全属性**（线上没有明文），
却没有任何一条测试验证**功能属性**（本地还看得见文件名）。这是同一族问题的又一面：
前两轮分别是「假绿断言」「测不出要修的缺陷」「测了逻辑没测接线」，这轮是
**测了安全没测功能**。

---

## 1. 裁决表

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | 阻断 | 接受 | 已核实：`Cargo.toml:3` 是 `0.2.0`，`e2ee.rs:285` 门槛是 `(0,3,0)`，而 `app_state.rs:17` 的 `APP_VERSION = env!("CARGO_PKG_VERSION")` 正是上报给对端的值。结论成立且后果就是它说的：**默认开启的加密功能在合并点上整体失效**，且每次传输都弹一次回落提示。这不是"发版时记得升"能兜住的——没有任何机制阻止它再次漂移。 | §2.1 |
| GLM-01 | glm | 重要 | 接受 | 与 AGY-01 同一缺陷，但**它给出的修法更好**，我按它的落地。两点独有价值：① 指出我的 `peer_capability_gate` 测试把 `0.2.0` 钉死为不支持，等于**把矛盾写进了测试**，所以全绿反而掩盖了功能是死的；② 提出用 `assert!(peer_supports_e2ee(Some(APP_VERSION)))` 把门槛与实际包版本绑死——这条守卫今天就红，强制「升版本」与「合并功能」在同一个变更里完成，从根上消除漂移。两侧定级不同（AGY 判 blocker、GLM 判 major），**我按 AGY 的定级处理**：功能整体不生效就是阻断。 | §2.1 |
| AGY-02 | gemini | 重要 | 接受 | 已核实成立，而且比它描述的还要广一点。`seal_offer_metadata` 用 `std::mem::take` 原地抹空，返回的 offer 随后在 `dispatch_offer` 里同时进了两处：`pending_outbound`（→ `start_sender_task` 的 `transfer-progress` 事件，卡片预览为空）与 `HistoryRepo::record_task`（→ 发送端自己的历史行落空文件名、空 sha256）。`lib.rs` 里我特意写了注释要求接收端"先解密再落库"，却在发送端犯了镜像的错误。 | §2.2 |
| AGY-03 | gemini | 次要 | 接受 | **我认为它被低估了，按重要处理。** 已核实：熔断 `break` 后落进 `!fully_completed` 分支（`transfer_engine.rs:1017`），`finalize_history_status` 写入泛化的「接收连接中断或校验失败」覆盖本地历史，并再发一条 `RECEIVER_DISCONNECTED` 覆盖发送端已收到的 `E2EE_AUTH_FAILED`。后果是**两端最终看到的都是错误的原因**：我在 §2.4 费力写的「先指向 PSK 不一致」的文案，在最常见的触发场景下根本到不了用户眼前。一条精准诊断被次生错误吃掉，等于没做。 | §2.3 |
| GLM-02 | glm | 次要 | 接受 | 已实测确认：`"0.3.0-beta.1".split('.')` 得 4 段，被 `it.next().is_some()` 判为 `None` → 不支持。它点出的时机问题是真的痛点：**恰好在用预发布包验证功能的阶段功能不可用**，而现象（回落 + toast）与原因（版本号带点）之间毫无线索。我原本的 patch 段截断逻辑只处理了 `0.3.0-beta` 这种不带点的形态，测试也只覆盖了那一种。 | §2.4 |
| GLM-03 | glm | 吹毛求疵 | 接受 | 已核实：`cmd_send_clipboard` 里 `decide_e2ee` → `emit_fallback` 排在 `spawn_blocking` 读剪贴板之前，剪贴板为空时命令返回 `Err`，但用户已经先收到一条「本次传输未加密」——**针对一次根本没发生的传输**。`cmd_send_files` 因为 precheck 排在前面而没有这个问题。两条路径顺序不一致本身也值得收敛。 | §2.5 |

---

## 2. 修改方案

### 2.1 版本门槛与包版本绑死（对应 AGY-01 / GLM-01）

两步，缺一不可：

**① 加守卫测试**（`e2ee.rs`）：

```rust
/// 门槛必须对**我们自己**成立：本 crate 的版本要能通过 peer_supports_e2ee。
///
/// 没有这条守卫时，E2EE_MIN_VERSION_TUPLE 与 Cargo.toml 的 version 是两个
/// 可以各自漂移的数字，而漂移的后果是**功能静默失效**——两台新客户端互判
/// 不支持、永远回落明文，全套测试却照样全绿（peer_capability_gate 甚至
/// 断言了当前版本不支持，把矛盾固化成了"预期行为"）。
#[test]
fn our_own_version_passes_the_gate() {
    assert!(
        peer_supports_e2ee(Some(crate::app_state::APP_VERSION)),
        "本 crate 版本 {} 低于 E2EE 门槛 {:?}——E2EE 对所有对端都会回落明文",
        crate::app_state::APP_VERSION, E2EE_MIN_VERSION_TUPLE
    );
}
```

同时把 `peer_capability_gate` 里那行 `assert!(!peer_supports_e2ee(Some("0.2.0")))`
改成用一个明确的历史版本字面量并加注释说明它代表"发布 E2EE 之前的版本"，
避免它再次与当前版本重合。

**② 版本号升至 `0.3.0`——这一步需要你授权。**

项目规则明确「**不要自动修改版本号字段**」，而这里升版本不是发版决策的附带动作，
它是**本功能能否工作的前提**：门槛不能降（`0.2.0` 是已发布的、确实不支持 E2EE 的
版本，降下去会让新客户端给旧客户端发出它解不开的 OFFER，正是 plan 阶段
AGY-01 要避免的形态），所以只能升版本。

涉及四处，与上次升版本同一组：`client/package.json`、`client/src-tauri/Cargo.toml`、
`client/src-tauri/tauri.conf.json`、`Cargo.lock` 本包条目。

**在你授权之前，守卫测试会是红的**——这是有意的，它正确地反映了"功能现在不可用"
这个事实。我不会为了让测试变绿而把守卫删掉或改弱。

### 2.2 发送端保留完整元数据（对应 AGY-02）

把「密封」从 `prepare_offer` 移到 `dispatch_offer`，职责重新切分：

- `prepare_offer` / `prepare_offer_from_bytes`：产出**本地视角的完整 offer**——
  含真实文件名与 sha256，同时按加密块长算好 `total_chunks`、设好
  `encrypted` / `e2ee_version`。它仍然需要 `e2ee_key`，但只用于判断块长。
- `dispatch_offer`：在构造 `TRANSFER_OFFER` 信令时，若 `offer.encrypted` 则先做一份
  **密封副本**发往中继；`pending_outbound` 与 `HistoryRepo::record_task` 继续用
  原始 offer。

因此 `seal_offer_metadata(&mut offer, key)` 改为
`sealed_offer_for_wire(&offer, key) -> Result<TransferOfferPayload, String>`——
返回副本而不是原地改，让「本地那份被抹空」在类型层面就不容易发生。

密钥需要一路传到 `dispatch_offer`。`LessSafeKey` 不是 `Clone`，
用 `Option<Arc<LessSafeKey>>` 承载，`decide_e2ee` 产出时即包 `Arc`。

**配套测试**（这次要把功能属性也钉住）：

- `sender_keeps_plaintext_metadata_locally`：加密传输后，断言**本地** offer 的
  `relative_path` / `sha256` / `preview_summary` 仍是真值，而
  `sealed_offer_for_wire` 的产物里没有它们。现有的
  `encrypted_offer_leaks_no_filename_or_hash` 保留，两条一正一反成对。
- 断言 `record_task` 收到的 offer 与 `prepare_offer` 的返回值是同一份
  （不被密封污染）。

### 2.3 熔断后不发次生错误（对应 AGY-03）

接收循环加一个 `e2ee_aborted: bool`，熔断时置位并 `break`。
`!fully_completed` 分支据此分流：

- 已熔断：**不再**发送 `RECEIVER_DISCONNECTED`（发送端已经收到
  `E2EE_AUTH_FAILED`），`finalize_history_status` 传入与熔断一致的精准原因；
- 其余情况：行为完全不变。

注释写明为什么不能靠"后发的覆盖先发的"：两条失败信令走同一条 `session_id`，
后到的会在 `lib.rs` 覆盖先到的错误文案，于是**越精准的诊断越先被发出、也越先被覆盖**。

沙盒清理（`remove_dir_all`）在两条路径下都要保留——熔断同样意味着落盘的分片不可信。

### 2.4 预发布版本号解析（对应 GLM-02）

`parse_semver` 改为**先在第一个 `-` 处截断，再按点分段**：预发布标识整体丢弃，
只比较数字三元组。这样 `0.3.0-beta.1`、`0.3.0-rc.1`、`0.3.0-alpha.2.3` 一律
等价于 `0.3.0`。

测试补 `0.3.0-beta.1` 与 `0.2.0-rc.1`（后者必须仍判不支持，确认截断没把
主版本号一起吃掉）。

### 2.5 回落提示的时机（对应 GLM-03）

`cmd_send_clipboard` 里把 `emit_fallback` 移到 `spawn_blocking` 成功返回**之后**，
与 `cmd_send_files` 保持同一顺序：**先确定这次传输真的会发生，再告诉用户它没加密**。

注释写明这个顺序约束，否则以后给 `cmd_send_files` 加预检时很容易再次颠倒。

---

## 3. 验证计划

改完后除常规三套外，重点复验：

- `our_own_version_passes_the_gate` 在版本升到 `0.3.0` 后转绿，
  且把版本改回 `0.2.0` 会立刻变红（这条守卫的存在意义就是这个）；
- 新增的发送端元数据测试做撤改动验证：把密封改回原地修改，
  `sender_keeps_plaintext_metadata_locally` 必须红；
- 熔断分流：构造篡改场景，断言只发出一条 `E2EE_AUTH_FAILED`、
  没有后续的 `RECEIVER_DISCONNECTED`。

---

## 4. 两侧的独立价值与本轮记录

**两侧在同一处缺陷上给出了不同深度。** AGY-01 定级更准（功能整体失效 = 阻断），
GLM-01 的修法更好（守卫测试把两个数字绑死，而不只是"这次记得升"）。
我采纳了 AGY 的定级 + GLM 的修法。

**AGY-02 是 ZCode 没看到的。** 它追踪的是 `seal_offer_metadata` 返回之后
那个 offer 又去了哪里——跨了三个文件（`e2ee.rs` 抹空 → `clipboard_cmd.rs`
落库 → `transfer_engine.rs` 上报进度）。只读单点看不出问题，每一处单独看都是对的。

**ZCode 的"认为正确的部分"这次做了真正的独立核实**，不是复述：它自己走通了
`auth 上报 → registry/session.go:88 ToOnlineDevice → DEVICE_LIST_SYNC →
lib.rs 填充 online_devices` 这条链，确认协商链路真实存在；也自己确认了
`BinaryHeader::new_data` 在生产代码里只剩 `build_data_frame` 一个调用点。
这两条正是焦点要问的，它给的是证据而不是同意。

**一条给自己的记录**：我这轮的两处缺陷（版本门槛、发送端抹空）有同一个形状——
**测试覆盖了我关心的那一面，而缺陷在另一面**。门槛逻辑测了（对端版本低要拒绝），
没测「我们自己能过」；加密测了（线上没明文），没测「本地还看得见」。
写安全相关的测试时，每断言一个"不应该有什么"，都要配一条"应该还有什么"。
