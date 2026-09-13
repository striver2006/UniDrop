---
schema: trivium.review.v1
topic: multi-account-hardening
stage: code
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260913T093116Z
rerun_index: 1
snapshot_hash: sha256:6361d0ddfd22ed48378059c8e6ce6a6b9081cd8b26d38d4f653f82f1f26181cb
blind: true
started_at: 2026-09-13T09:31:17.312Z
finished_at: 2026-09-13T09:40:17.969Z
duration_s: 218
tool_calls:
  read: 41
  grep: 12
  glob: 0
  run_command: 0
  project_reads: 22
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 1
  major: 2
  minor: 1
  nit: 0
degraded: false
parse_fallback: false
peer_deny: false
drift: false
consulted_unverified: true
heterogeneous: true
session_id: 54dc011f-b10f-4e2d-9136-c85c004f8dcf
---

# 代码审查：multi-account-hardening

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260913T093116Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

本次审查重点关注生命周期联动与拆除分支，发现最危险的缺陷（AGY-01）：长传输超过 5 分钟后，虽然 SweepIdlePipes 留住了 auth 条目，但由于 ExpiresAt 未刷新，countAuthorizedForAccountLocked 计数失效，且 AuthorizeSessionIfUnderLimit 允许其他设备使用相同 SessionID 无冲突覆盖该条目，攻击者可借此拆除受害者正在传输的管道。此外，前端 App.tsx 漏接 tls-cert-failed 事件致使证书报错完全静默（AGY-02）；TRANSFER_ANSWER 拒收或畸形时原样转发且客户端缺少 else 处理导致发送端永久挂起（AGY-03）。综合判定为 request-changes。

**结论**：`request-changes`

## 审查意见（共 4 条：阻断 1 ｜ 重要 2 ｜ 次要 1）

### AGY-01 · 阻断（blocker）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `server/internal/relay/relay_manager.go:155` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：长传输超过 5 分钟后在途计数归零，且 AuthorizeSessionIfUnderLimit 允许任意设备无冲突覆盖该 SessionID 并借由 RemovePipeForSession 掐断正在进行的管道。

**依据**：server/internal/relay/relay_manager.go:155 处 AuthorizeSessionIfUnderLimit 仅在 now.Before(existing.ExpiresAt) 时校验所有权冲突；同文件 210 行 countAuthorizedForAccountLocked 仅在 now.Before(auth.ExpiresAt) 时计入在途数；同文件 517-524 行 SweepIdlePipes 虽然保留了活跃 pipe 的 auth 条目，但未更新 ExpiresAt。当传输超过 5 分钟后，任何第三方发送同 session_id 的 answer 会直接跳过冲突检测并覆盖 auth，随后发送 CANCEL 即通过 421-434 行将受害者的活跃 pipe.Close()。

**建议**：在 AuthorizeSessionIfUnderLimit 中检查冲突以及 countAuthorizedForAccountLocked 统计在途时，必须同时检查该 session_id 对应的 RelayPipe 是否存活（例如 now.Before(auth.ExpiresAt) || m.pipes[id] != nil），禁止覆盖活跃管道关联的会话。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src/App.tsx:141` |
| 类别 | contract ｜ 层次 code |
| 置信度 | high |

**问题**：前端 App.tsx 缺失 tls-cert-failed 事件监听，导致 TLS 证书校验失败时界面完全静默无提示，退化为无休止重试。

**依据**：client/src-tauri/src/core/connection_actor.rs:336-345 处通过 app_handle.emit(\"tls-cert-failed\", ...) 发送专门错误；但查看 client/src/App.tsx:123-196，前端仅注册了 devices-updated、auth-failed、auth-success 等监听器，完全没有 listen(\"tls-cert-failed\")；docs/USER_GUIDE.md:68-70 亦说明界面会提示「无法验证服务器证书」。事件无人接收导致设计意图完全失效。

**建议**：在 client/src/App.tsx 的 useEffect 中增加对 tls-cert-failed 事件的 listen 监听，将其作为错误通知或横幅展示给用户，引导其前往设置中检查证书或开启允许不安全连接。

### AGY-03 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `server/internal/controller/control_ws.go:306` |
| 类别 | concurrency ｜ 层次 code |
| 置信度 | high |

**问题**：TRANSFER_ANSWER 在拒收或反序列化失败时仍原样转发，而客户端无拒绝处理分支，导致发送端永久挂起。

**依据**：server/internal/controller/control_ws.go:306 处仅在 err == nil && answer.Accepted && h.relayManager != nil 时处理授权，其他分支（如 answer.Accepted 为 false 或反序列化失败）直接落入第 379 行 routeToPeer 转发；而在 client/src-tauri/src/lib.rs:200-256 处，处理 TRANSFER_ANSWER 仅有 if answer.accepted { if let Some(token) = answer.token { ... } }，缺失 else 分支，导致接收方拒收或报文损坏时，发送方 pending_outbound 永远不释放且卡片停在 TRANSFERRING。

**建议**：在 control_ws.go 中若 answer 反序列化失败应 rejectToSelf 拒绝；若 answer.Accepted 为 false，转发的同时应确保客户端 lib.rs 在 answer.accepted 为 false 时清理 pending_outbound 并将任务卡片置为 FAILED/REJECTED。

### AGY-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `server/internal/controller/control_ws.go:325` |
| 类别 | contract ｜ 层次 code |
| 置信度 | high |

**问题**：control_ws 未校验 TRANSFER_ANSWER 的 session_id，空 session_id 导致授权异常且双向拒绝通知静默失败。

**依据**：server/internal/controller/control_ws.go:325 直接将未校验的 answer.SessionID 传给 relayManager；同文件 422 行 buildFailure 明确指出 if sessionID == \"\" { return nil, false }。若 answer.SessionID 为空，不仅 AuthorizeSessionIfUnderLimit 会存入非法键，且后续的 rejectToSelf 和 rejectToPeer 均因 buildFailure 失败而静默丢弃，两端都收不到错误通知而挂起。

**建议**：在 control_ws.go 解包 TRANSFER_ANSWER 后，先调用 auth.ValidateSessionID(answer.SessionID) 校验其非空与字符合法性，若非法则立即向当前连接返回错误并终止处理。

## 认为正确的部分

- DeviceRegistry 改用 (AccountID, DeviceID) 复合键，彻底解决了同名设备跨账号被顶下线的缺陷，同时完整保留了同账号同设备重连顶号的合理逻辑。
- 握手流程中严格将账户/设备标识符格式校验置于 HMAC 验签之前，有效避免了非法输入消耗防重放 nonce，防止重试错误漂移为重放攻击误报。
- /metrics 与 /healthz 端点严禁打入 account_id 标签，改为输出 max_devices_per_account 与 max_pipes_per_account 等单值聚合指标，有效规避了客户端随意伪造账号造成的 Prometheus 序列基数爆炸风险。
- TLS 策略默认启用 webpki 证书验证，并将 InsecureServerCertVerifier 显式收敛到 allow_insecure_tls 配置项中，控制面与数据面策略一致，堵住了默认无条件放行中间人攻击的漏洞。
- 设置面板与后端双重校验 account_id，统一使用 Chinese 规则提示，输入框去掉原生 HTML required 属性改用自定义校验，避免了空格绕过与英文原生弹泡样式割裂问题。

## 未覆盖范围（本侧盲区）

- STUN 服务（RFC 8489）的绑定、NAT 类型嗅探及 UDP 打洞链路。
- 平台特定剪贴板装载底层系统 FFI 实现（macOS pasteboard / Windows CF_HDROP / Linux uri-list）。
- 历史记录修剪逻辑（history_pruner）在高并发批量传输终态写入时的 SQLite WAL 锁竞争表现。

## 实际查阅的项目文件

- `server/internal/relay/relay_manager.go`
- `server/internal/relay/pipe.go`
- `server/internal/relay/relay_test.go`
- `server/internal/controller/control_ws.go`
- `server/internal/controller/data_ws.go`
- `server/internal/controller/health.go`
- `server/internal/controller/cross_account_ws_test.go`
- `server/internal/controller/identity_ws_test.go`
- `server/internal/controller/limits_ws_test.go`
- `server/internal/auth/identity.go`
- `server/internal/auth/verifier.go`
- `server/internal/registry/registry.go`
- `server/internal/limits/limits.go`
- `server/internal/protocol/envelope.go`
- `server/cmd/unidrop-server/main.go`
- `server/configs/.env.example`
- `server/internal/e2e_test.go`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/commands/clipboard_cmd.rs`
- `client/src-tauri/src/core/connection_actor.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src/components/SettingsModal.tsx`
- `client/src/components/SettingsModal.test.tsx`
- `client/src/App.tsx`
- `docs/USER_GUIDE.md`
- `docs/design/DESIGN.md`
- `docs/需求.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":41,"grep":12,"glob":0,"run_command":0,"project_reads":22}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
