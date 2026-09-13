# 多账号硬化 + TLS 证书校验修复 — 计划

- 主题：`multi-account-hardening`
- 阶段：plan
- 角色：claude（Driver）
- 日期：2026-09-13

---

## Context

起因是两个核验问题：「账号标识 + 共享密钥是不是已经同时支持多用户了」「全程加密是否已实现」。核验结论：

**多用户**——功能上已经成立。所有分发路径都做了同账号过滤（`ListByAccount` / `BroadcastToAccount` / `routeToPeer` 与 `rejectToPeer` 的 AccountID 比对 / `AuthorizeSessionIfUnderLimit` 的 per-account 计数），没有任何一处「广播给所有已连接客户端」。但 `docs/需求.md:81` 第 6 条没标已实现，因为凭据层是单把全局 PSK、`account_id` 由客户端自报且服务端不校验——账号是「便利分区而非安全边界」。

**加密**——没有实现。E2EE 只有协议位占坑（`FLAG_ENCRYPTED`、`encrypted` 字段全部硬编码 `false`，零密码学依赖），属 Phase 3 规划。而传输层有一个现成的洞：客户端 `client/src-tauri/src/core/connection_actor.rs:29-38` 的 `verify_server_cert` 无条件返回 `Ok(assertion())`，控制面与数据面全走这条路，**即使连 `wss://` 也挡不住中间人**。

**本轮范围（用户已确认）**：保留单把 PSK 的凭据模型不变，只修「即使所有人都守规矩也会真实串台或互相挤占」的硬化项；加上 TLS 证书校验修复。E2EE 另开一轮。

预期结果：账号仍不是安全边界（这个取舍不变），但不再互相误伤；内容在公网上不再对中间人裸奔。

---

## 范围 A：多账号硬化

依赖顺序：步骤 1 是 2、3 的正确性前提（没有非空 account_id，复合键会重建同一个公共桶）。4 依赖 3，5 依赖 2/3。

### A1. account_id / device_id 格式校验

**新建 `server/internal/auth/identity.go`**（放 `auth` 包而非新建包：它和 `BuildCanonicalString` 约束同一串字节）。导出 `MaxIdentifierLen = 64`、`ValidateAccountID` / `ValidateDeviceID` / `ValidateSessionID`、`ErrInvalidAccountID` / `ErrInvalidDeviceID`。

| | 长度 | 字符集 |
|---|---|---|
| `account_id` | 1–64 字节 | `[A-Za-z0-9._@-]` |
| `device_id` / `session_id` | 1–64 字节 | `[A-Za-z0-9_-]` |

**调用点：`server/internal/controller/control_ws.go` 握手，排在 `h.verifier.VerifyWithSalt` 之前**。三条理由都要写进注释：

- 不塞进 `verifier`：`Verify*` 的契约是「这串字节是不是本 PSK 签的」，混入格式判断会让它对同一失败返回两种语义。
- **必须排在验签之前**：`VerifyWithSalt` 在签名通过后会 `CheckAndAdd(nonce)` 烧掉 nonce（`verifier.go:88`）。格式非法但签名正确的请求若排在后面，重试会因同一 nonce 被判重放，错误从「账号格式非法」漂成「replay detected」，用户永远改不对。
- 代价与对策：校验因此跑在未鉴权的攻击者可控输入上（控制面读上限 512 KiB）。`Validate*` 必须**先查长度再扫字符集**，手写 byte 循环，不得用 `regexp` 或先 `TrimSpace`（会分配）。

需要解释性注释的格式决定：**禁 `\n`**（`BuildCanonicalStringWithSalt` 用 `\n` 分隔字段，允许换行等于让客户端重排签名串的字段边界——现有取值下不可利用，但这是一整类 bug，不该靠「恰好利用不了」活着；`verifier.go` 侧加一行反向指针）；**ASCII-only**（account_id 是匹配键，`"团队"` 的 NFC/NFD 字节不同肉眼相同，会静默分成两个桶——与本轮要修的空账号公共桶是同一缺陷的镜像）；**不做成环境变量可配**（可配的身份文法 = 没人配的旋钮 + 翻倍测试矩阵，且「放宽格式」正好抵消本轮目的）。

**拒绝通道**：复用现有 AUTH_RESPONSE。`server/internal/protocol/envelope.go` 收编错误码常量（现在 `"UNAUTHORIZED"` 是 control_ws.go 里的裸字面量）：`AuthErrUnauthorized` / `AuthErrInvalidAccountID` / `AuthErrInvalidDeviceID`。把 control_ws.go 内联的失败应答抽成 `writeAuthFailure(ctx, ws, traceID, code, message)`——本步新增两个拒绝点，第三次复制那 17 行就该抽了。

`ErrorMessage` 用**中文且带上规则本身**（对齐 `limits.Violation` 的「用户只看到『文件太大』是改不了的」约定）：`账号标识不能为空，只能包含字母、数字与 . _ @ -，长度 1-64`。UNAUTHORIZED 分支现在塞的是 `err.Error()`（英文内部错误），**本轮不改**，但在 `writeAuthFailure` 上注释说明两类 message 受众不同，免得后来人「统一」掉。

客户端接收侧已通：`connection_actor.rs:196-204` → `auth-failed` → App.tsx 红色横幅，**无需改连接层**。

**客户端前置校验（两层都必要）**：

- `client/src/components/SettingsModal.tsx`：新增 `parseIdentifier`，形态对齐既有 `parseU32`，进 `nextFieldErrors`。**去掉 account_id 输入框的 `required`**——理由就是文件里已有的那条注释（原生校验在 submit 前拦下，handleSubmit 根本不跑，用户看到样式不可控的英文原生气泡，而填非法字符时看到的是我们的中文红字，同一输入框两套错误呈现）；**更关键的是 `required` 对 `"   "` 判通过**，而 handleSubmit 里的 `.trim()` 会把它变成空串提交。`server_url` / `psk_secret` 的 `required` 本轮不动，注释里明说这是有意的局部不一致。
- `client/src-tauri/src/commands/settings_cmd.rs` 的 `cmd_save_settings`：加 Rust 侧兜底。**为什么要两道**：设置面板不是唯一写入方，`default_config()` 与老库反序列化都直接流进 `config_actor`（lib.rs:70）。仓库已有同形态先例（`cache_sweep_interval_minutes` 的前后端各校验一次）。注释要指出它必须与 `auth/identity.go` 的规则一致、以及不一致时的表现。

已核对：默认值 `default_user`、文档示例 `my_team_sync`、测试里的 `user1` / `alice` / `acct` 全部通过新规则，**默认安装不受影响**。

### A2. registry 改复合键

**推荐复合键 `sessionKey{AccountID, DeviceID}`**，不用「扁平键 + 显式比对」。改动面实测只有 2 个生产调用点（`Get` 在 control_ws.go 的 `routeToPeer` 与 `rejectToPeer`，`Unregister` 零生产调用），且两处因这次改动**变简单**（`if targetSession.AccountID == session.AccountID` 分支整个消失）。

理由（写进注释）：扁平键 + 比对把不变量留在约定里——map 的键仍在宣称「DeviceID 全局唯一」，而它不是；以后任何人给 registry 加方法都得自己记得再比一次账号，正是 `countAuthorizedForAccountLocked` 那段注释拒绝的形态。

`server/internal/registry/registry.go` 改动：`sessions` 换键 + `keyOf(s)`；`Register` 签名不变（**同 `(account, device)` 仍 `old.Close()`，正当的同设备重连顶号原样保留**）；`Get` / `Unregister` 加 `accountID` 首参（破坏性，但仅 Go 内部）；`UnregisterSession` 的 CAS 按 `keyOf(s)` 取；`SweepInactive` 遍历改键；`ListByAccount` / `BroadcastToAccount` / `Count` 行为不变。顶部写一条：`AccountID` 非空是本结构的前置条件，由 `auth.ValidateAccountID` 保证，删掉其中之一公共桶就会回来。

**必须显式记录的语义损失**：改前 `routeToPeer` 能把「跨账号被拦」和「对端离线」分开打日志，复合键之后归一。这是**有意接受**的——对 A 账号而言「别的账号存在一台同名设备」不是它有权知道的信息，写进日志本身也是一次跨账号信息外泄到运维视野。日志文案改成 `"peer not found in account"`，注释写明这是故意放弃不是漏改。

### A3. relay 会话归属保护

**这里与 A2 相反，推荐保留扁平键 + 显式归属校验**，理由成对写进两处注释互相指向：device_id 是用户可见、跨账号会自然撞车的标识，撞车是常态，该分区；session_id 是每次传输现铸的 UUIDv4（`transfer_engine.rs:956,974`），撞车是异常，该**记录并拒绝**而不是静默分区掉——分区会把「有人在复用会话 ID」这个信号吃掉。且数据面 URL（`data_ws.go:31-35`）里没有 account，复合键会逼着加一个客户端自报、唯一校验手段是我们已经在校验的 token 的字段，严格更差。

**改动 A — 授权覆盖保护**。`server/internal/relay/relay_manager.go` 新增 `ErrSessionIDConflict`。`AuthorizeSessionIfUnderLimit` 在加锁后、计数前插入：既有条目未过期且（`AccountID` 不同 或 `(sender, receiver)` 对不同）→ 冲突。四点注释：过期条目可自由覆写（死容量，报冲突只会让陈旧 ID 白毒键位 5 分钟）；同账号也要比设备对（合法重复授权只有「接收方重发 ANSWER」一种，设备对变了就是会话 ID 复用）；**冲突检查必须排在 `countAuthorizedForAccountLocked` 之前**——该函数按 ID 排除「自己」而不看账号，顺序颠倒会让 B 复用 A 的 sessionID 白得一个额度豁免；顺带对 sessionID 调 `auth.ValidateSessionID`（防 500 KiB 字符串当 map 键、控制字符进日志）。

**改动 B — 拆除归属保护（本轮最重要的一条）**。现状：`RemovePipe(sessionID)` 被 `control_ws.go:341-350` 在 TRANSFER_COMPLETE/FAILURE/CANCEL 时调用，`session_id` 由客户端自报，服务端直接双 `delete`，**连账号都不比**。这比改动 A 的覆盖写入更致命——覆盖至少还要铸 token，拆除不需要任何凭据。

改成 `RemovePipeForSession(sessionID, accountID, deviceID) bool`：无 auth 条目→拒绝（生产路径下 pipe 只能由 `ValidateAndGetOrCreatePipe` 创建，而它必须有 auth 条目）；账号不符→拒绝 + `slog.Warn("cross-account pipe teardown blocked")`；`deviceID` 不是 sender/receiver 之一→拒绝（**为什么账号内也卡设备**：数据面早就钉死在这两个角色上，元数据面比数据面松没有理由，且这两个值本来就在 `SessionAuth` 里，检查是免费的）。调用点传 `session.AccountID, session.DeviceID`，返回 false 打 warn——但**即使拆除被拒，`routeToPeer` 仍照常执行**，转发本身已受 A2 的账号内路由约束，CANCEL 信令本该送达对端。

**改动 C — 授权失败不得转发无 token 的 ANSWER**。现状 `control_ws.go:330-332`：`authErr` 非 `ErrConcurrencyLimit` 时只打一条 warn 就继续 `routeToPeer`，客户端 `if let Some(token)` 静默跳过 → 发送方永久挂起。这正是 control_ws.go:300-315 那段注释声称已消除的形态，`ErrSessionIDConflict` 会从这扇门重新走进去。改成**任何 `authErr != nil` 都双向拒绝、不转发**。`server/internal/limits/limits.go` 新增 `CodeSessionConflict`（`传输会话标识冲突，请重试`）与 `CodeAuthorizeFailed`（`服务端无法授权本次传输，请重试`）。注释说明：`Violation` 原本只承载限额，现在还承载授权失败——理由是它是唯一能同时把失败告知两端并让卡片落地的通道（`buildFailure` 依赖 `session_id` 匹配卡片）。已核对客户端不枚举 `error_code`（`HistoryPanel.tsx:185` 只做展示），新码无需客户端改动。

### A4. per-account pipe 上限

**先回答「已有的 per-account 授权限额够不够」：接近够，但有一个真实缺口。** pipe 只能由 `ValidateAndGetOrCreatePipe` 创建，而它要求存在未过期的 auth 条目，所以建 pipe 的瞬间受 `MaxConcurrentTransfers`（默认 8）约束。**缺口在生命周期不对齐**：auth 条目 TTL 5 分钟，pipe 的存活条件是 60 秒内有活动。持续涓流的 pipe 可以活过自己的 auth 条目过期——此时授权额度被释放而 pipe 还在，于是一个账号可以每 5 分钟循环一次把 pipe 刷到全局 200。现实严重性低，但确实是「单账号耗尽全局容量」的可达路径。

改动：`server/internal/relay/pipe.go` 的 `RelayPipe` 加 `AccountID`（值从 `auth.AccountID` 来，免费）；`NewRelayManager(maxPipesPerAccount int)`——**不留无参构造**，理由写注释：留一个就等于留一条能造出不设限管理器的路，默认路径必须是安全的那条；`ValidateAndGetOrCreatePipe` 在全局上限之前插 per-account 检查。

**顺序关键**：per-account 检查必须排在**已有 pipe 的命中返回之后**，否则账号打满上限时，传输中的 pipe 断线重连会被自己的上限打死，表现是「传到一半莫名中断」。这条要写注释并配测试。`MaxConcurrentPipes = 200` 保留，注释改成说明它现在是第二道线。

**不新增环境变量**，复用 `MaxConcurrentTransfers`——一个 session 恰好对应一条 pipe，单位相同、约束同一种资源，第二个旋钮只会在两者不一致时产生谁也说不清的行为。注释必须写明它多出来的唯一作用是堵住「pipe 活过 auth 条目」那个窗口，否则后来人会判定它与授权限额重复而删掉。`0` 继续表示关闭。

### A5. 可观测性（结论：不加 account_id label）

**必须拒绝 per-account label**：account_id 是未鉴权、客户端自报的任意字符串，单个客户端可以循环握手铸出无限多个，每个都在 exporter 里留一条常驻时序——那是针对服务端和抓取端的内存放大攻击面。

`server/internal/controller/health.go` 加：`unidrop_online_accounts`、`unidrop_max_devices_per_account`、`unidrop_max_pipes_per_account`（三个 gauge，需在 registry / relay 各加一个聚合函数）、`unidrop_auth_rejected_total{reason}`（reason ∈ `{invalid_account_id, invalid_device_id, unauthorized}`）。

「最大值」那两条是关键：用**一个数**回答运维真正要问的「是不是某一个账号在吃掉整台机器」，不产生任何 per-account 时序——这个理由要写注释，否则下一个人会觉得「那不如直接加 label」。`{reason}` 是代码里定义的闭集枚举，基数由构造有界，与 account_id 本质不同，也要注释，免得被当成先例。这条 counter 是 A1 破坏性变更的唯一自动化观测手段。

`/healthz` JSON 同步加三个聚合字段，**不要塞账号 ID 列表**——它是未鉴权端点（main.go:77），列表等于把全部账号名交给任何能 GET 的人。

顺带：`MetricsTracker.TotalOffers` / `RetransmitsCount` 被定义、被赋零值、从未被读也从未被写，与 `config.go` 顶部注释痛斥的 `MaxItemsPerOffer` 是同一缺陷，一并删掉并加同款「每个字段都必须有人读」注释。新字段用 `atomic.Uint64`。

### A6. 客户端 paired_devices：删表

查清结果：**这张表全仓库零读零写**。命中只有 `client/src-tauri/src/storage/db.rs:69-78` 的建表语句、DESIGN.md 的设计稿、以及 `docs/reviews/2026-09-11-*` 已点名「从未使用」至今未处理。没有 repo 模块，没有任何 SQL。它不是「没有按 account_id 过滤」，是**没有任何查询可供过滤**。

`create_schema` 移除该表；在 db.rs 已有的宽松迁移块加 `DROP TABLE IF EXISTS paired_devices;`。理由写注释：这正是 `config.go` 顶部注释确立的先例；更糟的是它**主动制造了账号分区的假象**——一个 `account_id TEXT NOT NULL` 的列会让读 schema 的人以为配对表已按账号分区，本次需求就是被它误导出来的（这已是它造成的第二次误判）。`DROP` 在这里可证明无损（从不存在写入方），注释里要写明**不要**把这个模式抄到有数据的表上。DESIGN.md §5 加一句：配对/E2EE 尚未实现，表不再预建。

### A7. 测试

风格：中文 `t.Fatalf` 文案，每个用例上方一条注释说明「这条断言被删掉之后会退回成什么缺陷」（对齐 `TestAuthorizeSessionIfUnderLimitIsAtomicUnderConcurrency`）。每项测试跟随对应步骤，不排在最后。

- **新建 `server/internal/auth/identity_test.go`**：空 / 纯空格 / 含 `\n` / 超长各一条负例；`TestValidateAccountIDAcceptsShippedValues`（`default_user` / `my_team_sync` / `user1` / `alice` 必须通过，钉死「新规则没把已发布的默认值和文档示例打死」）；`TestValidateIsAllocationFreeOnOversizedInput`（长度先于字符集判定，防有人改成 regexp 后在未鉴权路径上被打）。
- **新建 `server/internal/controller/identity_ws_test.go`**：前置重构——`limits_ws_test.go` 的 `connect`（56-116）鉴权失败时 `t.Fatalf`，无法用来测拒绝，拆成 `connectRaw` + `connect`。用例含四类拒绝 + 一条正例 + **`TestRejectedIdentityDoesNotBurnNonce`**（同一 nonce 改正 account_id 后重试必须成功，钉死校验排在验签之前这个顺序决策）。
- **扩充 `registry_test.go`**：`TestRegisterSameDeviceIDDifferentAccountsCoexist`（核心负例，改回扁平键就红）、`TestRegisterSameAccountSameDeviceStillReplaces`（正例，确认没把正当顶号一起修掉）、`Get` 按账号作用域、CAS 不误摘他账号同名条目、广播不漏发不串发、`SweepInactive` 跨账号、聚合函数。
- **新建 `server/internal/controller/cross_account_ws_test.go`**：`TestCrossAccountSameDeviceIDDoesNotKickExistingPeer`（用户最在意的那条）、路由不跨账号、**`TestCrossAccountPipeTeardownBlocked`**（B 拿 A 的 session_id 发 CANCEL，断言 A 的 pipe 与 auth 条目仍在——A3 改动 B 的端到端守卫）、`TestAnswerAuthorizationFailureRejectsBothPeersInsteadOfForwarding`。
- **扩充 `relay_test.go`**：跨账号覆盖被拒且**旧 token 仍有效**（只断 error 不够）、同账号设备对不符被拒、同账号重授权放行（正例）、过期条目可覆写、`TestConflictCheckPrecedesConcurrencyCount`、拆除的三条负例 + 两端参与者都能拆的正例、`TestPipeCapIsPerAccountNotGlobal`、`TestExistingPipeLookupSucceedsAtCap`（正例，钉死 A4 的检查顺序）、全局上限仍生效。
- **新建 `server/internal/controller/health_test.go`**：**`TestMetricsHasNoAccountIDLabel`**（抓 `/metrics` 全文断言不含 `account_id=`，这是基数闸门的唯一自动化守卫，注释写明它防的是「有人照 DESIGN.md 原稿去加 label」）、聚合指标正确、reason 闭集、`/healthz` 不泄露账号名。`Metrics` 是包级单例，测试间需复位。
- **扩充 `e2e_test.go`**：`TestCrossAccountIsolationEndToEnd`——两账号各两台设备、**跨账号 device_id 完全相同**，A 内跑完整传输，断言传输成功、B 全程无信令、B 未被踢。
- **客户端**：`settings_cmd.rs` 的 `mod tests` 加四条校验用例 + `legacy_settings_json_with_invalid_account_id_still_deserializes`（反序列化不该失败——校验发生在保存/连接时，否则老库整条读不出来会触发 `unwrap_or_else` 冲掉全部配置，这是该文件既有注释反复强调的风险）；`SettingsModal.test.tsx` 新建一组账号校验，含「纯空格被拦下」那条。

---

## 范围 B：TLS 证书校验

### B1. 默认走真实校验

`client/src-tauri/src/core/connection_actor.rs`：`create_tls_connector()` → `create_tls_connector(allow_insecure: bool)`。`false`（默认）用 webpki 根证书构建 `ClientConfig`（`tokio-tungstenite` 已启用 `rustls-tls-webpki-roots` feature，无需新增依赖）；`true` 才走现有的 `CustomServerCertVerifier`。

`CustomServerCertVerifier` **保留但改名为 `InsecureServerCertVerifier`**，并在类型上方写明它跳过服务器身份校验、仅供自签证书/直连 IP 的内网部署，以及启用后的确切后果（任何中间人出示自签证书即可接管连接，读走剪贴板明文与文件字节）。现有那两行 `// Same TLS policy as the control plane (supports self-signed / direct-IP deployments)` 注释要一并更新——它现在描述的是默认行为，改完之后描述的是可选行为。

三个调用点：`connection_actor.rs:131`（从 `current_cfg` 取，`ConnectionConfig` 加字段）、`transfer_engine.rs:261` 与 `:592`（`start_sender_task` / `start_receiver_task` 已经在接 `server_url: String`，新参数跟着 `server_url` 的取值点一路传下来）。

### B2. 设置项

`AppSettings` 加 `allow_insecure_tls: bool`，`#[serde(default)]`。**这里可以用裸 default，与 `history_max_entries` 那几个字段相反**——`bool` 的 `Default` 是 `false`，恰好是安全值，老库升级后默认变成「校验证书」。这个反差要写注释，否则会被误认为漏写了自定义 default。

UI 加勾选项，文案必须说清后果而不是只说「允许自签证书」，参考 `limits.Violation` 的「用户只看到『文件太大』是改不了的」约定。勾选时给一条显式警告。

### B3. 升级行为与文档

**这是一处会让既有部署连不上的破坏性变更**：用自签证书或 IP 直连的用户升级后会握手失败。表现是 `create_tls_connector` 构建的连接报 TLS 错误 → 现有重连退避，**用户看到的是连不上而不是「证书不受信任」**。因此需要在 `connection_actor` 的连接失败分支识别 rustls 证书错误，emit 一条能落到界面上的专门提示（指向新的设置项），否则用户只会看到反复重连。

文档：`docs/USER_GUIDE.md` 说明新设置项与何时需要勾选；`docs/design/DESIGN.md` 补充客户端 TLS 策略。

### B4. 测试

TLS 握手的单测成本高，本轮做到：`allow_insecure_tls` 缺省反序列化为 `false`（钉死老库升级是安全值）、勾选状态能正确传到三个调用点。**用自签证书起测试服务器断言默认配置连接失败/勾选后成功**——价值高但需要新测试依赖，标为可选，实施时若成本可控就做。

---

## 文档同步清单

| 文件 | 改什么 | 必要性 |
|---|---|---|
| `docs/design/DESIGN.md` §4.4 | canonical string 段落补充 account_id / device_id 的格式契约（长度、字符集），说明禁 `\n` 是为防分隔符注入 | 必须——协议契约，第三方客户端照它实现 |
| `docs/design/DESIGN.md` §4.6（~592 行） | **修订** `unidrop_connected_devices{account_id, os_type}` 为无 label 的聚合指标，写明为什么不能加 account_id label | 必须——现文是一条照做会有害的规范 |
| `docs/design/DESIGN.md` §5（~1017-1051 行） | 标注配对 / E2EE 未实现、`paired_devices` 不再预建 | 必须 |
| `docs/design/DESIGN.md` | 补充客户端 TLS 策略与 `allow_insecure_tls` | 必须 |
| `docs/USER_GUIDE.md` §2.2（53-54 行） | 账号标识字段补充格式要求与拒绝时的表现；新增 TLS 设置项说明 | 必须——本轮两处用户可见的破坏性变更 |
| `server/configs/.env.example` 18-19 行 | 「知道 PSK 的客户端可以声明任意 account_id，账号隔离是便利分区而非安全边界」**保持不变**（取舍没变），补一句本轮已修掉「即使各方守规矩也会串台」的几处 | 建议 |
| `server/configs/.env.example` 传输限额段 | `UNIDROP_MAX_CONCURRENT_TRANSFERS` 注释补充：它**同时**作为每账号在途 pipe 上限，说明为什么不另开变量 | 必须——不写运维不知道这个数被复用了 |
| `server/configs/.env.example` | **不新增任何变量** | — |

---

## 破坏性改动清单

| # | 改动 | 谁受影响 | 表现 | 可否回避 |
|---|---|---|---|---|
| 1 | account_id / device_id 格式校验 | 手工清空账号字段、或填了空格/中文/特殊符号的既有用户 | 握手被拒 + 中文红横幅指明规则 + 30s 退避重试 | 不可（正是本轮目的）；建议**不提供**逃生开关——开关一旦存在，缺陷就会带着开关长期部署 |
| 2 | TLS 默认校验证书 | 用自签证书 / IP 直连的既有部署 | 升级后连不上，需勾选新设置项 | 不可；靠 B3 的专门错误提示降低误判成本 |
| 3 | `registry.Get` / `Unregister` 签名变更 | 仅 Go 内部（2 个生产调用点） | 编译期 | — |
| 4 | `relay.NewRelayManager` 签名变更 | 仅 Go 内部（5 个调用点，4 个在测试） | 编译期 | — |
| 5 | `relay.RemovePipe` → `RemovePipeForSession` | 仅 Go 内部（1 个生产调用点） | 编译期 | — |
| 6 | `DROP TABLE paired_devices` | 无（表证明为空） | 无 | — |
| 7 | 新增两个 `TRANSFER_FAILURE` 错误码 | 无（已核对客户端不枚举 `error_code`，仅展示） | 无 | — |
| 8 | `routeToPeer` 丢失「跨账号 vs 离线」日志区分 | 运维日志 | 跨账号尝试记作「账号内找不到对端」 | 有意接受，写注释 |

**不破坏**：线路协议（AUTH_REQUEST / AUTH_RESPONSE / 各 payload 的字段集合完全不变）、数据面 URL 参数、持久化设置 JSON 的形状、既有环境变量语义。

---

## 明确不做

**传输历史与磁盘缓存今天是跨账号混在一起的**。`transfer_tasks` / `transfer_items` / `chunk_bitmaps` / `cache_entries` 四张表完全没有 `account_id` 列（`client/src-tauri/src/storage/db.rs:31-88`），`history_repo.rs` 的所有查询也没有任何账号条件——切到账号 B，历史面板会列出账号 A 的文件名、大小、预览摘要，「重新装载」还能把 A 的缓存文件写进剪贴板。

这个泄露比 `paired_devices` 实质得多（那张表是空的，这几张有数据）。不纳入本轮是因为改动面完全不同（四张表加列 + 存量行归属迁移 + 所有查询加条件 + `CacheManager` 按账号分目录），会把这份 diff 撑到没法审。**建议作为下一轮的第一项。**

同样不做：E2EE（Phase 3）、per-account 凭据或 JWT（用户已明确保留单 PSK 模型）、静态加密。

---

## 验证

```bash
# 服务端：全量测试 + 竞态检测（本仓库有 session_race_test.go，-race 是必须的）
cd server && go test ./... && go test -race ./...

# 客户端
cd client/src-tauri && cargo test
cd client && npm test
```

手工端到端（自动化测试覆盖不到「两个真实客户端」的形态）：

1. 起一个服务端，用**两台客户端填相同 account_id** → 互相可见、可传文件（确认没把正常功能修坏）。
2. 第三台客户端填**不同 account_id 但相同 device_id**（手动改本地库的 device_id 制造撞车）→ 断言前两台**不掉线**，且第三台看不到它们。这是 A2 的人工验证。
3. 把 account_id 清空保存 → 界面出现红色横幅并写明格式规则，不是静默重连。这是 A1 的人工验证。
4. `curl http://localhost:8080/metrics | grep account_id` → 应无输出。
5. TLS：把 server_url 指向一个自签证书的 `wss://` 端点 → 默认应连接失败并给出指向设置项的提示；勾选「允许不安全连接」后恢复。这是 B1/B3 的人工验证。

---

## 给审查员的重点提示

请优先核验以下几处判断，它们是本计划的承重点：

1. **A1 的顺序决策**——格式校验排在 `VerifyWithSalt` 之前，代价是校验跑在未鉴权输入上。这个取舍是否成立？「不烧 nonce」的收益是否真的压过暴露面？
2. **A2 与 A3 的结论相反**（registry 改复合键、relay 保留扁平键 + 显式校验）。两处的理由是否各自站得住，还是应当统一？
3. **A3 改动 B**（`RemovePipe` 无任何归属校验）是否如本计划判断的那样是本轮最严重的一条。
4. **A3 改动 C** 声称「任何 `authErr != nil` 都双向拒绝」不会产生新的挂起形态——请找反例。
5. **A4 所述的「pipe 活过 auth 条目」窗口**是否真实可达；复用 `MaxConcurrentTransfers` 而不新开变量是否会在某个取值下产生反直觉行为。
6. **A5 拒绝 per-account label** 的基数论证是否充分；三个聚合 gauge 是否真能回答运维的问题。
7. **B1/B3**：默认开启证书校验是否会让既有部署以难以诊断的方式失败；B3 的错误识别方案是否够。
8. **破坏性清单**是否有遗漏——特别是第 1、2 条之外是否还存在用户可见的行为变化。
