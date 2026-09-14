use std::sync::Arc;
use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::{mpsc, Notify, RwLock};
use tauri::Emitter;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::core::tls_trust::{self, create_tls_connector};
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

    /// TLS 信任策略。默认只信任公共根证书库。
    ///
    /// 见 `tls_trust::create_tls_connector`：Insecure 档等于接受任何出示证书的中间人。
    pub tls_trust: tls_trust::TlsTrustConfig,
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

            // 控制面装证书观察器：证书失败时要把实际看到的指纹报给用户，
            // 让他能跟服务器上那张证书核对。数据面不装（见 create_tls_connector）。
            let observer = tls_trust::CertObserver::new();
            let connector = create_tls_connector(&current_cfg.tls_trust, Some(observer.clone()));
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
                    // 完全无从知道是证书问题还是服务端没起来。
                    //
                    // 发的是结构化的分类结果而不是一句拼好的话：处置动作因档而异，
                    // 拼字符串的做法逼着所有档共用一句建议，而那句建议
                    // （「勾选允许不安全连接」）对其中几档是错的。
                    //
                    // 不按档位过滤：**能走到这里的证书错误，都是用户绕不过的。**
                    // 早先这里有一道 `!skips_verification()` 守卫，原意是「用户自己
                    // 关掉的校验，不该再拿校验失败去打扰他」——听着合理，但它假定了
                    // Insecure 档下不会有证书错误，而那个假定不成立：
                    // InsecureServerCertVerifier 只放行链、域名、有效期三项，
                    // 证书**解析**仍然要做（rustls 在签名校验里调
                    // `EndEntityCert::try_from`）。X.509 v1、畸形 DER 之类照样报错，
                    // 而它们恰恰是关掉校验也救不回来的那一类。
                    // 守卫把它们一并吞了，用户关了校验反而连一句解释都看不到，
                    // 只剩无休止的重连——那正是他最需要知道原因的时刻。
                    match tls_trust::classify(&err) {
                        Some(mut failure) => {
                            failure.observed_cert_sha256 = observer.observed();
                            log::warn!(
                                "TLS certificate verification failed [{}]: {}",
                                failure.kind.tag(),
                                err
                            );
                            let _ = app_handle.emit("tls-cert-failed", &failure);
                        }
                        _ => log::warn!("Connection failed: {}, retrying...", err),
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


