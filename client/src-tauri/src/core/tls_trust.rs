//! TLS 信任策略与证书失败分类。
//!
//! 从 `connection_actor` 拆出来的原因有二：那个模块已经同时管着握手、鉴权、
//! 重连与心跳；而证书这块接下来还要长出信任档位与自定义 verifier。
//!
//! 本模块的立场，一句话：**证书校验失败有很多种，它们的正确处置互不相同，
//! 把它们统一引导到「关掉校验」是错的。** 这不是洁癖——其中至少两种关了也没用
//! （X.509 v1 在解析阶段就被拒、过期证书该去续期），而「名字不匹配」那一种，
//! 关校验等于为了一张**完全合法**的证书把整个传输链路的身份验证废掉。

use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite;

/// 证书失败的分类。每一档对应一种完全不同的处置动作。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CertFailureKind {
    /// 证书本身有效，但签发给了别的名字。典型成因是用 IP 直连一个只对域名
    /// 签发的证书——证书完全受信任，错的是我们连的地址。
    NameMismatch {
        expected: String,
        presented: Vec<String>,
    },
    /// 公共根证书库里没有这个签发者：自签证书或企业内部 CA。
    UnknownIssuer,
    Expired,
    NotYetValid,
    Revoked,
    /// X.509 v1。rustls 在**解析**阶段就拒绝，早于任何校验逻辑，
    /// 因此跳过校验对它无效——这一点必须让用户知道，否则他会一直试错。
    UnsupportedVersion,
    /// 分类不出来的证书错误。`raw` 是唯一的现场，必须原样带走。
    Other,
}

impl CertFailureKind {
    /// 给日志用的短标签。前端不依赖它（前端读 serde 的 `kind`）。
    pub fn tag(&self) -> &'static str {
        match self {
            Self::NameMismatch { .. } => "name_mismatch",
            Self::UnknownIssuer => "unknown_issuer",
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::Revoked => "revoked",
            Self::UnsupportedVersion => "unsupported_version",
            Self::Other => "other",
        }
    }
}

/// 发给前端的载荷。
///
/// `observed_cert_sha256` 携带握手中**实际观察到的**叶子证书指纹，
/// 供用户与服务器上那张证书核对——对不上就意味着中间有人。
/// 只有控制面装了观察器，数据面为 `None`（见 `create_tls_connector`）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TlsCertFailure {
    #[serde(flatten)]
    pub kind: CertFailureKind,
    /// 横幅收起时显示的一行。
    pub title: String,
    /// 展开后逐行显示，已经是对症的处置建议。
    pub detail: Vec<String>,
    /// 原始错误串。永远保留：分类错了的时候这是唯一的现场。
    pub raw: String,
    pub observed_cert_sha256: Option<String>,
}

/// 从 tungstenite 的错误里取出 rustls 的错误本体。
///
/// **为什么不用 `source()` 链**——试过，走不通，这里写清楚免得后人再"简化"回去：
///
/// - `tokio-tungstenite` 的 rustls 分支把握手失败包成 `Error::Io`（不是 `Error::Tls`，
///   这正是用户看到的 `IO error:` 前缀的由来）；
/// - `tokio-rustls` 用 `io::Error::new(InvalidData, rustls_err)` 包装，未转字符串，
///   所以类型信息其实还在；
/// - 但 `std::io::Error::source()` 返回的是**内层错误的 source**，不是内层错误本身，
///   而 `rustls::Error::source()` 是 `None`。链子到这里就断了。
///
/// 所以只能 `get_ref()` 拿到 `&dyn Error` 再 downcast。
///
/// **唯一的脆弱点**：downcast 成立的前提是本包依赖的 `rustls` 与 `tokio-rustls`
/// 依赖的那个解析成**同一个 crate 实例**。今天 Cargo.lock 里只有 rustls 0.23.44
/// 一份，成立。若将来分裂成两份（例如 tokio-tungstenite 升到依赖 rustls 0.24），
/// 这里会静默返回 `None`，分类能力随之消失——退化成通用提示，方向是安全的，
/// 但会悄无声息。`classify_survives_real_handshake` 那条测试就是钉这一点的。
fn extract_rustls_error(err: &tungstenite::Error) -> Option<&rustls::Error> {
    match err {
        tungstenite::Error::Io(io_err) => io_err.get_ref()?.downcast_ref::<rustls::Error>(),
        // 本项目走不到这一支（用的是 rustls feature），留着是为了将来万一换成
        // native-tls 之外的其他 TLS 后端时，这里不至于无声漏判。
        tungstenite::Error::Tls(tungstenite::error::TlsError::Rustls(e)) => Some(e),
        _ => None,
    }
}

/// 按错误文本判断"是不是证书问题"。
///
/// 只在 `extract_rustls_error` 拿不到类型时兜底，所以它只回答是与不是，
/// 不回答是哪一种——分类交给类型化的那条路。
fn is_cert_error_by_text(text: &str) -> bool {
    text.contains("certificate")
        || text.contains("CertificateError")
        || text.contains("UnknownIssuer")
        || text.contains("NotValidForName")
        || text.contains("invalid peer certificate")
}

/// 把 rustls 给的一条 SAN 项还原成人能读的名字。
///
/// **不要以为 `presented` 里是裸域名。** webpki 填这个字段时用的是
/// `format!("{:?}", GeneralName)`（rustls-webpki 的 dns_name.rs），
/// 所以实际拿到的形如 `DnsName("www.leafun.xyz")`、`IpAddress(1.2.3.4)`、
/// `UniformResourceIdentifier("...")`。直接显示出去，用户看到的就是
/// 一行 `证书签发给：DnsName("www.leafun.xyz")`。
///
/// 这个坑是真机验证抓到的：单元测试里手工构造的 `presented` 是裸字符串，
/// 怎么测都不会暴露它。
///
/// 认不出的形态原样返回——宁可显示得难看一点，也不要为了好看把信息吃掉。
fn humanize_san(raw: &str) -> String {
    for prefix in ["DnsName(", "IpAddress(", "UniformResourceIdentifier("] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            if let Some(inner) = rest.strip_suffix(')') {
                return sanitize_untrusted(inner.trim_matches('"'));
            }
        }
    }
    sanitize_untrusted(raw)
}

/// 单个名称在界面上的最大长度。超出截断。
const MAX_NAME_LEN: usize = 80;

/// 清洗来自网络对端的文本，供直接渲染到界面。
///
/// **SAN 与原始错误串都是攻击者可控的。** 能触发证书错误的人，也就能决定
/// 证书里写什么名字——而这些名字会出现在横幅上，紧挨着「请把服务器地址改为
/// 证书上的域名」这类用户会照做的指引。React 转义了 HTML，所以这里防的不是
/// 注入，是**视觉欺骗**：
///
/// - 双向控制符（U+202A–202E、U+2066–2069、U+200E/F）能反转渲染方向，
///   把一段文本显示成完全不同的样子；
/// - 其它 Cc/Cf 类不可打印字符可以伪造换行、拼出形似系统提示的片段；
/// - 超长名字能把横幅撑开、把后面的内容挤出视野。
///
/// 被删掉的字符用 U+FFFD 顶替而不是静默丢弃：留一个可见的痕迹，
/// 让「这里原本有东西」这件事本身可见。
fn sanitize_untrusted(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(MAX_NAME_LEN));

    for (count, ch) in s.chars().enumerate() {
        if count >= MAX_NAME_LEN {
            out.push('…');
            break;
        }
        let dangerous = matches!(ch,
            // 双向覆写 / 隔离 / 标记
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}'
        ) || ch.is_control();

        out.push(if dangerous { '\u{FFFD}' } else { ch });
    }
    out
}

/// 清洗原始错误串。与 `sanitize_untrusted` 同样的字符规则，但长度上限宽得多——
/// 它是分类出错时唯一的现场，截太狠就没有排查价值了。
fn sanitize_raw(s: &str) -> String {
    const MAX_RAW_LEN: usize = MAX_NAME_LEN * 4;
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= MAX_RAW_LEN {
            out.push('…');
            break;
        }
        let dangerous = matches!(ch,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}'
        ) || (ch.is_control() && ch != '\n');
        out.push(if dangerous { '\u{FFFD}' } else { ch });
    }
    out
}

/// SAN 列表在界面上最多列几个。超出的折叠掉——横幅在一个约 760px 宽的窗口里，
/// 而通配符证书的 SAN 动辄几十条，全列出来会把设备列表整个挤出视口。
const MAX_PRESENTED_NAMES: usize = 5;

fn format_presented(presented: &[String]) -> String {
    if presented.is_empty() {
        return "（证书没有声明任何适用名称）".to_string();
    }
    // 这里拿到的已经是 classify 清洗过的名字，不必再剥一次。
    if presented.len() <= MAX_PRESENTED_NAMES {
        return presented.join("、");
    }
    format!(
        "{}…等 {} 个",
        presented[..MAX_PRESENTED_NAMES].join("、"),
        presented.len()
    )
}

/// 把 rustls 的证书错误翻译成用户能照着做的处置建议。
///
/// 非证书类错误返回 `None`——连不上、DNS 失败之类不该套上证书的壳。
pub fn classify(err: &tungstenite::Error) -> Option<TlsCertFailure> {
    // raw 里嵌着 rustls 拼进去的 SAN，同样是对端可控文本，同样要显示到界面上。
    // 截断上限放宽到名字的四倍：它是排查现场，信息量比单个名字大得多。
    let raw = sanitize_raw(&err.to_string());

    let kind = match extract_rustls_error(err) {
        Some(rustls::Error::InvalidCertificate(ce)) => classify_certificate_error(ce),
        // 拿到了 rustls 错误，但不是证书类（握手失败、协议不兼容等）。
        Some(_) => return None,
        // downcast 不成立：退回文本判定，只回答"是不是证书问题"。
        None => {
            if !is_cert_error_by_text(&raw) {
                return None;
            }
            CertFailureKind::Other
        }
    };

    let (title, detail) = describe(&kind);
    Some(TlsCertFailure {
        kind,
        title,
        detail,
        raw,
        observed_cert_sha256: None,
    })
}

fn classify_certificate_error(ce: &rustls::CertificateError) -> CertFailureKind {
    use rustls::CertificateError as CE;
    match ce {
        CE::NotValidForNameContext {
            expected,
            presented,
        } => CertFailureKind::NameMismatch {
            expected: expected.to_str().to_string(),
            // 在这里清洗而不是留到格式化时：这个字段会经 serde 发给前端，
            // 存着 Debug 包装等于把脏数据一路带到界面上。
            presented: presented.iter().map(|p| humanize_san(p)).collect(),
        },
        // 没有上下文的老形态：知道是名字不匹配，但不知道两边分别是什么。
        CE::NotValidForName => CertFailureKind::NameMismatch {
            expected: String::new(),
            presented: Vec::new(),
        },
        CE::UnknownIssuer => CertFailureKind::UnknownIssuer,
        CE::Expired | CE::ExpiredContext { .. } => CertFailureKind::Expired,
        CE::NotValidYet | CE::NotValidYetContext { .. } => CertFailureKind::NotYetValid,
        CE::Revoked => CertFailureKind::Revoked,
        // X.509 v1 没有专门的变体，落在 Other 里，只能看文本。
        // 只在这一支做字符串匹配是刻意的：为它引 rustls-webpki 直接依赖去 downcast
        // 同样受"同一 crate 实例"的约束，收益为零。
        CE::Other(o) if o.to_string().contains("UnsupportedCertVersion") => {
            CertFailureKind::UnsupportedVersion
        }
        // CertificateError 是 #[non_exhaustive]，这一支不能省。
        _ => CertFailureKind::Other,
    }
}

/// 各档的标题与处置建议。
///
/// 这里是本次改造真正的交付物。改动之前，**所有**档位共用一句
/// 「请在设置中勾选『允许不安全连接』」——对名字不匹配那一档来说，
/// 那是在教用户为了一张合法证书关掉全部身份验证。
fn describe(kind: &CertFailureKind) -> (String, Vec<String>) {
    match kind {
        CertFailureKind::NameMismatch {
            expected,
            presented,
        } if !expected.is_empty() => (
            format!("服务器证书有效，但不是签发给 {expected} 的"),
            vec![
                format!("证书签发给：{}", format_presented(presented)),
                format!("你连接的是：{expected}"),
                // 带外核对必须排在「改地址」前面，否则这条指引本身就是攻击面。
                //
                // 「证书由公共 CA 签发」只证明签发者确认了对方拥有那个域名，
                // **不证明那个域名是你的服务器**。一个在位的中间人完全可以出示
                // 一张自己域名的合法证书，于是这里如实报出它——而如果我们紧接着
                // 说「把地址改成证书上的域名」，就等于替攻击者把用户请了进去。
                //
                // 这一整轮改造是为了修「误导用户关掉校验」，不能在这里换一种方式
                // 继续误导他。下方的指纹正是用来做这次核对的。
                "先确认上面列出的域名确实是你自己的服务器——证书由公共 CA 签发只说明                 对方拥有那个域名，不说明那台机器是你的。"
                    .to_string(),
                "确认无误后，把「服务器地址」改为证书上的域名，端口保持不变。".to_string(),
                "若这个域名你并不认识，不要改地址，也不要关闭证书校验——那可能意味着                 连接被中间人接管了。"
                    .to_string(),
            ],
        ),
        // 退化形态：rustls 没给出上下文，只能说清方向。
        CertFailureKind::NameMismatch { .. } => (
            "服务器证书上的名称与你填的地址不匹配".to_string(),
            vec![
                "常见成因是用 IP 直连一台只为域名签发证书的服务器。".to_string(),
                "请核对服务器上的证书，确认它确实是你自己的，再把「服务器地址」改成                 证书上的域名。"
                    .to_string(),
                "证书本身可能完全有效，不要为此关闭证书校验。".to_string(),
            ],
        ),
        CertFailureKind::UnknownIssuer => (
            "服务器证书由未知的签发者签发".to_string(),
            vec![
                "公共根证书库里没有这个签发者，因此无法确认对端身份。".to_string(),
                "常见于自签证书或企业内部 CA。".to_string(),
                "若这是你自己的服务器，建议改用受信任 CA 签发的证书（Let's Encrypt 可免费签发）。"
                    .to_string(),
            ],
        ),
        CertFailureKind::Expired => (
            "服务器证书已过期".to_string(),
            vec![
                "请在服务端续期证书；使用 Let's Encrypt 时执行 certbot renew。".to_string(),
                "关闭证书校验也能连上，但那会同时放弃对所有中间人的防护——续期是几分钟的事。"
                    .to_string(),
            ],
        ),
        CertFailureKind::NotYetValid => (
            "服务器证书尚未生效".to_string(),
            vec![
                "证书的生效时间还没到。".to_string(),
                "先检查本机系统时间是否正确——时间跑偏是这个错误最常见的原因，而不是证书真有问题。"
                    .to_string(),
            ],
        ),
        CertFailureKind::Revoked => (
            "服务器证书已被吊销".to_string(),
            vec![
                "签发者已声明这张证书作废，通常意味着私钥泄漏。".to_string(),
                "在查清原因之前不要继续连接，更不要关闭证书校验。".to_string(),
            ],
        ),
        CertFailureKind::UnsupportedVersion => (
            "服务器证书是 X.509 v1，客户端无法解析".to_string(),
            vec![
                "rustls 在解析阶段就会拒绝 v1 证书，这一步早于任何校验逻辑——关闭证书校验对它无效。"
                    .to_string(),
                "必须重新签发带扩展的 v3 证书。用 openssl 自签时要带上 -addext subjectAltName=...，"
                    .to_string(),
                "macOS 自带的 LibreSSL 不加这个参数会默认产出 v1 证书。".to_string(),
            ],
        ),
        CertFailureKind::Other => (
            "无法验证服务器证书".to_string(),
            vec![
                "证书校验没有通过，但客户端无法判断具体原因。".to_string(),
                "下方的原始错误信息可用于排查。".to_string(),
            ],
        ),
    }
}

/// 三档信任策略。
///
/// 改造前只有一个 `allow_insecure_tls: bool`，于是「合法证书但名字对不上」
/// 和「企业内部 CA 签发」这类部署，唯一的出路就是把校验整个关掉——
/// 一个本该收窄到单台服务器的例外，被迫放大成对所有中间人敞开。
/// 中间那一档就是为这两种情形补的。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsTrustMode {
    /// 默认：只信任公共根证书库。
    ///
    /// `#[default]` 不只是省掉一个 impl：它让「三档里哪一档是安全默认」
    /// 写在枚举定义上，而不是藏在别处的 impl 里。
    #[default]
    PublicCa,
    /// 公共根校验不过时，再比对用户指定的证书指纹。
    Pinned,
    /// 完全不校验。任何出示证书的中间人都能接管连接。
    Insecure,
}

/// 把用户填的指纹规范化成 32 字节。
///
/// 接受 `openssl x509 -fingerprint -sha256` 的原样输出（带冒号、大写），
/// 也接受裸 hex。**在保存设置时调用**，不在重连循环里——格式错了要当场
/// 以精确原因回给用户，而不是变成一次次查不出原因的连接失败。
pub fn normalize_fingerprint(raw: &str) -> Result<[u8; 32], String> {
    // openssl 的输出形如 "SHA256 Fingerprint=AB:CD:..."，把前缀也一并吃掉，
    // 省得用户还要手工裁剪——那一步最容易连带把首字节删掉。
    let body = raw.rsplit('=').next().unwrap_or(raw);
    let cleaned: String = body
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .collect();

    if cleaned.len() != 64 {
        return Err(format!(
            "指纹应为 64 个十六进制字符（SHA-256），当前为 {} 个：{}",
            cleaned.len(),
            raw.trim()
        ));
    }
    let bytes = hex::decode(&cleaned).map_err(|e| format!("指纹不是合法的十六进制：{e}"))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// 连接层真正使用的信任配置。
#[derive(Debug, Clone, Default)]
pub struct TlsTrustConfig {
    pub mode: TlsTrustMode,
    pins: Vec<[u8; 32]>,
}

impl TlsTrustConfig {
    /// 由设置构造。指纹解析失败直接报错——见 `normalize_fingerprint` 的理由。
    ///
    /// **只有 Pinned 档才解析指纹。** 这不是优化，是在拆一个死结：
    /// 设置面板只在 Pinned 档显示指纹输入框，格式校验也只在那一档跑。
    /// 早先这里无条件解析所有 raw_pins，于是「在 Pinned 档填错指纹 → 切回
    /// 公共 CA 档 → 保存」会被后端拒掉，而那时输入框已经折叠不可见——
    /// 用户对着一个看不见的字段反复保存失败，没有任何出路。
    ///
    /// 非 Pinned 档下那些文本原样留在设置里（不清空），这样他切回来还能看到
    /// 自己填过什么、错在哪里。它们只是不参与这一档的判定。
    pub fn new(mode: TlsTrustMode, raw_pins: &[String]) -> Result<Self, String> {
        if mode != TlsTrustMode::Pinned {
            return Ok(Self {
                mode,
                pins: Vec::new(),
            });
        }

        let mut pins = Vec::new();
        for raw in raw_pins {
            if raw.trim().is_empty() {
                continue;
            }
            pins.push(normalize_fingerprint(raw)?);
        }
        if pins.is_empty() {
            return Err("选择「信任指定证书」时至少要填一条证书指纹".to_string());
        }
        Ok(Self { mode, pins })
    }

    pub fn public_ca() -> Self {
        Self::default()
    }

    /// 这一档是否根本不做身份校验。用于决定要不要把证书失败报给用户——
    /// 用户自己关掉的校验，不该再拿校验失败去打扰他。
    pub fn skips_verification(&self) -> bool {
        self.mode == TlsTrustMode::Insecure
    }
}

/// 公共根证书库。
///
/// 内容与 tokio-tungstenite 在 `rustls-tls-webpki-roots` feature 下自建的
/// 完全一致（同一个 `webpki_roots::TLS_SERVER_ROOTS`）——这一点是默认档
/// 行为不变的依据，改动这里等于改动所有用户的信任集。
///
/// 顺带修掉一处浪费：上游那段在**每次 connect 时**都重建一遍根存储，
/// ~150 个锚点，每次重连都做。这里只建一次。
fn public_root_store() -> &'static Arc<rustls::RootCertStore> {
    static ROOTS: std::sync::OnceLock<Arc<rustls::RootCertStore>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut store = rustls::RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(store)
    })
}

/// 记录握手中实际看到的服务器证书指纹。
///
/// 存在的理由只有一个：让用户能核对"客户端看到的证书"与"服务器上那张证书"
/// 是不是同一张。指纹对不上就意味着中间有人——而这恰恰是填指纹之前
/// 唯一该做的一步。
///
/// 为什么在握手过程中取而不是事后再连一次去探：事后探测既多一次握手，
/// 又可能探到另一台机器（负载均衡、DNS 轮询），拿回来的指纹和刚才失败的
/// 那次根本不是同一张证书——那比不显示更糟，因为它看起来是可信的。
#[derive(Debug, Default)]
pub struct CertObserver {
    seen: std::sync::Mutex<Option<String>>,
}

impl CertObserver {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn observed(&self) -> Option<String> {
        self.seen.lock().ok().and_then(|g| g.clone())
    }

    fn record(&self, der: &[u8]) {
        let digest = Sha256::digest(der);
        if let Ok(mut g) = self.seen.lock() {
            *g = Some(hex::encode(digest));
        }
    }
}

/// 只做一件事：把叶子证书的指纹记下来，然后把判定**原样**交给 inner。
///
/// **它不改变任何校验结果。** 这一点必须保持——因为装它的时候用的是
/// `.dangerous().with_custom_certificate_verifier()`，那个 API 名字会让
/// 后来读代码的人以为默认档的安全性被削弱了。没有：inner 就是
/// `WebPkiServerVerifier`，与 `with_root_certificates()` 内部构造的是同一个东西。
///
/// 改这个类型的时候请想清楚：任何一处提前返回 `Ok`，都会让默认档静默失去校验。
#[derive(Debug)]
struct ObservingVerifier {
    inner: Arc<dyn rustls::client::danger::ServerCertVerifier>,
    observer: Arc<CertObserver>,
}

impl rustls::client::danger::ServerCertVerifier for ObservingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // 先记录再判定：失败路径才是最需要拿到指纹的那一条。
        self.observer.record(end_entity.as_ref());
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// 公共 CA 校验不过时，回退比对用户指定的证书指纹。
///
/// **是并集，不是纯指纹**，这一点是刻意的：中间档的目的是让一个连不上的
/// 合法部署连上，不是收紧。纯指纹模式下，服务端换成 Let's Encrypt 证书之后
/// 用户会被自己锁在门外——手里拿着一张完全合法的证书却连不上，
/// 而那个体验会把他推回「允许不安全连接」，正好是这一整轮要消灭的路径。
///
/// 安全性上这不是削弱：对固定的一台服务器，"钉住这张证书"比
/// "接受任意公共 CA 签发的任意证书"更强。
#[derive(Debug)]
struct PinnedCertVerifier {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
    pins: Vec<[u8; 32]>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        match self
            .inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            Ok(v) => Ok(v),
            Err(e) => {
                let fp: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
                // 定长比较，不走 == 之外的早退分支。指纹不是秘密（它公开可得），
                // 所以这里不需要恒定时间比较，但也没理由写得更花哨。
                if self.pins.iter().any(|p| p == &fp) {
                    // 指纹命中后**刻意**不再补校验域名与有效期。
                    // 这一档服务的正是「证书上的名字对不上」和「自签证书过期了
                    // 但密钥没换」这两种情形——在这里把那两项检查加回来，
                    // 整档就失去存在意义了。
                    return Ok(rustls::client::danger::ServerCertVerified::assertion());
                }
                Err(e)
            }
        }
    }

    // 签名校验一律走真实实现。
    //
    // 这不是可选项：**跳过签名校验的话，指纹就完全失去意义**——
    // 任何人都能出示一份从别处抄来的证书副本，指纹当然对得上，
    // 而他并不持有对应的私钥。是签名校验在证明"出示者确实拥有这张证书"。
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// 跳过服务器证书校验的 verifier。
///
/// `verify_server_cert` 无条件返回成功：它不看证书链、不看域名、不看有效期。
/// 后果要说清楚——启用之后，任何位于中间的人只要出示一张自签证书就能接管
/// 这条连接，读走经由它传输的剪贴板明文与文件字节。TLS 仍在加密，但加密的
/// 对端是谁不再有任何保证。
///
/// 下面两个签名校验函数是真的——握手本身仍需自洽，
/// 只是「对方是不是你要找的那台服务器」不再被验证。
#[derive(Debug)]
pub struct InsecureServerCertVerifier(pub Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// 构造 WebSocket 的 TLS 连接器。
///
/// `observer` 只由控制面传入：它与信任档位正交，装不装都不影响判定，
/// 所以数据面传 `None` 不会造成两条链路信任集不一致。
///
/// **返回 `None` 的含义**是"交给 tokio-tungstenite 自己建 ClientConfig"。
/// 改造前默认档一律走这条；现在只有「公共 CA 且不需要观察指纹」才走——
/// 数据面的两条连接正是这种情况，于是它们的行为与改造前逐字节相同。
///
/// 控制面因为要观察指纹而改走自建。等价性依据：上游那段（tls.rs:91-115）做的是
/// `RootCertStore::empty()` + `extend(webpki_roots::TLS_SERVER_ROOTS)` +
/// `ClientConfig::builder().with_root_certificates(..).with_no_client_auth()`，
/// 这里逐行一致，provider 同为进程默认的 ring，两边都不设 ALPN。
pub fn create_tls_connector(
    trust: &TlsTrustConfig,
    observer: Option<Arc<CertObserver>>,
) -> Option<tokio_tungstenite::Connector> {
    // 三档都要装 provider，即便默认档马上就 return None。
    //
    // 返回 None 之后由 tokio-tungstenite 自己构建 ClientConfig，而 rustls 0.23
    // 在依赖树里同时存在 ring 与 aws-lc-rs 时（本项目正是如此）无法自动选定
    // provider，会直接 panic 而不是返回 Err——发生在连接 actor 的 task 里，
    // 整个重连循环就此死掉。
    //
    // 今天 lib.rs 的 run() 开头已经装过一次，所以这行是冗余的；写在这里是因为
    // 默认档不该依赖一个远在别处的副作用才能不 panic。
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 默认档 + 不观察 = 完全维持原样，一行 ClientConfig 都不自己碰。
    if trust.mode == TlsTrustMode::PublicCa && observer.is_none() {
        return None;
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let base: Arc<dyn rustls::client::danger::ServerCertVerifier> = match trust.mode {
        TlsTrustMode::Insecure => Arc::new(InsecureServerCertVerifier(provider.clone())),
        TlsTrustMode::PublicCa => rustls::client::WebPkiServerVerifier::builder_with_provider(
            public_root_store().clone(),
            provider.clone(),
        )
        .build()
        .expect("webpki verifier from static roots"),
        TlsTrustMode::Pinned => Arc::new(PinnedCertVerifier {
            inner: rustls::client::WebPkiServerVerifier::builder_with_provider(
                public_root_store().clone(),
                provider.clone(),
            )
            .build()
            .expect("webpki verifier from static roots"),
            pins: trust.pins.clone(),
        }),
    };

    let verifier: Arc<dyn rustls::client::danger::ServerCertVerifier> = match observer {
        Some(observer) => Arc::new(ObservingVerifier {
            inner: base,
            observer,
        }),
        None => base,
    };

    let client_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("valid tls protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    Some(tokio_tungstenite::Connector::Rustls(Arc::new(client_config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use rustls::CertificateError as CE;
    use std::error::Error as _;
    use std::io;

    /// 造一个与 tokio-rustls 实际产出形状相同的错误：
    /// rustls::Error 被 io::Error 包住，再被 tungstenite 包成 Io。
    fn wrap(e: rustls::Error) -> tungstenite::Error {
        tungstenite::Error::Io(io::Error::new(io::ErrorKind::InvalidData, e))
    }

    #[test]
    fn rustls_error_survives_tungstenite_io_wrapping() {
        // 钉的是 downcast 机制本身：类型信息必须能穿过两层包装。
        let err = wrap(rustls::Error::InvalidCertificate(CE::UnknownIssuer));
        let f = classify(&err).expect("应识别为证书错误");
        assert_eq!(f.kind, CertFailureKind::UnknownIssuer);
    }

    #[test]
    fn source_chain_does_not_expose_rustls_error() {
        // 这条测试看着怪，但它锁住的是一个反直觉的事实：source() 链在
        // io::Error 这一层就断了。没有它，后人很容易把 get_ref()+downcast
        // "简化"成 source() 链，而那会让所有分类静默失效。
        let err = wrap(rustls::Error::InvalidCertificate(CE::UnknownIssuer));
        let via_source = err.source().and_then(|s| s.source());
        assert!(
            via_source.is_none(),
            "source() 链居然通了——若 rustls 加了 source()，可以简化 extract_rustls_error"
        );
    }

    #[test]
    fn name_mismatch_carries_both_names() {
        let err = wrap(rustls::Error::InvalidCertificate(
            CE::NotValidForNameContext {
                expected: ServerName::try_from("120.26.54.84").unwrap().to_owned(),
                // 注意这里刻意用 webpki 的真实格式，不是裸域名——
                // 早先这条测试用裸字符串，正因如此漏掉了 DnsName(..) 包装。
                presented: vec![r#"DnsName("www.leafun.xyz")"#.to_string()],
            },
        ));
        let f = classify(&err).expect("应识别为证书错误");
        assert_eq!(
            f.kind,
            CertFailureKind::NameMismatch {
                expected: "120.26.54.84".to_string(),
                // 进来时是 DnsName("..")，分类后必须已经是干净名字
                presented: vec!["www.leafun.xyz".to_string()],
            }
        );

        let all = format!("{} {}", f.title, f.detail.join(" "));
        assert!(all.contains("120.26.54.84"), "缺少实连地址: {all}");
        assert!(all.contains("www.leafun.xyz"), "缺少证书名称: {all}");

        // 本次改造的核心诉求，反向钉死：证书合法时绝不能引导用户去关校验。
        assert!(
            !all.contains("允许不安全连接"),
            "名字不匹配时不得建议关闭证书校验: {all}"
        );
    }

    #[test]
    fn name_mismatch_without_context_still_advises_address_change() {
        let err = wrap(rustls::Error::InvalidCertificate(CE::NotValidForName));
        let f = classify(&err).expect("应识别为证书错误");
        let all = format!("{} {}", f.title, f.detail.join(" "));
        assert!(all.contains("服务器地址"), "应引导改地址: {all}");
        assert!(!all.contains("允许不安全连接"));
    }

    #[test]
    fn san_debug_wrapper_is_stripped() {
        // webpki 给的是 Debug 格式，直接显示会让用户看到 DnsName("...")。
        assert_eq!(humanize_san(r#"DnsName("www.leafun.xyz")"#), "www.leafun.xyz");
        assert_eq!(humanize_san("IpAddress(1.2.3.4)"), "1.2.3.4");
        assert_eq!(
            humanize_san(r#"UniformResourceIdentifier("https://x/")"#),
            "https://x/"
        );
        // 认不出的原样返回，不吃信息
        assert_eq!(humanize_san("Unsupported(0x07)"), "Unsupported(0x07)");
        assert_eq!(humanize_san("DirectoryName"), "DirectoryName");
    }

    #[test]
    fn many_presented_names_are_folded() {
        let names: Vec<String> = (0..9)
            .map(|i| format!(r#"DnsName("h{i}.example.com")"#))
            .collect();
        let err = wrap(rustls::Error::InvalidCertificate(
            CE::NotValidForNameContext {
                expected: ServerName::try_from("other.example.com").unwrap().to_owned(),
                presented: names,
            },
        ));
        let f = classify(&err).unwrap();
        let joined = f.detail.join(" ");
        assert!(joined.contains("等 9 个"), "SAN 列表未折叠: {joined}");
    }

    #[test]
    fn unsupported_cert_version_detected_from_other() {
        #[derive(Debug)]
        struct WebpkiLike;
        impl std::fmt::Display for WebpkiLike {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "UnsupportedCertVersion")
            }
        }
        impl std::error::Error for WebpkiLike {}

        let err = wrap(rustls::Error::InvalidCertificate(CE::Other(
            rustls::OtherError(Arc::new(WebpkiLike)),
        )));
        let f = classify(&err).expect("应识别为证书错误");
        assert_eq!(f.kind, CertFailureKind::UnsupportedVersion);
        // 关校验救不回来这一档，提示必须说清，否则用户会反复试错。
        assert!(f.detail.join(" ").contains("无效"));
    }

    #[test]
    fn expired_and_not_yet_valid_are_distinguished() {
        let e = classify(&wrap(rustls::Error::InvalidCertificate(CE::Expired))).unwrap();
        assert_eq!(e.kind, CertFailureKind::Expired);

        let n = classify(&wrap(rustls::Error::InvalidCertificate(CE::NotValidYet))).unwrap();
        assert_eq!(n.kind, CertFailureKind::NotYetValid);
        // 时钟跑偏比证书本身有问题常见得多，提示要先指向这一点。
        assert!(n.detail.join(" ").contains("系统时间"));
    }

    #[test]
    fn non_cert_errors_are_not_classified() {
        let io_err = tungstenite::Error::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert!(classify(&io_err).is_none());

        let proto = tungstenite::Error::Protocol(
            tungstenite::error::ProtocolError::HandshakeIncomplete,
        );
        assert!(classify(&proto).is_none());
    }

    /// 端到端：真起一个自签 TLS 监听器，真握一次手，验证分类活着。
    ///
    /// **上面那些用手工构造的错误跑的测试，证明不了这一条。** 它们自己造
    /// `rustls::Error` 再自己 downcast，两头用的必然是同一个类型；
    /// 而真实链路里错误是 `tokio-rustls` 造的，downcast 是本包做的，
    /// 中间隔着一次「两处 rustls 必须解析成同一个 crate 实例」的赌注。
    /// 那正是分类唯一可能静默失效的地方——依赖树一分裂，前面九条测试
    /// 全绿，而用户那边的提示悄悄退回通用文案。
    ///
    /// 所以这条测试钉的不是分类逻辑，是**依赖树的形状**。
    #[tokio::test]
    async fn classify_survives_real_handshake() {
        use tokio::io::AsyncWriteExt;
        use tokio_rustls::TlsAcceptor;

        let _ = rustls::crypto::ring::default_provider().install_default();

        // 自签证书 → 客户端用公共根校验必然是 UnknownIssuer。
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = ck.cert.der().clone();
        let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
            ck.signing_key.serialize_der().into(),
        );

        let server_cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            // 握手会被客户端拒掉，这里 accept 失败是预期的，不要 unwrap。
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    let _ = tls.shutdown().await;
                }
            }
        });

        let url = format!("wss://localhost:{port}/ws/control");
        let err = tokio_tungstenite::connect_async_tls_with_config(
            &url,
            None,
            false,
            // 默认档：交给 tokio-tungstenite 用 webpki 根校验，正是用户走的那条路。
            create_tls_connector(&TlsTrustConfig::public_ca(), None),
        )
        .await
        .expect_err("自签证书必须握手失败");

        let failure = classify(&err).unwrap_or_else(|| {
            panic!("真实握手错误没被识别为证书错误，原始错误：{err}")
        });
        assert_eq!(
            failure.kind,
            CertFailureKind::UnknownIssuer,
            "真实握手错误分类失败——多半是依赖树里出现了两份 rustls。原始错误：{err}"
        );
    }

    /// 起一个本地自签 TLS 监听器，返回 (端口, 证书 DER)。
    ///
    /// 用真握手而不是直接调 verifier，是因为这一档真正要证明的是
    /// 「自签服务器能连上」这个端到端结果，而不是某个函数的返回值。
    async fn spawn_self_signed_server(san: &str) -> (u16, Vec<u8>) {
        use tokio::io::AsyncWriteExt;
        use tokio_rustls::TlsAcceptor;

        let _ = rustls::crypto::ring::default_provider().install_default();

        let ck = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
        let cert_der = ck.cert.der().clone();
        let cert_bytes = cert_der.to_vec();
        let key_der =
            rustls::pki_types::PrivateKeyDer::Pkcs8(ck.signing_key.serialize_der().into());

        let server_cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(stream).await {
                        let _ = tls.shutdown().await;
                    }
                });
            }
        });

        (port, cert_bytes)
    }

    fn fingerprint_of(der: &[u8]) -> String {
        hex::encode(Sha256::digest(der))
    }

    /// 一次本地握手的结论。
    ///
    /// 必须同时带上 `fingerprint`：只看「有没有证书错误」是不够的——
    /// TCP 没连上时同样没有证书错误，于是「根本没握手」会被记成「校验通过」。
    /// 指纹有值才证明服务端确实出示了证书、校验逻辑确实跑过。
    /// live 测试里已经栽过一次，本地用例不该留着同一个形状。
    struct LocalOutcome {
        cert_error: Option<CertFailureKind>,
        fingerprint: Option<String>,
        raw: Option<String>,
    }

    impl LocalOutcome {
        fn assert_cert_accepted(&self, what: &str) {
            assert!(
                self.fingerprint.is_some(),
                "{what}：服务端没出示证书，握手没走到校验那一步（raw={:?}）——\
                 这不是通过，是没连上",
                self.raw
            );
            assert!(
                self.cert_error.is_none(),
                "{what}：证书校验被拒（{:?}），raw={:?}",
                self.cert_error,
                self.raw
            );
        }

        fn assert_cert_rejected(&self, expect: CertFailureKind, what: &str) {
            assert!(
                self.fingerprint.is_some(),
                "{what}：没连上，raw={:?}",
                self.raw
            );
            assert_eq!(self.cert_error, Some(expect), "{what}：raw={:?}", self.raw);
        }
    }

    async fn try_connect(port: u16, trust: &TlsTrustConfig) -> LocalOutcome {
        let observer = CertObserver::new();
        let url = format!("wss://localhost:{port}/ws/control");
        let res = tokio_tungstenite::connect_async_tls_with_config(
            &url,
            None,
            false,
            create_tls_connector(trust, Some(observer.clone())),
        )
        .await;

        // 服务端握手后随即关闭，WebSocket 升级失败是预期的，
        // 而那已经是 TLS 之上的事——这里只关心证书这一关。
        let err = res.err();
        LocalOutcome {
            cert_error: err.as_ref().and_then(classify).map(|f| f.kind),
            fingerprint: observer.observed(),
            raw: err.map(|e| e.to_string()),
        }
    }

    #[tokio::test]
    async fn pinned_mode_accepts_matching_self_signed_cert() {
        let (port, cert) = spawn_self_signed_server("localhost").await;

        // 默认档连不上——自签证书不在公共根里。这是对照组，
        // 没有它就无法确认下面那次成功真的是指纹起的作用。
        try_connect(port, &TlsTrustConfig::public_ca())
            .await
            .assert_cert_rejected(CertFailureKind::UnknownIssuer, "默认档 + 自签证书");

        let trust =
            TlsTrustConfig::new(TlsTrustMode::Pinned, &[fingerprint_of(&cert)]).unwrap();
        try_connect(port, &trust)
            .await
            .assert_cert_accepted("指纹档 + 匹配的自签证书");
    }

    #[tokio::test]
    async fn pinned_mode_rejects_other_cert() {
        let (port, _) = spawn_self_signed_server("localhost").await;
        // 另造一张证书的指纹：形状完全合法，只是不是这一台的。
        let (_, other_cert) = spawn_self_signed_server("localhost").await;

        let trust =
            TlsTrustConfig::new(TlsTrustMode::Pinned, &[fingerprint_of(&other_cert)]).unwrap();
        try_connect(port, &trust).await.assert_cert_rejected(
            CertFailureKind::UnknownIssuer,
            "指纹不匹配时应仍卡在公共 CA 那一关，而不是被指纹放行后另生枝节",
        );
    }

    #[tokio::test]
    async fn observer_records_fingerprint_even_when_verification_fails() {
        // 失败路径才是最需要指纹的那一条：用户要拿它跟服务器上的证书核对。
        let (port, cert) = spawn_self_signed_server("localhost").await;
        let observer = CertObserver::new();

        let url = format!("wss://localhost:{port}/ws/control");
        let _ = tokio_tungstenite::connect_async_tls_with_config(
            &url,
            None,
            false,
            create_tls_connector(&TlsTrustConfig::public_ca(), Some(observer.clone())),
        )
        .await;

        assert_eq!(
            observer.observed().as_deref(),
            Some(fingerprint_of(&cert).as_str()),
            "握手失败时也必须记下实际看到的证书指纹"
        );
    }

    #[tokio::test]
    async fn observing_verifier_does_not_weaken_default_mode() {
        // 装了观察器的默认档，判定必须与不装时逐字一致——
        // 它用的是 .dangerous() 那个 API，很容易被误读成"放宽了"。
        let (port, _) = spawn_self_signed_server("localhost").await;

        let url = format!("wss://localhost:{port}/ws/control");
        let err = tokio_tungstenite::connect_async_tls_with_config(
            &url,
            None,
            false,
            create_tls_connector(&TlsTrustConfig::public_ca(), Some(CertObserver::new())),
        )
        .await
        .expect_err("装了观察器也不能放行自签证书");
        assert_eq!(
            classify(&err).map(|f| f.kind),
            Some(CertFailureKind::UnknownIssuer)
        );
    }

    #[test]
    fn fingerprint_normalization_accepts_openssl_output() {
        let want = [0xABu8; 32];
        let hexed = hex::encode(want);

        assert_eq!(normalize_fingerprint(&hexed).unwrap(), want);
        assert_eq!(normalize_fingerprint(&hexed.to_uppercase()).unwrap(), want);

        let colonized = hexed
            .as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join(":");
        assert_eq!(normalize_fingerprint(&colonized).unwrap(), want);
        assert_eq!(
            normalize_fingerprint(&format!("SHA256 Fingerprint={colonized}")).unwrap(),
            want
        );

        // 长度错了要报长度，而不是丢一句"不是合法十六进制"——
        // 少粘一个字节是最常见的手误。
        let err = normalize_fingerprint("ab:cd").unwrap_err();
        assert!(err.contains("64"), "报错应指出长度要求: {err}");
    }

    #[test]
    fn public_ca_mode_ignores_pins() {
        // 填了指纹但档位是 PublicCa：指纹不该悄悄生效。
        let trust = TlsTrustConfig::new(TlsTrustMode::PublicCa, &[hex::encode([0u8; 32])]).unwrap();
        assert_eq!(trust.mode, TlsTrustMode::PublicCa);
        assert!(!trust.skips_verification());
    }

    // ───────────────────────────────────────────────────────────────
    // 真机验证。默认 `cargo test` 不跑（需要外网），用
    // `cargo test -- --ignored --nocapture live_` 手动执行。
    //
    // 存在的理由：上面所有测试用的都是本地自签证书，走的全是**失败**路径。
    // 而自建 ClientConfig 改的是**所有用户都在走的成功路径**——
    // 「默认档仍然连得上一张正常的公共 CA 证书」这件事，本地自签测不出来。
    //
    // **断言必须以 observer 有值为前提。** 第一版写成「握手报错但 classify
    // 返回 None 就算通过」，那是假的：TCP 连不上时 classify 同样返回 None，
    // 于是「根本没连上」被当成了「证书校验通过」。实测确实踩中——
    // 本机 DNS 走 fake-ip 代理，域名解析出 198.18.3.51，Rust 直连必然失败，
    // 而那条测试当时显示为绿。observer 有值才证明服务器真的出示了证书、
    // 校验逻辑真的跑过。
    // ───────────────────────────────────────────────────────────────

    /// 一次真实握手的结果：证书这一关过没过，以及实际看到的指纹。
    struct LiveOutcome {
        cert_error: Option<CertFailureKind>,
        fingerprint: Option<String>,
        raw: Option<String>,
    }

    async fn live_probe(addr: &str, port: u16, trust: &TlsTrustConfig) -> LiveOutcome {
        let observer = CertObserver::new();
        let url = format!("wss://{addr}:{port}/ws/control");
        let res = tokio_tungstenite::connect_async_tls_with_config(
            &url,
            None,
            false,
            create_tls_connector(trust, Some(observer.clone())),
        )
        .await;

        let err = res.err();
        LiveOutcome {
            cert_error: err.as_ref().and_then(classify).map(|f| f.kind),
            fingerprint: observer.observed(),
            raw: err.map(|e| e.to_string()),
        }
    }

    impl LiveOutcome {
        /// 证书这一关确实跑过并且通过了。
        ///
        /// WebSocket 升级失败不算数——那已经是 TLS 之上的事。
        fn assert_cert_accepted(&self, what: &str) {
            assert!(
                self.fingerprint.is_some(),
                "{what}：服务器没出示证书，握手没走到校验那一步（raw={:?}）。\
                 这不是「通过」，是没连上——检查网络 / DNS / 代理",
                self.raw
            );
            assert!(
                self.cert_error.is_none(),
                "{what}：证书校验被拒（{:?}），raw={:?}",
                self.cert_error,
                self.raw
            );
        }
    }

    /// 最高风险的一条：自建 ClientConfig 之后，默认档还认不认公共 CA 的证书。
    ///
    /// 用 badssl.com 而不是自己的部署：它 DNS 正常、证书正常，
    /// 且不会因为某台机器的代理配置而变成假阳性。
    #[tokio::test]
    #[ignore = "需要外网"]
    async fn live_public_ca_accepts_normal_public_cert() {
        let out = live_probe("sha256.badssl.com", 443, &TlsTrustConfig::public_ca()).await;
        out.assert_cert_accepted("默认档 + 正常公共 CA 证书");
    }

    /// 默认档必须拒绝过期证书——自建之后这条不能松。
    #[tokio::test]
    #[ignore = "需要外网"]
    async fn live_public_ca_rejects_expired_cert() {
        let out = live_probe("expired.badssl.com", 443, &TlsTrustConfig::public_ca()).await;
        assert!(out.fingerprint.is_some(), "没连上，raw={:?}", out.raw);
        assert_eq!(out.cert_error, Some(CertFailureKind::Expired));
    }

    /// 事故现场的形态：证书有效，但签发给了别的名字。
    #[tokio::test]
    #[ignore = "需要外网"]
    async fn live_public_ca_reports_name_mismatch() {
        let out = live_probe("wrong.host.badssl.com", 443, &TlsTrustConfig::public_ca()).await;
        assert!(out.fingerprint.is_some(), "没连上，raw={:?}", out.raw);
        match &out.cert_error {
            Some(CertFailureKind::NameMismatch { expected, presented }) => {
                assert_eq!(expected, "wrong.host.badssl.com");
                // 真机才能验到的一点：这里必须已经是干净名字，
                // 不能是 webpki 原样给的 DnsName("...")
                assert!(
                    presented.iter().all(|n| !n.starts_with("DnsName(")),
                    "SAN 的 Debug 包装没剥掉：{presented:?}"
                );
                assert!(!presented.is_empty());
            }
            other => panic!("分类错了：{other:?}，raw={:?}", out.raw),
        }
    }

    /// 自签证书在默认档下被拒，在填对指纹后放行。
    #[tokio::test]
    #[ignore = "需要外网"]
    async fn live_pinned_accepts_self_signed_by_fingerprint() {
        let host = "self-signed.badssl.com";

        // 默认档：拒绝，同时把指纹给出来（这正是用户拿指纹的路径）
        let out = live_probe(host, 443, &TlsTrustConfig::public_ca()).await;
        assert_eq!(
            out.cert_error,
            Some(CertFailureKind::UnknownIssuer),
            "raw={:?}",
            out.raw
        );
        let fp = out.fingerprint.expect("失败路径也必须拿到指纹");

        // 填对指纹：放行
        let trust = TlsTrustConfig::new(TlsTrustMode::Pinned, std::slice::from_ref(&fp)).unwrap();
        live_probe(host, 443, &trust)
            .await
            .assert_cert_accepted("中间档 + 正确指纹");

        // 指纹错一位就必须拒绝，否则这一档等于没校验
        let mut wrong = fp.clone();
        let last = wrong.pop().unwrap();
        wrong.push(if last == '0' { '1' } else { '0' });
        let bad_trust = TlsTrustConfig::new(TlsTrustMode::Pinned, &[wrong]).unwrap();
        let out = live_probe(host, 443, &bad_trust).await;
        assert_eq!(
            out.cert_error,
            Some(CertFailureKind::UnknownIssuer),
            "错误指纹被放行了"
        );
    }

    /// 不校验档：自签证书也照连不误。
    #[tokio::test]
    #[ignore = "需要外网"]
    async fn live_insecure_accepts_anything() {
        let trust = TlsTrustConfig::new(TlsTrustMode::Insecure, &[]).unwrap();
        live_probe("self-signed.badssl.com", 443, &trust)
            .await
            .assert_cert_accepted("不校验档 + 自签证书");
    }

    #[test]
    fn insecure_mode_still_produces_certificate_errors() {
        // 钉的是那道已被删掉的守卫所依赖的错误假定。
        //
        // 守卫写的是「Insecure 档就别再报证书错误了」，前提是这一档下不会有
        // 证书错误。实际上 InsecureServerCertVerifier 只放行链、域名、有效期三项，
        // 证书**解析**照旧要做（rustls 在签名校验里调 EndEntityCert::try_from）。
        // 所以 X.509 v1 这类错误在 Insecure 档下照样冒出来，而它们恰恰是
        // 关掉校验也救不回来、最需要告诉用户的那一类。
        //
        // 这两条断言合起来说明：一旦按 skips_verification() 过滤，
        // 这个错误就会被静默吞掉，用户只剩无提示的重连。
        #[derive(Debug)]
        struct V1Like;
        impl std::fmt::Display for V1Like {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "UnsupportedCertVersion")
            }
        }
        impl std::error::Error for V1Like {}

        let err = wrap(rustls::Error::InvalidCertificate(CE::Other(
            rustls::OtherError(Arc::new(V1Like)),
        )));
        let failure = classify(&err).expect("Insecure 档下这个错误依然存在");
        assert_eq!(failure.kind, CertFailureKind::UnsupportedVersion);

        let insecure = TlsTrustConfig::new(TlsTrustMode::Insecure, &[]).unwrap();
        assert!(
            insecure.skips_verification(),
            "若这里为 false，说明档位语义变了，本测试的前提需要重写"
        );
    }

    #[test]
    fn non_pinned_modes_tolerate_leftover_malformed_fingerprints() {
        // 拆的是一个死结：设置面板只在 Pinned 档显示指纹输入框、也只在那一档
        // 校验格式。若非 Pinned 档仍解析指纹，用户「填错 → 切回公共 CA → 保存」
        // 会被后端拒掉，而那时输入框已经折叠不可见，他没有任何修正入口。
        let junk = vec![
            "not-a-fingerprint".to_string(),
            "还没填完的半截".to_string(),
        ];

        for mode in [TlsTrustMode::PublicCa, TlsTrustMode::Insecure] {
            let trust = TlsTrustConfig::new(mode, &junk)
                .unwrap_or_else(|e| panic!("{mode:?} 档不该因残留指纹而拒绝保存：{e}"));
            assert_eq!(trust.mode, mode);
            // 残留文本绝不能在这一档里悄悄变成信任来源
            assert!(trust.pins.is_empty());
        }

        // Pinned 档仍然必须严格校验——放宽的只是另外两档
        assert!(TlsTrustConfig::new(TlsTrustMode::Pinned, &junk).is_err());
    }

    #[test]
    fn untrusted_san_text_is_sanitized() {
        // SAN 由出示证书的一方决定，而它会显示在用户照着做处置的横幅上。
        // React 转义了 HTML，所以这里防的不是注入，是视觉欺骗。

        // 双向覆写：能把后面的文本反着渲染，伪装成别的域名
        let rlo = humanize_san("DnsName(\"evil\u{202E}moc.kcatta\")");
        assert!(!rlo.contains('\u{202E}'), "Bidi 覆写未被清洗: {rlo:?}");
        assert!(rlo.contains('\u{FFFD}'), "应留下可见痕迹: {rlo:?}");

        // 控制字符：可以伪造换行、拼出形似系统提示的片段
        let ctrl = humanize_san("DnsName(\"a\rb\nc\t d\")");
        assert!(
            !ctrl.chars().any(|c| c.is_control()),
            "控制字符未被清洗: {ctrl:?}"
        );

        // 超长名字：不截断会把横幅撑开，把后面的内容挤出视野
        let long = humanize_san(&format!("DnsName(\"{}\")", "a".repeat(500)));
        assert!(long.chars().count() <= MAX_NAME_LEN + 1, "未截断: {}", long.chars().count());
        assert!(long.ends_with('…'));

        // 正常名字不受影响——清洗不能顺手把可读性也洗掉
        assert_eq!(humanize_san("DnsName(\"www.leafun.xyz\")"), "www.leafun.xyz");
        assert_eq!(humanize_san("IpAddress(1.2.3.4)"), "1.2.3.4");
    }

    #[test]
    fn raw_error_string_is_sanitized_but_keeps_detail() {
        // raw 是分类出错时唯一的现场，清洗要比名字宽松，但同样不能放过控制符。
        let err = tungstenite::Error::Io(io::Error::other(
            "invalid peer certificate: \u{202E}spoofed\u{0007}",
        ));
        let f = classify(&err).expect("应识别为证书错误");
        assert!(!f.raw.contains('\u{202E}'));
        assert!(!f.raw.contains('\u{0007}'));
        // 有用的部分必须留着
        assert!(f.raw.contains("invalid peer certificate"));
    }

    #[test]
    fn name_mismatch_requires_out_of_band_check_before_changing_address() {
        // 这条钉的是一个真实的攻击路径：在位的中间人可以出示一张**自己域名的
        // 合法公共 CA 证书**，从而触发 NameMismatch。如果横幅紧接着说
        // 「把地址改成证书上的域名」，就等于替攻击者把用户请进去。
        //
        // 「证书由公共 CA 签发」只证明对方拥有那个域名，不证明那台机器是用户的。
        let err = wrap(rustls::Error::InvalidCertificate(
            CE::NotValidForNameContext {
                expected: ServerName::try_from("myserver.example").unwrap().to_owned(),
                presented: vec![r#"DnsName("attacker.example")"#.to_string()],
            },
        ));
        let f = classify(&err).unwrap();
        let all = f.detail.join(" ");

        assert!(
            all.contains("确认") && all.contains("你自己的服务器"),
            "缺少带外核对前提，这条指引本身就成了攻击面: {all}"
        );
        assert!(
            all.contains("中间人"),
            "应点明域名不认识时的风险: {all}"
        );
        // 原有承诺不能因此丢掉：仍然不得引导用户关校验
        assert!(!all.contains("允许不安全连接"));

        // 核对必须排在「改地址」之前，否则用户读到前半句就动手了
        let check_at = all.find("确认").expect("应有核对要求");
        let change_at = all.find("改为证书上的域名").expect("应有改地址指引");
        assert!(check_at < change_at, "核对要求必须排在改地址指引之前");
    }

    #[test]
    fn unclassifiable_cert_text_falls_back_to_other() {
        // downcast 拿不到类型时（rustls 版本分裂的情形）走文本兜底。
        let err = tungstenite::Error::Io(io::Error::other(
            "invalid peer certificate: something brand new",
        ));
        let f = classify(&err).expect("文本兜底应识别为证书错误");
        assert_eq!(f.kind, CertFailureKind::Other);
        // 原始错误必须原样带走——分类失败时这是唯一的现场。
        assert!(f.raw.contains("something brand new"));
    }
}
