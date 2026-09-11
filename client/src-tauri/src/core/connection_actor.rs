use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
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
}

pub struct ConnectionActor {
    config: ConnectionConfig,
    outgoing_rx: mpsc::Receiver<ControlEnvelope>,
}

impl ConnectionActor {
    pub fn new(config: ConnectionConfig) -> (Self, mpsc::Sender<ControlEnvelope>) {
        let (out_tx, out_rx) = mpsc::channel(128);

        let actor = Self {
            config,
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

    /// Background loop with exponential backoff and jitter.
    pub async fn run(mut self, incoming_tx: mpsc::Sender<ControlEnvelope>) {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(30);

        loop {
            let base = self.config.server_url.trim().trim_end_matches('/');
            let ws_url = if base.starts_with("ws://") || base.starts_with("wss://") {
                format!("{}/ws/control", base)
            } else {
                format!("wss://{}/ws/control", base)
            };
            log::info!("Connecting to control server: {}", ws_url);

            match connect_async(&ws_url).await {
                Ok((ws_stream, _)) => {
                    log::info!("Connected to control server");
                    backoff = Duration::from_secs(1); // Reset backoff

                    let (mut write, mut read) = ws_stream.split();

                    // 1. Wait for AUTH_CHALLENGE
                    let mut authed = false;
                    let mut auth_rejected = false;
                    if let Some(Ok(Message::Text(text))) = read.next().await {
                        if let Ok(env) = serde_json::from_str::<ControlEnvelope>(&text) {
                            if env.action == ActionType::AUTH_CHALLENGE {
                                let challenge = serde_json::from_value::<AuthChallengePayload>(env.payload).unwrap_or(AuthChallengePayload {
                                    nonce_salt: String::new(),
                                    server_time: 0,
                                });

                                // 2. Send AUTH_REQUEST with NonceSalt binding (P2-1)
                                let now = current_time_ms();
                                let nonce = Uuid::new_v4().to_string();
                                let sig = Self::compute_signature(
                                    &self.config.psk_secret,
                                    &self.config.account_id,
                                    &self.config.device_id,
                                    &nonce,
                                    now,
                                    &challenge.nonce_salt,
                                );

                                let auth_payload = AuthRequestPayload {
                                    account_id: self.config.account_id.clone(),
                                    device_id: self.config.device_id.clone(),
                                    hostname: self.config.hostname.clone(),
                                    os_type: self.config.os_type.clone(),
                                    app_version: self.config.app_version.clone(),
                                    signature: sig,
                                    nonce,
                                    timestamp: now,
                                };

                                let auth_env = ControlEnvelope {
                                    version: 1,
                                    trace_id: Uuid::new_v4().to_string(),
                                    action: ActionType::AUTH_REQUEST,
                                    from_device: self.config.device_id.clone(),
                                    to_device: None,
                                    timestamp: now,
                                    payload: serde_json::to_value(auth_payload).unwrap(),
                                };

                                if write.send(Message::Text(serde_json::to_string(&auth_env).unwrap())).await.is_ok() {
                                    // 3. Wait for AUTH_RESPONSE and check result (P0-4)
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

                    if !authed {
                        if auth_rejected {
                            log::warn!("Authentication rejected by server (invalid credentials). Backing off for 30s before retry (N9).");
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        } else {
                            log::warn!("Authentication handshake unsuccessful, disconnecting");
                            tokio::time::sleep(Duration::from_secs(3)).await;
                        }
                        continue;
                    }

                    // 4. Heartbeat & Forwarding loop (P0-2: consume outgoing_rx and forward to websocket)
                    let mut ping_interval = tokio::time::interval(Duration::from_secs(15));
                    loop {
                        tokio::select! {
                            _ = ping_interval.tick() => {
                                let ping_env = ControlEnvelope {
                                    version: 1,
                                    trace_id: Uuid::new_v4().to_string(),
                                    action: ActionType::HEARTBEAT_PING,
                                    from_device: self.config.device_id.clone(),
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
                    log::warn!("Connection failed: {}, retrying...", err);
                }
            }

            // Exponential backoff with jitter
            let jitter = Duration::from_millis(fastrand_u64(0, 1000));
            tokio::time::sleep(backoff + jitter).await;
            backoff = (backoff * 2).min(max_backoff);
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
