use std::sync::Arc;
use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::{mpsc, Notify, RwLock};
use tauri::Emitter;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::protocol::{ActionType, AuthChallengePayload, AuthRequestPayload, AuthResponsePayload, ControlEnvelope};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub server_url: String, // e.g. "ws://127.0.0.1:8080"
    pub account_id: String,
    pub device_id: String,
    pub psk_secret: String,
    pub hostname: String,
    pub os_type: String,
    pub app_version: String,

    /// 跳过 TLS 服务器证书校验。默认 false。
    ///
    /// 见 `create_tls_connector`：打开它就等于接受任何出示证书的中间人。
    pub allow_insecure_tls: bool,
}

/// 一个**不校验服务器身份**的证书校验器。
///
/// `verify_server_cert` 无条件返回成功：它不看证书链、不看域名、不看有效期。
/// 后果要说清楚——启用之后，任何位于中间的人只要出示一张自签证书就能接管
/// 这条连接，读走经由它传输的剪贴板明文与文件字节。TLS 仍在加密，但加密的
/// 对端是谁不再有任何保证。
///
/// 它只在用户显式勾选「允许不安全连接」时才被装上，用于自签证书、IP 直连、
/// 或证书由非公共 CA 签发的内网部署。下面两个签名校验函数是真的——
/// 握手本身仍需自洽，只是「对方是不是你要找的那台服务器」不再被验证。
///
/// 改名自 `CustomServerCertVerifier`：原名听起来像是某种定制策略，
/// 而它实际做的事只有「跳过」。
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
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// 构造 WebSocket 的 TLS 连接器。
///
/// `allow_insecure = false`（默认）时返回 `None`，由 tokio-tungstenite 走它
/// 自带的 webpki 根证书校验。刻意**不**在这里手搓 `ClientConfig`：
/// `rustls-tls-webpki-roots` 是 tokio-tungstenite 的 feature，它不会把
/// `webpki_roots` 这个 crate 注入本包，自己构造就得额外声明一条依赖，
/// 而交给它自己加载既省依赖、也少一处可能配错的地方。
///
/// `allow_insecure = true` 时才装上 `InsecureServerCertVerifier`——
/// 读那个类型上的注释，它说明了代价。
///
/// 改造前这里**无条件**装载那个跳过校验的 verifier，也就是说即便连的是
/// `wss://`，中间人也照样能接管连接。默认走真实校验是本轮的目的之一。
pub fn create_tls_connector(allow_insecure: bool) -> Option<tokio_tungstenite::Connector> {
    if !allow_insecure {
        return None;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client_config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("valid tls protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier(provider)))
        .with_no_client_auth();

    Some(tokio_tungstenite::Connector::Rustls(Arc::new(client_config)))
}

/// 判断一个 tungstenite 错误是否为 TLS 证书校验失败。
///
/// rustls 把证书问题归到 `rustls::Error::InvalidCertificate`，经 tungstenite
/// 包装后只剩下 IO/Tls 层的字符串，因此这里按错误文本匹配。这不优雅，也不够
/// 稳固——rustls 改文案就会漏判——但漏判的后果只是退回到通用的「连接失败」
/// 提示，不会误导用户去关掉校验，所以这个不精确是可接受的方向。
fn is_cert_error(err: &tokio_tungstenite::tungstenite::Error) -> bool {
    let text = err.to_string();
    text.contains("certificate")
        || text.contains("CertificateError")
        || text.contains("UnknownIssuer")
        || text.contains("NotValidForName")
        || text.contains("invalid peer certificate")
}

pub struct ConnectionActor {
    config: Arc<RwLock<ConnectionConfig>>,
    reconnect_notify: Arc<Notify>,
    outgoing_rx: mpsc::Receiver<ControlEnvelope>,
}

impl ConnectionActor {
    pub fn new(
        config: Arc<RwLock<ConnectionConfig>>,
        reconnect_notify: Arc<Notify>,
    ) -> (Self, mpsc::Sender<ControlEnvelope>) {
        let (out_tx, out_rx) = mpsc::channel(128);

        let actor = Self {
            config,
            reconnect_notify,
            outgoing_rx: out_rx,
        };

        (actor, out_tx)
    }

    /// Generates canonical signature: "UNIDROP_V1\n{account_id}\n{device_id}\n{nonce}\n{timestamp_ms}\n{nonce_salt}"
    pub fn compute_signature(secret: &str, account_id: &str, device_id: &str, nonce: &str, timestamp: i64, nonce_salt: &str) -> String {
        let canonical = if nonce_salt.is_empty() {
            format!("UNIDROP_V1\n{}\n{}\n{}\n{}", account_id, device_id, nonce, timestamp)
        } else {
            format!("UNIDROP_V1\n{}\n{}\n{}\n{}\n{}", account_id, device_id, nonce, timestamp, nonce_salt)
        };
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(canonical.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Background loop with exponential backoff and jitter, with instant wakeup on configuration change.
    ///
    /// `app_handle` 只用于一件事：把 TLS 证书校验失败直接报到界面上。
    /// 它必须走这条路而不是既有的 `incoming_tx`——握手失败发生在 WebSocket
    /// 建立**之前**，此时根本不存在可以塞进通道的 ControlEnvelope。
    ///
    /// 也考虑过合成一条 AUTH_RESPONSE 丢进 incoming_tx 来复用 `auth-failed`，
    /// 但那是把传输层错误伪装成鉴权失败：事件名会就此名不副实，
    /// lib.rs 里那段 AUTH_RESPONSE 处理逻辑（它还负责存服务端限额）也会被污染。
    pub async fn run(mut self, incoming_tx: mpsc::Sender<ControlEnvelope>, app_handle: tauri::AppHandle) {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(30);

        loop {
            // 0. Read latest configuration
            let current_cfg = {
                let cfg = self.config.read().await;
                cfg.clone()
            };

            let clean_url: String = current_cfg.server_url.chars().filter(|c| !c.is_whitespace()).collect();
            let base = clean_url.trim().trim_end_matches('/');
            let ws_url = if base.starts_with("ws://") || base.starts_with("wss://") {
                format!("{}/ws/control", base)
            } else {
                format!("wss://{}/ws/control", base)
            };
            log::info!("Connecting to control server: {}", ws_url);

            let connector = create_tls_connector(current_cfg.allow_insecure_tls);
            match tokio_tungstenite::connect_async_tls_with_config(&ws_url, None, false, connector).await {
                Ok((ws_stream, _)) => {
                    log::info!("Connected to control server");
                    backoff = Duration::from_secs(1); // Reset backoff

                    let (mut write, mut read) = ws_stream.split();

                    // 1. Wait for AUTH_CHALLENGE (or reconnect request)
                    let mut authed = false;
                    let mut auth_rejected = false;

                    tokio::select! {
                        _ = self.reconnect_notify.notified() => {
                            log::info!("Reconnection requested while waiting for challenge, aborting handshake");
                            continue;
                        }
                        challenge_msg = read.next() => {
                            if let Some(Ok(Message::Text(text))) = challenge_msg {
                                if let Ok(env) = serde_json::from_str::<ControlEnvelope>(&text) {
                                    if env.action == ActionType::AUTH_CHALLENGE {
                                        let challenge = serde_json::from_value::<AuthChallengePayload>(env.payload).unwrap_or(AuthChallengePayload {
                                            nonce_salt: String::new(),
                                            server_time: 0,
                                        });

                                        // 2. Send AUTH_REQUEST with NonceSalt binding
                                        let now = current_time_ms();
                                        let nonce = Uuid::new_v4().to_string();
                                        let sig = Self::compute_signature(
                                            &current_cfg.psk_secret,
                                            &current_cfg.account_id,
                                            &current_cfg.device_id,
                                            &nonce,
                                            now,
                                            &challenge.nonce_salt,
                                        );

                                        let auth_payload = AuthRequestPayload {
                                            account_id: current_cfg.account_id.clone(),
                                            device_id: current_cfg.device_id.clone(),
                                            hostname: current_cfg.hostname.clone(),
                                            os_type: current_cfg.os_type.clone(),
                                            app_version: current_cfg.app_version.clone(),
                                            signature: sig,
                                            nonce,
                                            timestamp: now,
                                        };

                                        let auth_env = ControlEnvelope {
                                            version: 1,
                                            trace_id: Uuid::new_v4().to_string(),
                                            action: ActionType::AUTH_REQUEST,
                                            from_device: current_cfg.device_id.clone(),
                                            to_device: None,
                                            timestamp: now,
                                            payload: serde_json::to_value(auth_payload).unwrap(),
                                        };

                                        if write.send(Message::Text(serde_json::to_string(&auth_env).unwrap())).await.is_ok() {
                                            // 3. Wait for AUTH_RESPONSE
                                            if let Some(Ok(Message::Text(resp_text))) = read.next().await {
                                                if let Ok(resp_env) = serde_json::from_str::<ControlEnvelope>(&resp_text) {
                                                    if resp_env.action == ActionType::AUTH_RESPONSE {
                                                        if let Ok(resp_payload) = serde_json::from_value::<AuthResponsePayload>(resp_env.payload.clone()) {
                                                            if resp_payload.success {
                                                                authed = true;
                                                                let _ = incoming_tx.send(resp_env).await;
                                                            } else {
                                                                log::error!("Authentication failed: {:?}", resp_payload.error_message);
                                                                auth_rejected = true;
                                                                let _ = incoming_tx.send(resp_env).await;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if !authed {
                        if auth_rejected {
                            log::warn!("Authentication rejected by server (invalid credentials). Backing off for 30s or until settings change.");
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                                _ = self.reconnect_notify.notified() => {
                                    log::info!("New credentials received, reconnecting immediately");
                                    backoff = Duration::from_secs(1);
                                }
                            }
                        } else {
                            log::warn!("Authentication handshake unsuccessful, disconnecting");
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                                _ = self.reconnect_notify.notified() => {
                                    log::info!("Reconnection requested, reconnecting immediately");
                                    backoff = Duration::from_secs(1);
                                }
                            }
                        }
                        continue;
                    }

                    // 4. Heartbeat & Forwarding loop
                    let mut ping_interval = tokio::time::interval(Duration::from_secs(15));
                    loop {
                        tokio::select! {
                            _ = self.reconnect_notify.notified() => {
                                log::info!("Configuration changed, closing current session to reconnect with new config");
                                break;
                            }

                            _ = ping_interval.tick() => {
                                let ping_env = ControlEnvelope {
                                    version: 1,
                                    trace_id: Uuid::new_v4().to_string(),
                                    action: ActionType::HEARTBEAT_PING,
                                    from_device: current_cfg.device_id.clone(),
                                    to_device: None,
                                    timestamp: current_time_ms(),
                                    payload: serde_json::Value::Null,
                                };
                                if write.send(Message::Text(serde_json::to_string(&ping_env).unwrap())).await.is_err() {
                                    break;
                                }
                            }

                            Some(env) = self.outgoing_rx.recv() => {
                                let text = match serde_json::to_string(&env) {
                                    Ok(t) => t,
                                    Err(e) => {
                                        log::error!("failed to serialize outgoing envelope: {}", e);
                                        continue;
                                    }
                                };
                                if let Err(e) = write.send(Message::Text(text)).await {
                                    log::warn!("failed to send outgoing message to server: {}", e);
                                    break;
                                }
                            }

                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Ok(env) = serde_json::from_str::<ControlEnvelope>(&text) {
                                            let _ = incoming_tx.send(env).await;
                                        }
                                    }
                                    _ => break, // Connection closed
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    // 证书失败要单独报，否则用户只会看到反复重连，
                    // 完全无从知道是「证书不受信任」还是服务端没起来——
                    // 而这正是本轮默认开启校验之后，自签证书部署升级时的第一现场。
                    if is_cert_error(&err) && !current_cfg.allow_insecure_tls {
                        log::warn!("TLS certificate verification failed: {}", err);
                        let _ = app_handle.emit(
                            "tls-cert-failed",
                            format!(
                                "无法验证服务器证书：{}。\n\
                                 若服务端使用自签证书、直连 IP，或证书由非公共 CA 签发，\
                                 请在「设置」中勾选「允许不安全连接」。",
                                err
                            ),
                        );
                    } else {
                        log::warn!("Connection failed: {}, retrying...", err);
                    }
                }
            }

            // Exponential backoff with jitter, or instant wakeup if user changes settings
            let jitter = Duration::from_millis(fastrand_u64(0, 1000));
            tokio::select! {
                _ = tokio::time::sleep(backoff + jitter) => {
                    backoff = (backoff * 2).min(max_backoff);
                }
                _ = self.reconnect_notify.notified() => {
                    log::info!("Reconnection requested during backoff, connecting immediately");
                    backoff = Duration::from_secs(1);
                }
            }
        }
    }
}

pub fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn fastrand_u64(min: u64, max: u64) -> u64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().subsec_nanos() as u64;
    min + (nanos % (max - min + 1))
}


