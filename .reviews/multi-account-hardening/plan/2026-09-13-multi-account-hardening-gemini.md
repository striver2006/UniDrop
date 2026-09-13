---
schema: trivium.review.v1
topic: multi-account-hardening
stage: plan
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260913T082750Z
rerun_index: 1
snapshot_hash: sha256:77dc967f74398493f2ae3dc15cd566c4356b8d4835d1ebd9b908744bc106d479
blind: true
started_at: 2026-09-13T08:27:51.974Z
finished_at: 2026-09-13T08:41:35.027Z
duration_s: 244
tool_calls:
  read: 35
  grep: 12
  glob: 0
  run_command: 0
  project_reads: 29
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 3
  minor: 2
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: 5316df1b-a13a-4b54-ac52-2761fee9adce
---

# 计划审查：multi-account-hardening

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260913T082750Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

本计划整体架构设计扎实，给审查员的重点提示中涉及的顺序决策、键设计取舍与基数防护均成立。
最关键问题在于 §A3/§A4 对会话与管道生命周期不对齐的处理存在缺陷：长传输在 5 分钟后 authSessions 被清理，导致 RemovePipeForSession 误判拒绝正常管道拆除，且引发控制面与数据面在途计数脱节。
此外，§B3 证书错误通知忽略了 ConnectionActor 缺乏 AppHandle 的问题，§B1 手动构造 ClientConfig 存在 Cargo 直接依赖缺失。
建议修正 Relay 生命周期联动与客户端通知链路后再行实施。

**结论**：`request-changes`

## 审查意见（共 6 条：重要 3 ｜ 次要 2 ｜ 吹毛求疵 1）

### AGY-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §A3 改动 B` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：RemovePipeForSession 在「无 auth 条目」时直接拒绝，与 §A4 中 pipe 可活过 auth 条目 5 分钟 TTL 的现实相冲突，导致耗时超过 5 分钟的正常长传输在完成时无法拆除管道并产生误报警。

**依据**：查阅 .reviews/multi-account-hardening/plan/2026-09-13-multi-account-hardening-claude.md §A3 改动 B、§A4，对比 server/internal/relay/relay_manager.go:312-317（SweepIdlePipes 仅按 ExpiresAt 删除 authSessions）与 server/internal/relay/pipe.go。当传输大文件持续超过 5 分钟时，authSessions 已被 purge，而活跃的 RelayPipe 仍在传输。若此时传输完成或取消，调用 RemovePipeForSession 会因「无 auth 条目」被拒绝并打出 cross-account teardown blocked 虚假告警，导致已完成 pipe 无法主动清理。

**建议**：在 RelayManager.SweepIdlePipes 中清理过期 authSessions 时，若对应 sessionID 的 RelayPipe 依然活跃则不予清除；或者在 RemovePipeForSession 中，当 authSessions 已被清理但 RelayPipe 存在时，使用 RelayPipe 上的 AccountID/FromDevice/ToDevice 字段作为第二道合法性验证，避免对长传输误报拦截。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §A4` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：per-account pipe 上限仅在 ValidateAndGetOrCreatePipe 拦截，未联动 countAuthorizedForAccountLocked，导致长传输场景下控制面发空头支票、数据面硬性失败。

**依据**：查阅 server/internal/relay/relay_manager.go:94-154、server/internal/controller/control_ws.go:307-337 与 server/internal/controller/data_ws.go:67-72。countAuthorizedForAccountLocked 仅统计 authSessions，当长传输进行到 5 分钟后 authSessions 过期，控制面 ANSWER 协商时误以为在途名额充足并签发 Token；但随后客户端连接 /ws/data 时，ValidateAndGetOrCreatePipe 检查 active pipes 发现账号超限而直接 StatusPolicyViolation 断开，此时客户端已无法得到可读的限额提示。

**建议**：使 countAuthorizedForAccountLocked 在计数时将「有效 authSessions」与「当前活跃 RelayPipe」合并去重统计（或确保 pipe 活跃期间 authSessions 不失效），使并发超限拦截统一发生在控制面 TRANSFER_ANSWER 授权阶段，从而向两端回送清晰的限制文案，而不是在数据面建连时静默断开。

### AGY-03 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §B3` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | high |

**问题**：§B3 提出在 connection_actor 的连接失败分支识别 rustls 证书错误并 emit 界面提示，但 ConnectionActor 结构体本身未持有 AppHandle，缺乏向前端派发事件的通道。

**依据**：查阅 client/src-tauri/src/core/connection_actor.rs:76-96（ConnectionActor 仅有 outgoing_rx 与 incoming_tx，无 AppHandle）、client/src-tauri/src/lib.rs:81-89, 334（AppHandle 仅在 lib.rs 中持有）以及 client/src/App.tsx:140-144, 324-332（前端仅监听 auth-failed 事件）。底层握手发生 TLS 错误时尚未建立 WebSocket 连接，无法构造 ControlEnvelope 发给 incoming_tx。

**建议**：补充底层连接错误上报前端的通道设计：可在构造 ConnectionActor 时注入 AppHandle，或新增错误通道传递给 lib.rs 转发；前端方面建议直接复用既有的 auth-failed 事件派发带有引导用户开启「允许不安全连接」提示的中文文本，以复用 App.tsx 现成且唯一的顶部红框横幅。

### AGY-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §B1` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | high |

**问题**：计划称「无需新增依赖」，但若在 connection_actor.rs 中直接引用 webpki_roots 模块构建 ClientConfig，会导致 Cargo 因缺少直接依赖声明而编译失败。

**依据**：查阅 client/src-tauri/Cargo.toml:31（仅 tokio-tungstenite 开启了 rustls-tls-webpki-roots feature）和 client/src-tauri/Cargo.lock:5340。在 Rust 2021 Edition 中，未在 Cargo.toml 的 dependencies 中声明的传递依赖无法在 connection_actor.rs 中通过 use webpki_roots::... 直接引用。

**建议**：在 Cargo.toml 的 [dependencies] 中显式添加 webpki-roots 依赖；或者在 connection_actor 中对于默认安全连接向 connect_async_tls_with_config 的 connector 参数传入 None，由 tokio-tungstenite 内部默认加载其自带的 webpki 根证书。

### AGY-05 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §文档同步清单` |
| 类别 | contract ｜ 层次 requirement |
| 置信度 | medium |

**问题**：文档同步清单遗漏了项目需求跟踪的核心文件 docs/需求.md 中第 6 条（多账号支持）与第 7 条（全程加密/TLS 硬化）的状态同步。

**依据**：查阅 docs/需求.md:70-98 与 .reviews/multi-account-hardening/plan/2026-09-13-multi-account-hardening-claude.md §文档同步清单。需求文档中第 4、5、8、9 项均有详尽的落地日期、取舍和边界记录，而本轮所涉及的第 6、7 项在同步清单中缺失。

**建议**：将 docs/需求.md 补充进文档同步清单，明确记录第 6 条（多账号便利隔离硬化完成、保持单 PSK 模型）与第 7 条（传输层 TLS 证书默认校验完成、E2EE 留待 Phase 3）的实现结论与范围界定。

### AGY-06 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §破坏性改动清单 项 4` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | high |

**问题**：破坏性清单中对 relay.NewRelayManager 签名变更的影响范围统计低估（列为 5 个调用点），实际测试集中共有 10 处调用需要同步调整。

**依据**：检索 server/cmd/unidrop-server/main.go:46、server/internal/relay/relay_test.go（6 处）、server/internal/controller/control_ws_test.go（2 处）、server/internal/controller/limits_ws_test.go:43 与 server/internal/e2e_test.go:37。NewRelayManager 实际共有 1 处生产与 10 处测试调用。

**建议**：更新影响范围计数，确保测试套件中 10 处 NewRelayManager 调用点在实施阶段全部按新签名构造，避免测试构建中断。

## 认为正确的部分

- §A1 中将账号与设备格式校验排在 VerifyWithSalt 验签之前是完全成立的防御性决策：字符集校验为零分配且有界（<=64 字节），相比 HMAC 计算极其轻量，能阻断畸形长字符串进入密码学流程，且符合语法错误不应消耗重放防线（burn nonce）的协议规范。
- §A2（Registry 改复合键）与 §A3（Relay 保留扁平键加归属核验）的相反设计理由各自充分：DeviceID 是账号作用域内的标识，跨账号撞名属常态，不分区会导致顶号与路由混乱；SessionID 是每次传输全局生成的 UUIDv4，撞车属异常入侵或复用，且数据面 URL 不含 account_id，扁平键能自然复用数据面 Token 校验逻辑并暴露异常碰撞。
- §A3 改动 B 的判断非常准确：现状中 RemovePipe 完全无鉴权和账号核验，任何客户端仅凭 session_id 即可拆除正在传输的数据管道，属于可被任意账号滥用阻断传输的严重漏洞，必须收口。
- §A3 改动 C 对任何 authErr 均双向回送 TRANSFER_FAILURE 的决策切断了既有代码中无 Token 仍转发导致发送端永久挂起的缺陷通道，且不会误伤 receiver 主动拒绝（accepted=false）的正常协商路径。
- §A5 坚决拒绝 per-account label 的基数论证非常专业：客户端自报且未鉴权的 account_id 属于无限基数，放进 Prometheus label 会直接引发时序爆炸与内存拒绝服务攻击；所设计的在线账号数与单账号峰值三条聚合 Gauge 恰到好处地满足了容量排查需求。
- §A6 彻底删除从未使用、无读无写且制造账号分区假象的 paired_devices 表，清理了历史技术债务并降低了后续维护人员误判风险。

## 未覆盖范围（本侧盲区）

- 未在真实运行环境中测试自签证书及各种非标准 CA 证书在 Rustls 下的实际校验表现（受审查员只读与无执行权限约束）。
- 未展开评估未来 Phase 3 端到端加密（E2EE）密钥协商协议的具体细节（已确认为后续独立主题，不在本轮范畴）。
- 未评审客户端本地 SQLite 数据库（transfer_tasks 等四张表）跨账号分区的迁移方案（本计划已在「明确不做」中显式排除并建议下轮开展）。

## 实际查阅的项目文件

- `server/internal/auth/verifier.go`
- `server/internal/auth/nonce_cache.go`
- `server/internal/controller/control_ws.go`
- `server/internal/controller/data_ws.go`
- `server/internal/controller/health.go`
- `server/internal/controller/limits_ws_test.go`
- `server/internal/registry/registry.go`
- `server/internal/registry/session.go`
- `server/internal/registry/registry_test.go`
- `server/internal/relay/relay_manager.go`
- `server/internal/relay/pipe.go`
- `server/internal/relay/relay_test.go`
- `server/internal/limits/limits.go`
- `server/internal/protocol/envelope.go`
- `server/internal/config/config.go`
- `server/internal/e2e_test.go`
- `server/cmd/unidrop-server/main.go`
- `server/configs/.env.example`
- `client/src-tauri/src/core/connection_actor.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/Cargo.lock`
- `client/src/components/SettingsModal.tsx`
- `client/src/App.tsx`
- `docs/design/DESIGN.md`
- `docs/USER_GUIDE.md`
- `docs/需求.md`
- `.reviews/multi-account-hardening/plan/2026-09-13-multi-account-hardening-claude.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":35,"grep":12,"glob":0,"run_command":0,"project_reads":29}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
