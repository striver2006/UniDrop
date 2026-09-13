---
schema: trivium.review.v1
topic: server-transfer-limits
stage: plan
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260913T033609Z
rerun_index: 1
snapshot_hash: sha256:f9acfb53b88e9bbe17d3151f390a7588bc75d216cdfece8f9307f5bfc3d7c34d
blind: true
started_at: 2026-09-13T03:36:10.742Z
finished_at: 2026-09-13T03:44:10.356Z
duration_s: 152
tool_calls:
  read: 27
  grep: 2
  glob: 0
  run_command: 0
  project_reads: 16
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
session_id: b69f7d2a-88c7-406c-abf1-1edde5a21f1b
---

# 计划审查：server-transfer-limits

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260913T033609Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

最危险的问题在于：Plan §4.2 虽规定环境变量设为 0 代表不限制，但随后要求“照抄 err == nil && val > 0”，导致部署者显式配置 0 时被当成无效值忽略并静默维持默认上限，且缺少 int64 溢出保护。
其次，剪贴板与单文件限额“同时适用且取更严”导致大图配置失效并报“文件太大”；预检未安排在 SHA-256 计算之前进行 Fast-Fail，导致超限大文件仍需白白算完哈希；客户端在 TRANSFER_FAILURE 路径未清理 pending_outbound 存在内存泄漏。
整体方案在兼顾旧版本兼容与常量解耦方面表现优秀，但上述核心逻辑与时序缺陷需修改后再实施。

**结论**：`request-changes`

## 审查意见（共 6 条：重要 3 ｜ 次要 2 ｜ 吹毛求疵 1）

### AGY-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4.2` |
| 类别 | correctness ｜ 层次 plan |
| 置信度 | high |

**问题**：照抄 val > 0 会导致环境变量显式设为 0（不限制）时被静默判定为无效并退回默认值，且现有代码缺失 warn 日志输出与 int64 溢出保护。

**依据**：Plan §4.2 明确规定“env 显式设为 0 = 不限制该项”，但随后声称“现有 config.go:38-41 的写法（if err == nil && val > 0）碰巧已经是对的形态，新增五项必须照抄它”。查阅 server/internal/config/config.go:39，条件 val > 0 会在用户配置 0 时为 false，导致配置直接被忽略并维持默认值；且该处 err != nil 时无任何日志。此外单文件/总传输上限为字节数，在 32 位环境或配置超 2GB 时用 strconv.Atoi 存在溢出截断风险。

**建议**：env 解析逻辑中，对传输限额字段不能使用 val > 0，应明确允许 val >= 0；当 err != nil || val < 0 时打印 slog.Warn 并保留默认值；涉及字节数的字段应使用 int64 并通过 strconv.ParseInt(s, 10, 64) 解析。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4.4` |
| 类别 | contract ｜ 层次 plan |
| 置信度 | high |

**问题**：剪贴板限额与单文件限额“同时适用且取更严”破坏了配置的独立性，导致部署者调大剪贴板限额静默失效，且引发错误码与提示文案类型混淆。

**依据**：Plan §4.4 规定“剪贴板限额与文件限额同时适用，取更严的那条……部署者把图片上限调到 512 MB 时，单文件 128 MB 必须仍然拦得住”。查阅 client/src-tauri/src/core/transfer_engine.rs:216-220，剪贴板图片在 offer 中作为唯一的 item 封装。若部署者配置图片上限 512MB、单文件 128MB，发 200MB 图片会被单文件限额拦截，返回 LIMIT_FILE_TOO_LARGE，客户端弹出的提示是“文件太大”而非需求 3 规定的“剪贴板内容太大”，导致部署者的图片配置完全被单文件限额压制失效。

**建议**：厘清正交分层关系：单次传输总字节上限（max_total_transfer_bytes）与总条目数适用于所有传输；但内容维度应按 data_type 正交互斥——FILES 校验 max_single_file_bytes，IMAGE 校验 max_clipboard_image_bytes，TEXT 校验 max_clipboard_text_bytes。

### AGY-03 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4.5` |
| 类别 | performance ｜ 层次 plan |
| 置信度 | high |

**问题**：文件传输预检仅放在 prepare_offer 之后，导致超出上限的大文件或海量条目仍会执行耗时长达数分钟的全量磁盘读与 SHA-256 计算，违背预检即时反馈初衷。

**依据**：Plan §1.5 指出“选 100GB 文件也会照常算完 SHA-256”的缺陷，但在 §4.5 和 §7.2 中仍将预检时序定在“prepare_offer 之后、dispatch_offer 之前”。查阅 client/src-tauri/src/core/transfer_engine.rs:94-125，prepare_offer 内部在读取全部内容做 SHA-256。若预检在其后执行，100GB 大文件或海量文件在超限被拒前仍需先花费漫长时间做全量哈希，导致界面无响应且浪费严重。

**建议**：在 prepare_offer 计算哈希前（或在其元数据遍历时）增加 Fast-Fail 预检：优先基于 paths.len() 与快速获取的 fs::metadata(p).len() 校验条目数、单文件大小与总容量，超限立即提前返回 Err，避免昂贵的 SHA-256 计算。

### AGY-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4.4` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | high |

**问题**：服务端回送 TRANSFER_FAILURE 路径下，客户端未从 pending_outbound 移除该会话，造成内存常驻泄漏。

**依据**：Plan §4.4 声称客户端 lib.rs:255-302 的 TRANSFER_FAILURE 分支“无需改动即可展示”。但查阅 client/src-tauri/src/lib.rs:255-302 与 203-205，pending_outbound 仅在 TRANSFER_ANSWER 且 accepted == true 时才移除。当服务端兜底拒绝并回送 TRANSFER_FAILURE 时，发送端的 pending_outbound 永远不会清理该条目及其关联的 payload/paths，导致会话内存泄漏。

**建议**：在计划中明确：客户端 lib.rs 的 TRANSFER_FAILURE 分支在处理失败时，必须同步调用 pending_outbound.lock().await.remove(&sid) 释放待发送队列中的条目。

### AGY-05 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §4.4` |
| 类别 | contract ｜ 层次 plan |
| 置信度 | medium |

**问题**：TRANSFER_OFFER 反序列化失败时回送 TRANSFER_FAILURE 缺失 session_id 提取与容错契约。

**依据**：Plan §4.4 第 1 点要求 offer 解析失败时回 TRANSFER_FAILURE。查阅 server/internal/protocol/envelope.go:133 与 client/src-tauri/src/lib.rs:258-261，客户端完全依赖 TRANSFER_FAILURE 中的 session_id 来定位任务卡片。若服务端因全量 unmarshal 失败直接回送空 session_id，客户端会直接丢弃该错误通知，导致发送端界面依然处于卡死挂起状态。

**建议**：补充解析失败时的契约处理：服务端应先尝试宽松提取（如先 unmarshal 出 session_id 字段），尽量携带 session_id 返回失败；若连 session_id 都无法提取，需在日志中明确记录并在控制连接层面进行相应处置。

### AGY-06 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `plan §5.1` |
| 类别 | maintainability ｜ 层次 plan |
| 置信度 | high |

**问题**：服务端实现清单遗漏 cmd/unidrop-server/main.go，且并发传输统计归属到 registry 存在模块职责倒置。

**依据**：查阅 server/cmd/unidrop-server/main.go:77，NewControlWSHandler 的初始化目前不接收 cfg 或 limits；同时查阅 server/internal/relay/relay_manager.go:67-87，传输在途状态与数据管道均由 RelayManager 维护，DeviceRegistry 仅管理 WebSocket 拓扑与在线状态。Plan §5.1 将并发统计放在 registry 会导致传输状态管理碎片化。

**建议**：在清单中补充 main.go 构造传参变更；将单账号并发传输数限制直接收敛在 RelayManager.AuthorizeSession 内判定。

## 认为正确的部分

- 老版本服务端兼容策略选型正确（三选一退化机制）：明确指出 limits 为 None 时绝不能使用 unwrap_or_default() 退化为全 0（导致什么都发不出去）或无限制（导致既有剪贴板约束失效），而是退回到客户端本地的兜底常量，确保向后兼容与行为稳定（Plan §4.3）。
- 文本 4 MB 兜底与服务端默认值严禁合并常量的设计精准：深刻识别出兜底值与新服务端默认值数值相同仅为巧合，防范了后续服务端默认值调整时无意识拖垮老服务端兜底约束的潜在隐患，并通过单测双向钉死（Plan §4.3.1、§7.2）。
- 拒绝信回送方向分析准确：精准指出现有的 routeToPeer 会将消息转发给 env.ToDevice（接收端），拒绝信必须发回当前发起会话的 session 自身，避免了“超限提示发给接收方，发送方依旧挂起”的严重缺陷（Plan §4.4）。
- 预检拦截避免污染任务历史的设计方向正确：预检在 dispatch_offer 之前进行，超限时直接阻断，避免向数据库写入 TRANSFERRING 孤儿记录及向 pending_outbound 插入悬空项（Plan §4.5）。
- 死字段清理与需求编号对齐严谨：对 rate_limit_mb、MaxItemsPerOffer、HeartbeatInterval 等历史死字段的清理态度果断，同时准确定位了业务代码和测试中因需求删除导致的编号错位（Plan §6.1-§6.5）。

## 未覆盖范围（本侧盲区）

- 数据面 WebSocket 传输过程中的流量限制与背压：本次仅审查控制面与协商阶段的静态大小与并发限额计划，未覆盖数据面传输过程中网络分片层面的实时流控与滑动窗口溢出保护。
- 多用户与动态权限体系扩展性：需求 6（多用户）与全局 PSK 权限模型在本轮仅作为背景记录，未深入评估未来多租户限额隔离的具体设计。
- 不同前端分辨率与国际化适配：未对不同语言字符长度导致的前端 CSS 截断样式（如 SendModal 内联提示）进行跨语言测试审查。

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `docs/需求.md`
- `server/cmd/unidrop-server/main.go`
- `server/configs/config.example.yaml`
- `server/internal/config/config.go`
- `server/internal/controller/control_ws.go`
- `server/internal/protocol/envelope.go`
- `server/internal/relay/relay_manager.go`
- `client/src-tauri/src/app_state.rs`
- `client/src-tauri/src/commands/clipboard_cmd.rs`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/lib.rs`
- `client/src/components/SendModal.tsx`
- `client/src/components/SettingsModal.test.tsx`

> 编排器从工具轨迹中记录到的读取次数：{"read":27,"grep":2,"glob":0,"run_command":0,"project_reads":16}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
