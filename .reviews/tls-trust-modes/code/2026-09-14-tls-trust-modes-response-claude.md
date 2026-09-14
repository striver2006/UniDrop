---
schema: trivium.disposition.v1
topic: tls-trust-modes
stage: code
role: claude
kind: response
run_id: 20260914T000047Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - AGY-04
  - AGY-05
  - GLM-01
  - GLM-02
  - GLM-03
---

# 代码审查应答：TLS 信任三档 + 证书失败分类 + 落盘日志

- 主题：`tls-trust-modes`
- 阶段：code（应答确认，含逐条裁决）
- 日期：2026-09-14
- 被审对象：`git diff HEAD`（HEAD = ab2f75e4），15 个文件，补丁 152.7 KB
- 两侧结论：Antigravity `request-changes`（重要 3 ｜ 次要 1 ｜ 吹毛求疵 1）；ZCode `approve-with-nits`（次要 1 ｜ 吹毛求疵 2）

## 裁决表

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | major | 接受 | 已独立核实成立，且比原意见更广：rustls `webpki/verify.rs:204` 的 `EndEntityCert::try_from` 在**签名校验**阶段解析证书，Insecure 档同样会走到这里，v1 证书经 `pki_error` 转成 `Other(UnsupportedCertVersion)`。守卫条件 `!skips_verification()` 把它吞掉，用户关了校验反而连一句解释都看不到——恰是 USER_GUIDE 明确承诺会提示的那种情形。 | `connection_actor.rs:265` 去掉 `!skips_verification()` 守卫（见下「对 AGY-01 的修正」，修法比原建议更彻底）；补一条测试钉住 Insecure 档下证书错误仍上报 |
| AGY-02 | gemini | major | 接受 | 核实成立，死结路径完整：`tls_trust.rs` 的 `TlsTrustConfig::new` 无条件 `normalize_fingerprint(raw)?` 不看 mode；`SettingsModal.tsx:257` 的格式校验只在 `trustMode === "pinned"` 时跑；`:396` 起输入框在非 pinned 档折叠隐藏。三者叠加 = 用户在 pinned 档填错指纹后切回 PublicCa 保存，后端报错而输入框已不可见，无从修正。 | `TlsTrustConfig::new` 仅在 `mode == Pinned` 时解析指纹；新增测试覆盖「非 Pinned 档携带非法指纹仍可保存」 |
| AGY-03 | gemini | major | 接受 | 与 GLM-03 同一处，两侧独立命中。核实属实：`pinned_mode_accepts_matching_self_signed_cert` 的 Err 分支只断言 `classify(e).is_none()`，且 `try_connect` 未挂观察器，无法区分「证书校验通过」与「根本没走到校验」。这正是我在 live 测试里修掉、却没回头清理本地用例的同一反模式。严重度采 GLM 的 nit 判断（前半段 public_ca 对照组已证明服务端在跑，残余窗口窄），但修复成本极低，按 major 一并处理。 | 给该用例挂 `CertObserver`，断言指纹确被记录且无证书错误，复用 live 侧 `assert_cert_accepted` 的形态 |
| AGY-04 | gemini | minor | 接受 | 与 GLM-01(a) 重叠，两侧独立命中。核实属实：`humanize_san` 只做 `strip_prefix`/`strip_suffix`/`trim_matches('"')`，不过滤 Cc/Cf 类字符；`App.tsx:473` 的 raw 行无高度上限（detail 的 `ul` 有 `max-h-32`，raw 没有）。React 转义了 HTML，故非注入而是纯文本层视觉欺骗——但 SAN 与 raw 都是网络对端可控文本，而横幅正是用户照着做处置的地方。 | `humanize_san` 过滤控制字符与 Bidi 覆写（U+202A–202E、U+2066–2069、U+200E/F 及不可打印字符）；`format_presented` 对单条名字截断；raw 行补高度上限 |
| AGY-05 | gemini | nit | 接受 | 核实属实，是我分阶段实施留下的注释漂移：`tls_trust.rs:56` 与 `types/index.ts:129` 都写着「目前恒为 None/null，留给后续的证书观察器」，而 `connection_actor.rs:266` 本轮已实际赋值、前端也已展示。注释比代码更容易误导后来者，因为它看起来是权威说明。 | 改写两处注释，如实描述「控制面握手失败时携带实际观测到的指纹；数据面不装观察器故为 None」 |
| GLM-01 | glm | minor | 接受 | (a) 与 AGY-04 同源，合并处理。(b) 是本轮**唯一触及核心目标反面**的意见，价值最高且 Antigravity 未发现：在位的 MITM 可出示一张自己域名的合法公共 CA 证书触发 NameMismatch，我的横幅随即引导用户「把服务器地址改为证书上的域名」——把用户直接送到攻击者域名。文案还断言「这张证书本身是受信任的」，进一步降低警觉。这条指引原本是为修复「误导用户关校验」而写的，却在边界情形下制造了另一种误导。 | (a) 同 AGY-04 落点；(b) `describe()` 的 NameMismatch 文案补一句带外核对前提，把已有的指纹核对话术沿用过来；`USER_GUIDE.md` 对照表同步；新增反向断言测试钉住该句不被删 |
| GLM-02 | glm | nit | 接受 | 两半都核实属实。计数错误：`USER_GUIDE.md:108` 写「四种成因」而表格 5 行（域名不匹配 / 签发者未知 / 已过期 / 尚未生效 / 格式不受支持），且该表是 Q4 指过去的排障主入口。Q5 遗漏：`transfer_engine.rs:133,140,864,926` 等 warn/error 级日志会落 `{:?} p`、`safe_target`、`item.relative_path`，即本地文件路径与文件名——非凭据，但介意隐私的用户按 Q5 现有清单裁剪会漏裁。 | `USER_GUIDE.md:108` 计数改为五种（或删掉计数）；Q5 明文项清单补「本地文件路径与文件名」 |
| GLM-03 | glm | nit | 接受 | 与 AGY-03 同一处，合并处理。GLM 的严重度判断更准确——它指出了 Antigravity 没提的缓解因素（同测试前半段的 public_ca 对照组已证明服务器在跑且 TLS 走到了校验阶段），也指出了 Ok 分支实际不可达这一点。 | 同 AGY-03 落点 |

## 对 AGY-01 的修正：修法应比原建议更彻底

原建议是「当 `kind == UnsupportedVersion` 时即使 `skips_verification()` 也广播」。这治得了本例，治不了同一类问题。

更完整的判断依据是：**Insecure 档下 `classify` 还能返回 `Some`，本身就说明这不是用户关掉的那类校验。** `InsecureServerCertVerifier::verify_server_cert` 无条件放行，链、域名、有效期三项全不查；能在这一档下仍然冒出来的证书错误，只可能来自更底层的解析（v1、畸形 DER、算法不受支持），而那些是**跳过校验也绕不过**的。对它们保持沉默，等于在用户最需要解释的时候闭嘴。

所以落点是直接去掉 `!current_cfg.tls_trust.skips_verification()` 这个守卫，而不是给它开一个 `UnsupportedVersion` 的特例口子。守卫当初的原意（「用户自己关掉的校验，不该再拿校验失败去打扰他」）本身没错，错在它假定了「Insecure 档下不会有证书错误」——这个假定不成立。

同时保留 `TlsTrustConfig::skips_verification()` 方法：AGY-02 的修复要用它来决定是否解析指纹。

## 严重度分歧的处理

AGY-03 与 GLM-03 指同一处，两侧给出 major / nit 两种严重度。采 GLM 的分析（对照组构成实质缓解，残余窗口窄），但因修复成本极低，不因严重度下调而延后，与 major 项一并落地。

两侧对本轮焦点一（默认档自建 `ClientConfig` 的等价性）**独立得出一致结论：等价成立**。GLM 侧逐项对照了 `tokio-tungstenite 0.23.1 src/tls.rs:91-115`、`rustls 0.23.44 client_conn.rs:315-317`、`builder.rs:200-204`、`webpki/server_verifier.rs` 与 `client/builder.rs:156-190`，确认版本集合、provider、根存储、verifier 构造参数（零 CRL 时唯一的默认值差异无行为影响）、ALPN / 压缩 / 会话缓存均相同。这是本轮风险最高的一处，双侧背靠背同向确认后可以放心。

## 无异议、不修改的部分

两侧列出的「认为正确的部分」不构成待办，此处不逐条复述。仅记录两条 GLM 主动登记为「未立案」的观测，我同意不在本轮处理：

- 用户手改 DB 写入非法 `tls_trust_mode` 会触发整份设置 JSON 解析失败并全量回落默认（丢 server_url/psk）——与其它字段损坏的既有行为同类，属既有面而非本轮引入，不在本轮扩大范围。
- `lib.rs` 数据面 `tls_trust_config().unwrap_or_default()` 静默回落（控制面同路径有日志）的观测性不一致——判断为近死代码路径（控制面连不上时数据面根本不启动），不值得为它铺第二条日志链路。

## 待办汇总（按落地顺序）

1. **AGY-01** `connection_actor.rs` 去掉 `!skips_verification()` 守卫 + 测试
2. **AGY-02** `TlsTrustConfig::new` 按 mode 决定是否解析指纹 + 测试
3. **AGY-03 / GLM-03** `pinned_mode_accepts_matching_self_signed_cert` 挂观察器强化断言
4. **AGY-04 / GLM-01(a)** `humanize_san` 字符清洗 + 长度截断 + raw 行高度上限
5. **GLM-01(b)** NameMismatch 文案补带外核对前提 + USER_GUIDE 同步 + 反向断言测试
6. **AGY-05** 两处 `observed_cert_sha256` 注释改写
7. **GLM-02** USER_GUIDE 计数修正 + Q5 明文项补本地文件路径

全部 8 条均为接受，无驳回、无暂缓。

**本裁决件不含任何代码改动**——闸门期不修改业务文件。以上待办等 `/dual-approve` 之后执行。
