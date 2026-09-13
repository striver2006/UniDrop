---
schema: trivium.review.v1
topic: e2ee-payload-encryption
stage: code
role: glm
vendor: zcode
agent_cli: zcode/0.16.5
model_requested: GLM-5.3
model_effective: bigmodel-coding-plan/GLM-5.3
model_effective_source: trace
model_route: config_copy
run_id: 20260913T163647Z
rerun_index: 1
snapshot_hash: sha256:99b32d20ca51a8139030763c0f96f157e76c439177cbe45dce0f96675e0b6f6c
blind: true
started_at: 2026-09-13T16:36:49.445Z
finished_at: 2026-09-13T16:44:37.923Z
duration_s: 468
tool_calls:
  read: 16
  grep: 7
  glob: 0
  run_command: 0
  project_reads: 13
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 1
  minor: 1
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
consulted_unverified: true
heterogeneous: true
session_id: sess_876f68c7-625f-4551-8ce6-bb3f2fece7be
---

# 代码审查：e2ee-payload-encryption

> Critic-B · 智谱 ZCode ｜ 模型 `bigmodel-coding-plan/GLM-5.3`
> ｜ 运行 `20260913T163647Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

四个焦点中三个核实成立：三条成帧路径全部经 build_data_frame 加密、块长四处同源无遗漏、AAD 取空论证站得住（item/chunk 经 nonce 认证，total_chunks 接收端不消费，sha256 兜底）。最危险的一条在协商链路：门槛定为 0.3.0，而 crate 版本仍是 0.2.0、上报的 app_version 取自 CARGO_PKG_VERSION——当前树上构建的两台新客户端会永远协商回落明文，且每次发送都弹「未加密」toast，功能在 HEAD 上整体不可用；无任何测试把门槛与实际版本绑定。另有两条小问题：parse_semver 把 0.3.0-beta.1 这类带点预发布版本判为不支持；cmd_send_clipboard 在读剪贴板之前就发回落提示。

**结论**：`request-changes`

## 审查意见（共 3 条：重要 1 ｜ 次要 1 ｜ 吹毛求疵 1）

### GLM-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/e2ee.rs:285（E2EE_MIN_VERSION_TUPLE）与 client/src-tauri/Cargo.toml:3` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：协商门槛是 0.3.0，但本 crate 版本是 0.2.0，而客户端上报给服务端的 app_version 恰恰取自 CARGO_PKG_VERSION。端到端链路（connection_actor 发 auth → registry 存储 → DEVICE_LIST_SYNC 回发 → decide_e2ee 读 online_devices）意味着：两个从当前 HEAD 构建的全新客户端互相看到的 app_version 都是 "0.2.0"，peer_supports_e2ee 判为不支持，E2EE 永远不会激活——默认开启的加密功能在合并点上整体失效，且每次传输都向用户弹出「本次传输未加密：未能确认对端支持端到端加密」的警告。peer_capability_gate 测试甚至把 "0.2.0" 钉死为不支持，等于把这一矛盾写进了测试而不被发现。注释说门槛是「发布本功能的版本号」，显然指望发版时再升 0.3.0，但仓库里没有任何机制（测试或断言）把 E2EE_MIN_VERSION_TUPLE 与实际包版本绑在一起，漏升版本就静默失效。

**依据**：client/src-tauri/src/core/e2ee.rs:285（(0,3,0) 门槛）与 :459（测试断言 0.2.0 不支持）；client/src-tauri/Cargo.toml:3（version = "0.2.0"）；client/src-tauri/src/app_state.rs:17（APP_VERSION = env!("CARGO_PKG_VERSION")）；client/src-tauri/src/lib.rs:116（ConnectionConfig 带该版本）；client/src-tauri/src/commands/clipboard_cmd.rs:310-318（decide_e2ee 从 online_devices 读对端 app_version）。

**建议**：在 e2ee.rs 测试中加一条守卫：assert!(peer_supports_e2ee(Some(crate::app_state::APP_VERSION)))——它今天就会红，强制「升版本到 0.3.0」与「合并本功能」在同一个变更里完成；或者在本变更内直接把 crate 版本升到 0.3.0。二选一即可，但必须让门槛与实际版本无法再各自漂移。

### GLM-02 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/e2ee.rs:289-305（parse_semver）` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | medium |

**问题**：parse_semver 按点分段后，若 patch 段带「含点的预发布后缀」（如 cargo semver 合法的 "0.3.0-beta.1"、"0.3.0-rc.1"），会把第四段当成多余段返回 None，peer_supports_e2ee 随之判为不支持。也就是说如果本功能以带点预发布版本打出测试包，两台这样的设备之间 E2EE 会被静默关掉（回落明文 + toast），恰好在最需要验证功能的阶段功能不可用。现有测试只覆盖了不带点的 "0.3.0-beta"。

**依据**：client/src-tauri/src/core/e2ee.rs:294-303：patch_raw 之后的 it.next().is_some() 分支把 "0.3.0-beta.1"（split('.') 得四段）判 None；测试 peer_capability_gate（:467）只断言了 "0.3.0-beta" 支持。

**建议**：先把版本串在第一个 '-' 处截断再按点分段（预发布整体丢弃，只比较数字三元组），并补一条 "0.3.0-beta.1" 的测试断言；或者明确约定本项目永不使用带点预发布版本号并写进注释。

### GLM-03 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/commands/clipboard_cmd.rs:399-405（cmd_send_clipboard）` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：cmd_send_clipboard 里 decide_e2ee 与 emit_fallback 发生在 spawn_blocking 读剪贴板之前：当剪贴板为空（或预检失败）导致命令最终返回 Err 时，若恰好同时处于回落场景，用户已经先收到一条「本次传输未加密」的 toast——针对一次根本没有发生的传输。cmd_send_files 没有这个问题（precheck 在 decide_e2ee 之前就返回了）。

**依据**：补丁中 cmd_send_clipboard 的顺序：decide_e2ee → emit_fallback → spawn_blocking 内 read_clipboard() 匹配 Empty 返回 Err；对比 cmd_send_files 中 precheck_file_paths 的 .await?? 位于 decide_e2ee 之前。

**建议**：把 emit_fallback 移到 spawn_blocking 成功返回之后再发，或让 prepare 阶段的 Err 路径吞掉未使用的 fallback_reason；两处保持同一种顺序即可。

## 认为正确的部分

- 三条成帧路径（首发 transfer_engine.rs:438、NACK 快重传 :498、超时重传 :524）全部经 build_data_frame 并传入 e2ee_key；BinaryHeader::new_data 在生产代码里只剩 build_data_frame 一个调用点，不存在漏加密的第四条路径。
- 四处块长同源成立：prepare_offer、prepare_offer_from_bytes、发送端切块、write_payload_chunk 全走 plaintext_chunk_len；MAX_PAYLOAD_LENGTH 在生产代码只剩 e2ee.rs 与帧头解码校验（binary_header.rs:170）两处合法引用；两端进度均按明文口径累计。等号断言（full_encrypted_frame_fits_the_relay_limit、total_chunks_follows_the_encrypted_flag、encrypted_multichunk_roundtrip_is_byte_identical）都有真实的失败条件，不是永真测试。
- 协商前置的论证与服务端代码吻合：OFFER 在 control_ws.go:278-298 仅解到局部变量做限额检查，routeToPeer 转发的是原始 payload（encrypted_metadata/e2ee_version 得以存活）；ANSWER 在授权成功后于 :388 经 json.Marshal(answer) 重建、未知字段确实被丢弃；拒收（accepted=false）的 ANSWER 不走重建分支，reject_reason 能完整到达发送端，且 lib.rs:384-419 有配套的 pending 清理与失败展示，不会静默挂起。
- app_version 协商链路真实存在：auth 上报 → registry/session.go:88 ToOnlineDevice → DEVICE_LIST_SYNC/DEVICE_ONLINE（control_ws.go:188-216）→ lib.rs:182-197 填充 online_devices，服务端零改动成立。
- AAD 取空的论证站得住：item_index/chunk_index 通过接收端自行重算的 nonce 参与认证（transfer_engine.rs:751），帧头 total_chunks 接收端不消费（完成判定用 OFFER 的 items[].total_chunks，:868-870），内容完整性由加密保护的 sha256 在收尾兜底（:878-896）；跨位置/跨会话的密文拼接会因 nonce 不匹配 tag 失败；verify_crc32 强制 payload_len 与实际长度一致（binary_header.rs:190-193），PayloadLen 篡改进不了解密。
- CRC32 算在密文上、nonce 域分隔（metadata_nonce 与全部 data_nonce 不相交）、双层熔断（会话级阈值针对 4 在途块并发 tag 失败的场景）三个决定均正确且有针对性测试；熔断文案先指向 PSK 不一致而非篡改，符合实际故障分布。
- e2ee_enabled 用自定义 serde default=true 的方向正确（安全值与 bool::default() 相反时必须显式写）；加密 OFFER 先解密再落库/展示、失败就地拒收的顺序正确；文档（USER_GUIDE、DESIGN §6.0、需求.md）如实记录了机会性加密的边界与服务器可见元数据清单。

## 未覆盖范围（本侧盲区）

- 未执行任何构建或测试（cargo test / go test / vitest），全部结论基于静态阅读；测试本身是否通过未验证。
- 服务端数据面（relay 管道、binary_header.go 的逐行实现）未完整阅读，4MB 满块不断连的结论依赖客户端测试与代码注释中对 ErrPayloadTooLarge 行为的引用。
- Cargo.lock 中 ring 与 rustls 解析版本是否为同一份 0.17.x 未逐条核对（只确认了 Cargo.toml 的注释声明与 lock 中出现 ring/base64）。
- 前端只读了 diff 段（App.tsx / SettingsModal.tsx / types / 两个测试文件），未读完整文件，也未验证 listenerCount 辅助函数的实现。
- settings_cmd.rs、cache_manager.rs、history_repo.rs 仅通过 diff 与 grep 交叉核对，未整读；settings 保存/加载路径对 e2ee_enabled 的往返未逐行验证。
- 恶意服务端主动攻击面（改写 app_version 降级、丢弃 OFFER 条目、重放信令）按文档声明的威胁模型排除，未做进一步攻击路径推演；断点续传场景的 nonce 复用只审了注释里的告诫。

## 实际查阅的项目文件

- `.reviews/e2ee-payload-encryption/code/_meta/changes-20260913T163647Z.diff`
- `client/src-tauri/src/core/e2ee.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/app_state.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/protocol/binary_header.rs`
- `client/src-tauri/src/commands/clipboard_cmd.rs`
- `client/src-tauri/Cargo.toml`
- `server/internal/controller/control_ws.go`
- `server/internal/limits/limits.go`
- `server/internal/registry/session.go`

> 编排器从工具轨迹中记录到的读取次数：{"read":16,"grep":7,"glob":0,"run_command":0,"project_reads":13}

---

*本文档由 TriviumCode 编排器从 `glm` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
