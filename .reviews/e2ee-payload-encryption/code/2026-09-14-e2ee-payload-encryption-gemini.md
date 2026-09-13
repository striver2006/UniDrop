---
schema: trivium.review.v1
topic: e2ee-payload-encryption
stage: code
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260913T163647Z
rerun_index: 1
snapshot_hash: sha256:99b32d20ca51a8139030763c0f96f157e76c439177cbe45dce0f96675e0b6f6c
blind: true
started_at: 2026-09-13T16:36:49.445Z
finished_at: 2026-09-13T16:44:37.923Z
duration_s: 193
tool_calls:
  read: 32
  grep: 16
  glob: 0
  run_command: 0
  project_reads: 17
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 1
  major: 1
  minor: 1
  nit: 0
degraded: false
parse_fallback: false
peer_deny: false
drift: false
consulted_unverified: true
heterogeneous: true
session_id: a7093e5e-5c3e-4fce-8a50-42f27d40e7df
---

# 代码审查：e2ee-payload-encryption

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260913T163647Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

审查焦点全部核实通过：三条成帧路径均严格加密、四处块长同源无遗漏、协商前置有效绕开 ANSWER 重建、AAD 取空论证成立。
但发现两处阻碍功能的关键缺陷：
1. 最危险的是 Cargo.toml 仍为 0.2.0，导致 peer_supports_e2ee 在真实运行时恒判为不支持，所有传输均无条件回落明文；
2. 发送端密封元数据时就地抹空了文件名与摘要，导致发送端本地历史与进度 UI 全部退化为空白。
请修复后重新提审。

**结论**：`request-changes`

## 审查意见（共 3 条：阻断 1 ｜ 重要 1 ｜ 次要 1）

### AGY-01 · 阻断（blocker）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/Cargo.toml:3` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：客户端 Cargo.toml 版本仍为 0.2.0，未同步升至 0.3.0，导致运行时对端能力门槛判定恒为不支持，所有传输均无条件回落明文。

**依据**：client/src-tauri/Cargo.toml:3 声明 version = 0.2.0，client/src-tauri/src/app_state.rs:17 通过 env!(CARGO_PKG_VERSION) 取版本；而 client/src-tauri/src/core/e2ee.rs:286 中 E2EE_MIN_VERSION_TUPLE 为 (0, 3, 0)，且 e2ee.rs:459 单元测试明确断言 0.2.0 被判为不支持。在 clipboard_cmd.rs:315 的 decide_e2ee 中，对端版本判定 peer_supports_e2ee 恒为 false，直接进入回落分支。

**建议**：将 client/src-tauri/Cargo.toml 及相关包版本升级至 0.3.0，使其与 e2ee.rs 中设定的最低 E2EE 支持版本对齐。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/commands/clipboard_cmd.rs:256` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：发送端构造加密 OFFER 时原地抹空了元数据字段，导致发送端本地数据库（transfer_tasks / transfer_items）持久化为空文件名与空摘要，传输进度与历史记录面板严重失真。

**依据**：client/src-tauri/src/core/e2ee.rs:199,205,206 中 seal_offer_metadata 直接 std::mem::take 清空了 offer.preview_summary、relative_path 与 sha256；返回后的 offer 随后在 clipboard_cmd.rs:256,265 被写入 pending_outbound 与 HistoryRepo::record_task；在 transfer_engine.rs:407 中，发送端 start_sender_task 上报 transfer-progress 时 offer.preview_summary 亦为空。lib.rs:209-214 的注释虽明确指出接收端必须先解密再落库以防展示空文件名，但发送端却直接落库了抹空后的对象。

**建议**：在 dispatch_offer 或 prepare_offer 中保留未抹空的元数据副本，仅将抹空并密封后的载荷发往中继，本地存储与事件派发继续使用原始元数据。

### AGY-03 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/transfer_engine.rs:1031` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：接收端触发 AEAD 熔断退出循环后，兜底逻辑再次发送通用的 RECEIVER_DISCONNECTED 错误帧并覆写本地历史，冲掉了原本指引用户检查 PSK 的精准报错。

**依据**：client/src-tauri/src/core/transfer_engine.rs:611-628 在 AEAD 熔断触发时发出包含 E2EE_AUTH_FAILED 及文案「内容校验失败：两端密钥可能不一致，或数据在传输中被篡改」的 failure envelope 并 break；循环退出后进入 1017 行 !fully_completed，1031 行调用 finalize_history_status 填入「接收连接中断或校验失败」，并在 1033-1045 行无条件向发送端发送第二条 RECEIVER_DISCONNECTED 错误信令。lib.rs:484 收到后覆写了先前的错误信息。

**建议**：在接收循环中记录主动熔断状态；若已发送针对性的 E2EE_AUTH_FAILED 失败信令，退出后不要再发送次生 RECEIVER_DISCONNECTED 覆盖，并向 finalize_history_status 传入精确原因。

## 认为正确的部分

- 三条成帧路径（首发、NACK 快重传、超时重传）均统一经由 build_data_frame 成帧，在开启 E2EE 时严格执行 AEAD 加密，且 CRC32 正确计算在密文上以防指纹泄露。
- 四处块长（prepare_offer、prepare_offer_from_bytes、发送端分块切片、接收端写盘 offset）严格同源于 plaintext_chunk_len(encrypted)，密文满块留出 16 字节预留，恰好贴合中继 4MB 限制；接收端进度累加亦严格按明文长度计。
- 能力协商前置到发 OFFER 前读取在线设备表 app_version，有效避开了服务端对 TRANSFER_ANSWER 反序列化/再序列化时未知字段被静默丢弃的问题，实现服务端零改动。
- AAD 取空的论证充分站得住脚：item_index 与 chunk_index 已作为 Nonce 参与认证，session_id 作为 HKDF salt 保证跨会话隔离，元数据与数据通过 Nonce 域分隔隔离，上层整文件 SHA256 兜底完整性。
- AEAD 失败熔断机制设计了单块 3 次与会话累计 3 次的双层计数，有效防止滑动窗口并发失败导致无限 NACK 重传。

## 未覆盖范围（本侧盲区）

- 真实网络高延迟与丢包抖动环境下的选择性重传与 AEAD 解密耗时基准测试
- STUN 穿透与非标 WSS 中继在超大文件（>10GB）连续加解密时的内存与 GC 压力表现
- 老版本 SQLite 数据库中配置数据存在非法或缺失字段时的极限边界迁移测试

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/Cargo.lock`
- `client/src-tauri/src/app_state.rs`
- `client/src-tauri/src/commands/clipboard_cmd.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/core/e2ee.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/protocol/binary_header.rs`
- `client/src-tauri/src/protocol/envelope.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src/App.test.tsx`
- `client/src/App.tsx`
- `client/src/components/SettingsModal.test.tsx`
- `client/src/components/SettingsModal.tsx`
- `server/internal/controller/control_ws.go`
- `server/internal/protocol/binary_header.go`
- `server/internal/protocol/envelope.go`

> 编排器从工具轨迹中记录到的读取次数：{"read":32,"grep":16,"glob":0,"run_command":0,"project_reads":17}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
