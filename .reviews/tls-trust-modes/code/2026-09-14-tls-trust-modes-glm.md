---
schema: trivium.review.v1
topic: tls-trust-modes
stage: code
role: glm
vendor: zcode
agent_cli: zcode/0.16.5
model_requested: GLM-5.3
model_effective: bigmodel-coding-plan/GLM-5.3
model_effective_source: trace
model_route: config_copy
run_id: 20260914T000047Z
rerun_index: 1
snapshot_hash: sha256:116fb18badda31e3167cc92977a63fe9ae347a8669b09bea54f0414e7663be03
blind: true
started_at: 2026-09-14T00:00:48.753Z
finished_at: 2026-09-14T00:09:15.846Z
duration_s: 507
tool_calls:
  read: 9
  grep: 13
  glob: 5
  run_command: 0
  project_reads: 9
evidence_ok: true
verdict: approve-with-nits
findings_count:
  blocker: 0
  major: 0
  minor: 1
  nit: 2
degraded: false
parse_fallback: false
peer_deny: false
drift: false
consulted_unverified: true
heterogeneous: true
session_id: sess_f240cc33-5053-4ea0-a36d-61c70ae4e35a
---

# 代码审查：tls-trust-modes

> Critic-B · 智谱 ZCode ｜ 模型 `bigmodel-coding-plan/GLM-5.3`
> ｜ 运行 `20260914T000047Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

最高风险项（默认档自建 ClientConfig 与 tokio-tungstenite 内建的等价性）对照 registry 中 tokio-tungstenite 0.23.1 与 rustls 0.23.44 源码逐项核实：成立——版本集合、ring provider、根存储内容、verifier 构造参数（零 CRL 时唯一默认值差异无行为影响）、ALPN/压缩/会话缓存均相同，无遗漏差异。ObservingVerifier 零判定、Pinned 档三要点（真实签名校验/空指纹失败关闭/叶子指纹）、迁移四路径含降级回写、token 日志脱敏均独立核实通过。发现三处问题：GLM-01 SAN 与 raw 错误串是攻击者可控文本，未做 Unicode/双向控制符清洗即渲染进横幅上可执行的处置建议，且建议文案本身未要求带外核对该域名属于用户自己的服务器；GLM-03 本地 pinned 测试的成功断言仍是『错误不是证书错误』而非『证书校验确实通过』的弱断言形态；GLM-02 USER_GUIDE 对照表上方写『四种成因』而表列 5 行。整体实现与测试质量高，建议批准并顺手处理三条。

**结论**：`approve-with-nits`

## 审查意见（共 3 条：次要 1 ｜ 吹毛求疵 2）

### GLM-01 · 次要（minor）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/tls_trust.rs:124-133 (humanize_san) 与 client/src/App.tsx:432,473` |
| 类别 | security ｜ 层次 code |
| 置信度 | high |

**问题**：证书 SAN 与 rustls 原始错误串是网络对端（潜在 MITM）可控的任意文本，humanize_san 只剥 Debug 包装（DnsName("..") 前后缀），不做任何字符清洗就直接进入前端横幅——而横幅文案是『请把「服务器地址」改为证书上的域名』这类用户会照做的处置建议。两条攻击面：(a) 双向控制符（U+202A-202E、U+2066-2069、U+200E/F）与 Unicode 同形字可让 presented 名字视觉上伪装成系统提示或用户熟悉的域名，raw 行（App.tsx:473 的 break-all <p>）同样含未清洗 SAN 且无高度上限（ul 有 max-h-32，raw 没有），超长 SAN 列表可把横幅撑开；(b) 更根本地，NameMismatch 档的指引假设『证书上的域名就是用户自己的服务器』，但一个在位的 MITM 完全可以出示一张自己域名的合法公共 CA 证书触发 NameMismatch，横幅随即引导用户把地址改成攻击者域名——文案没有任何『先确认该域名确实属于你的服务器（可先核对下方指纹）』的带外核对要求。React 已转义 HTML，故这是纯文本层视觉欺骗而非注入。影响受 PSK 鉴权与默认 E2EE 缓冲（重指向后攻击者主要获得持续中继位与元数据，不易拿明文），故评 minor，但这是本轮功能『不要在证书失败时误导用户』核心目标的边界情形。

**依据**：读 client/src-tauri/src/core/tls_trust.rs 全文：humanize_san（124-133 行）仅对三种前缀做 strip_prefix/strip_suffix/trim_matches('"')，认不出的形态原样返回，无任何控制符过滤；classify 把 presented 与 raw 原样放入 TlsCertFailure。读补丁中 App.tsx 横幅段及工作树 client/src/App.tsx:432,473：detail 逐行渲染于 <li>，raw 渲染于无高度上限的 break-all <p>；describe() 的 NameMismatch 文案（tls_trust.rs:220-243）为『请把「服务器地址」改为证书上的域名』且断言『这张证书本身是受信任的』，无带外核对要求；docs/USER_GUIDE.md:108-112 的对照表同样直接引导改地址。核实过 observed_cert_sha256 在 NameMismatch 档也会附带（connection_actor.rs:265-272），前端有指纹区块，但指引文案未引导用户用它核对。

**建议**：两处收口：(1) 在 humanize_san 或其调用侧过滤 Cc/Cf 类字符（至少 U+202A-202E、U+2066-2069、U+200E/F 与其它不可打印字符），对非 ASCII 域名考虑显示 punycode 或加视觉标记，并对单条名字与 raw 行做长度截断；(2) 给 NameMismatch 的指引加一句带外核对前提，例如『先确认列出的域名确实是你自己的服务器（可在服务器上核对证书），再修改地址』，把已有的指纹核对话术沿用过来。方向即可，无需大改。

### GLM-02 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `docs/USER_GUIDE.md:108` |
| 类别 | correctness ｜ 层次 code |
| 置信度 | high |

**问题**：新增的故障对照表导语写『四种成因的正确处置完全不同』，但紧随其后的表格列了 5 行（域名不匹配、签发者未知、已过期、尚未生效、格式不受支持）。用户按导语数着找会漏一行，且该表是 Q4 指回的排障主入口。另注：Q5 声明日志明文项为『服务器地址与账号标识』，但 error 级日志还会落本地文件路径（transfer_engine.rs 的 safe_target/p.display() 等），虽非凭据，介意隐私的用户按 Q5 裁剪会漏裁。

**依据**：读工作树 docs/USER_GUIDE.md:108（『四种成因』）与 110-116 行的五行表格；grep transfer_engine.rs 确认 error/warn 级日志含 p.display()、safe_target 路径输出。

**建议**：把『四种』改为『五种』（或删掉计数），Q5 的明文项清单补上本地文件路径一项。

### GLM-03 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src-tauri/src/core/tls_trust.rs:954-975 (pinned_mode_accepts_matching_self_signed_cert)` |
| 类别 | test-gap ｜ 层次 code |
| 置信度 | medium |

**问题**：该测试的 Err 分支断言仍是『错误不是证书错误』（classify(e).is_none()）而非『证书校验确实跑过并通过』——正是开发者自己在 live 测试里修掉的那类假绿形态的残留。由于测试服务器 accept TLS 后立即 shutdown，WebSocket 升级几乎必然失败，Ok 分支实际不可达，整条测试的有效信号只剩这条弱断言：若 pinned 路径发生非证书类的 TLS 失败（rustls 非 InvalidCertificate 错误、协议层失败），会被记成指纹放行成功。缓解因素确实存在且不小：同测试前半段的对照组（public_ca 必须得到 UnknownIssuer）证明了服务器在跑且 TLS 走到了校验阶段，故残余风险窗口窄，评 nit。

**依据**：读 client/src-tauri/src/core/tls_trust.rs:940-975：try_connect 的注释与实现、pinned 用例的 if let Err(e) = &res { assert!(classify(e).is_none()) }；对照同文件 1088-1132 行 live 测试的 assert_cert_accepted（以 observer 有值为前提）正是补这一形态的写法。

**建议**：仿照 live_probe/assert_cert_accepted：给该用例也传一个 CertObserver，断言指纹确实被记录且无证书错误，把『没连上/没走到校验』从通过域里排除。

## 认为正确的部分

- 焦点一等价性声明成立：对照 tokio-tungstenite 0.23.1 src/tls.rs:91-115 与 rustls 0.23.44（client_conn.rs:315-317 builder()=进程默认provider+DEFAULT_VERSIONS；builder.rs:200-204 with_safe_default_protocol_versions 同为 DEFAULT_VERSIONS；webpki/server_verifier.rs 两条构造路径零 CRL 时参数等价、无缓存字段；client/builder.rs:156-190 两路径同一终点），未发现会话缓存/ALPN/证书压缩/协议版本集合上的遗漏差异；lib.rs run() 首行安装 ring provider 保证进程默认与显式 ring 同源。
- ObservingVerifier 确为零判定：四个 trait 方法全部原样转交 inner，verify_server_cert 先 record 再委派，无任何提前返回 Ok 或吞错转成功的分支，且配有 observing_verifier_does_not_weaken_default_mode 行为测试。
- PinnedCertVerifier 三要点全部成立：verify_tls12/13_signature 在三档下都走真实实现（Pinned/PublicCa 委派 inner WebPkiServerVerifier，Insecure 走 rustls::crypto::verify_*）；pins 为空时 TlsTrustConfig::new 直接 Err 且 pins 字段私有、Default 为 PublicCa，所有调用点（lib.rs 两处 unwrap_or_*）都回落最严格档，失败方向关闭；指纹取 end_entity 即叶子证书。
- 设置迁移四条路径方向全部正确且有针对性测试：缺键→PublicCa、allow_insecure_tls=true→Insecure 保留、新键优先且 normalize_tls_trust 回写保持一致、降级回旧版时 Pinned 落 false 属安全方向失败；grep 确认 legacy bool 在 Rust 侧再无其它读取点，effective_trust_mode 确为唯一入口。
- 日志脱敏完整：数据面 URL 唯一凭据是 token= 且两条 info 日志均经 redact_query_token；控制面 URL 无凭据；全量 grep info/warn/error 后未发现 psk、签名或其它令牌落盘；account_id 明文已在 USER_GUIDE Q5 向用户声明。
- live 测试的 assert_cert_accepted 以 observer 有值为前提，正确堵住了『TCP 连不上被记成校验通过』的假绿；classify_survives_real_handshake 用真握手钉依赖树形状，pinned 用例带 public_ca 对照组，这两处测试设计是对的。
- 前端三档 UI 与后端迁移规则一致（trustMode 派生逻辑与 effective_trust_mode 同构）、指纹前后端校验规则一致（都取最后一个 = 之后、忽略冒号与空白、64 位十六进制）、不给一键信任按钮只给指纹的交互取舍合理。

## 未覆盖范围（本侧盲区）

- 未执行任何测试或构建命令（纯只读审查）：cargo test 与 vitest 的实际通过情况、以及 --ignored live 测试对 badssl.com 的真机结果均未独立复跑。
- tauri-plugin-log 2.9.1 的轮转/KeepOne/日志目录行为按其 API 文档字面采信，未读其源码；USER_GUIDE Q5 给出的三平台日志路径未逐一核实。
- rustls NotValidForNameContext 的 Display 是否把全部 SAN 拼进 raw 串未查证（影响仅限 GLM-01 中 raw 行长度问题的严重度）。
- 若用户手改 DB 写入非法 tls_trust_mode 字符串，会触发整份设置 JSON 解析失败并全量回落默认（丢 server_url/psk）——与其它字段损坏的既有行为同类，未单独立案，仅在此记录。
- lib.rs 中数据面 tls_trust_config().unwrap_or_default() 静默回落（控制面同路径有日志）的观测性不一致未立案，判断为近死代码路径。
- 服务端代码不在本补丁范围，未审。

## 实际查阅的项目文件

- `.reviews/tls-trust-modes/code/_meta/changes-20260914T000047Z.diff`
- `client/src-tauri/src/core/tls_trust.rs`
- `client/src-tauri/src/core/connection_actor.rs`
- `client/src-tauri/src/core/transfer_engine.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/commands/clipboard_cmd.rs`
- `client/src-tauri/src/storage/history_repo.rs`
- `client/src/App.tsx`
- `client/src/App.test.tsx`
- `client/src/types/index.ts`
- `client/src/components/SettingsModal.tsx`
- `client/src/components/SettingsModal.test.tsx`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/Cargo.lock`
- `docs/USER_GUIDE.md`
- `docs/需求.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":9,"grep":13,"glob":5,"run_command":0,"project_reads":9}

---

*本文档由 TriviumCode 编排器从 `glm` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
