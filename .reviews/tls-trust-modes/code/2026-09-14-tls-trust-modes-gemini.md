---
schema: trivium.review.v1
topic: tls-trust-modes
stage: code
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260914T000047Z
rerun_index: 1
snapshot_hash: sha256:116fb18badda31e3167cc92977a63fe9ae347a8669b09bea54f0414e7663be03
blind: true
started_at: 2026-09-14T00:00:48.753Z
finished_at: 2026-09-14T00:09:15.846Z
duration_s: 164
tool_calls:
  read: 26
  grep: 2
  glob: 1
  run_command: 0
  project_reads: 16
evidence_ok: true
verdict: request-changes
findings_count:
  blocker: 0
  major: 3
  minor: 1
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: 26cecb84-ddbf-41a2-9a25-2b91a22dda5e
---

# 代码审查：tls-trust-modes

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260914T000047Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

【重点警示】Insecure 档位在遇到 X.509 v1 证书时因过度过滤静默吞掉了 UnsupportedVersion 事件，导致用户关校验排障时陷入无提示重连死循环；
同时非 Pinned 档位下若残留未完成指纹，后端解析失败将导致前端在输入框已隐藏的情况下无法保存设置；
此外，本地单元测试存在假绿断言反模式，且未受信的证书 SAN 缺少控制字符与 Bidi 视觉欺骗防护。
总体判断为 request-changes，需针对上述三处功能/测试缺陷及防护细节进行修正。

**结论**：`request-changes`

## 审查意见（共 5 条：重要 3 ｜ 次要 1 ｜ 吹毛求疵 1）

### AGY-01 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/connection_actor.rs:265` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：Insecure 信任档位下静默吞掉 X.509 v1（UnsupportedVersion）错误事件，导致用户在关校验排障时陷入无提示的死循环重连。

**依据**：在 client/src-tauri/src/core/connection_actor.rs:265 中，判定条件为 `Some(mut failure) if !current_cfg.tls_trust.skips_verification() =>`。当用户在 Insecure 档下（skips_verification 恒为 true）连接出示 X.509 v1 证书的服务器时，rustls 会在握手解析阶段报错并被 classify 成功识别为 CertFailureKind::UnsupportedVersion，但由于该守卫条件，该错误被丢弃至普通重连分支，不会向前端发送 tls-cert-failed 事件。这与 client/src-tauri/src/core/tls_trust.rs:2307-2309 与 docs/USER_GUIDE.md:121-123 中「关闭证书校验对 v1 证书无效，必须让用户知晓」的承诺直接冲突。

**建议**：放宽事件上报守卫：当 failure.kind == CertFailureKind::UnsupportedVersion 时，即使当前为 skips_verification() 也应向前端广播证书错误横幅。

### AGY-02 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/tls_trust.rs:2630` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：非 Pinned 档位（PublicCa / Insecure）下残留的非法指纹文本会触发保存校验失败，且前端输入框已被隐藏导致用户无法修正。

**依据**：在 client/src-tauri/src/core/tls_trust.rs:2630-2636 中，TlsTrustConfig::new 在循环中无条件执行 `pins.push(normalize_fingerprint(raw)?);`，未检查当前 mode 是否为 Pinned。在 client/src/components/SettingsModal.tsx:1992 中，当 trustMode 不是 pinned 时输入框被折叠隐藏；若用户曾输入过非法/未完成指纹随后切回 PublicCa 或 Insecure 档并保存，前端放行提交，但后端 settings_cmd.rs:268 调用 tls_trust_config() 时因残留文本直接报错，导致用户对着不可见输入框遭遇无法保存的死结。

**建议**：在 TlsTrustConfig::new 中将指纹格式解析限定在 mode == TlsTrustMode::Pinned 分支，或在非 Pinned 模式下忽略 raw_pins，避免无效文本阻断安全档位的保存。

### AGY-03 · 重要（major）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/tls_trust.rs:3244` |
| 类别 | test-gap ｜ 层次 code |
| 置信度 | high |

**问题**：本地单元测试 pinned_mode_accepts_matching_self_signed_cert 存在断言「非证书错误即视为通过」的假绿漏洞。

**依据**：在 client/src-tauri/src/core/tls_trust.rs:3244-3250 的 pinned_mode_accepts_matching_self_signed_cert 测试中，若 try_connect 返回 Err(e)，仅断言 `assert!(classify(e).is_none())`。由于本地自签 mock 服务端在 TLS accept 成功后立即 shutdown，客户端必定得到 EOF/协议错误；而若本地发生网络拒绝或 TCP 未连上，classify(e) 同样为 None。测试传入的是未装观察器的 connector，无法证实握手确实走到证书验证这一步，属于典型的假绿断言（对比同文件 3355-3360 行 LiveOutcome 修复的正是同一反模式）。

**建议**：为该测试挂载 CertObserver，并明确断言 observer.observed() 确实捕获到了预期证书指纹，确保连接确实走过了完整的 TLS 证书校验流程。

### AGY-04 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/tls_trust.rs:2400` |
| 类别 | security ｜ 层次 code |
| 置信度 | medium |

**问题**：证书 SAN 字段未对控制字符与双向文本覆写（Bidi Overrides）进行过滤，且单项未限制最大长度，存在视觉欺骗与提示伪造风险。

**依据**：在 client/src-tauri/src/core/tls_trust.rs:2399-2427 中，humanize_san 与 format_presented 仅剔除固定前缀和双引号，未过滤 ASCII 控制字符（如 \r, \n）和 Unicode 双向控制字符（如 U+202E RLO 等 Bidi Overrides），亦未限制单个条目字符长度。在 client/src/App.tsx:1670-1676 中，detail 列表项直接以 break-words 渲染，攻击者可构造含换行符或 RTL 覆写字符的畸形 SAN 伪造系统提示文本或反转整行渲染方向。

**建议**：在 humanize_san 中清洗 ASCII 控制字符与 Unicode Bidi 控制字符，并在 format_presented 中对单个名称长度进行安全截断。

### AGY-05 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src/types/index.ts:130` |
| 类别 | maintainability ｜ 层次 code |
| 置信度 | high |

**问题**：类型注释与实际事件载荷契约漂移，误称 observed_cert_sha256 恒为 null / None。

**依据**：在 client/src/types/index.ts:130 与 client/src-tauri/src/core/tls_trust.rs:56 的注释中，均标注 observed_cert_sha256「目前后端恒为 null / None，留给后续证书观察器」。然而本轮提交已在 client/src-tauri/src/core/connection_actor.rs:266 实际赋值了 observer.observed()，且前端 App.tsx:1677-1709 已展示该指纹。类型注释与实际运行时契约脱节。

**建议**：更新前后端关于 observed_cert_sha256 字段的注释文档，如实描述其在控制面握手失败时携带实际观测证书指纹的行为。

## 认为正确的部分

- 默认档（PublicCa）自建 ClientConfig 经核对与 tokio-tungstenite 0.23.1 src/tls.rs 完全等价：同源 webpki_roots、ring provider、均未配置 ALPN、协议版本一致，且通过 OnceLock 避免了每次重连重建根证书存储的性能开销。
- ObservingVerifier 实现了严密的零判定委派：所有四个 ServerCertVerifier trait 方法均原样无损转发给 inner，未引入任何提前 Ok 或吞错分支。
- PinnedCertVerifier 密码学设计严谨：TLS 1.2 与 TLS 1.3 签名校验均由真实底层算法验证，指纹匹配严格针对 end-entity 叶子证书且空 pins 不放行，有效杜绝证书副本冒充。
- 数据面与控制面信任策略同步严格，日志级别提至 Info 落盘时 redact_query_token 成功脱敏查询串中的一次性令牌，全量代码排查未见 PSK 或派生密钥泄露。
- 设置迁移双向投影严谨，effective_trust_mode() 与 normalize_tls_trust() 保证了老配置缺失或降级时始终落在安全侧（fail-closed）。

## 未覆盖范围（本侧盲区）

- 服务端 Go 中继实现（本次仅审查客户端 Tauri 守护进程与前端 React 变更）。
- 跨平台日志目录底层写入权限及日志文件轮转在 Linux/macOS/Windows 实机运行时的边界行为。
- 带有 #[ignore] 标签的外网 badssl.com 真机握手测试实际网络交互（仅静态审阅其断言逻辑）。
- 海量设备连接抖动下 WebSocket 自动重连及事件风暴对前端渲染性能的压力影响。

## 实际查阅的项目文件

- `CLAUDE.md`
- `AGENTS.md`
- `README.md`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/core/connection_actor.rs`
- `client/src-tauri/src/core/tls_trust.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src-tauri/src/core/cache_manager.rs`
- `client/src/App.test.tsx`
- `client/src/App.tsx`
- `client/src/components/SettingsModal.test.tsx`
- `client/src/components/SettingsModal.tsx`
- `client/src/types/index.ts`
- `docs/USER_GUIDE.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":26,"grep":2,"glob":1,"run_command":0,"project_reads":16}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
