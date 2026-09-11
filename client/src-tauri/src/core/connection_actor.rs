use std::sync::Arc;
use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::protocol::{ActionType, AuthRequestPayload, ControlEnvelope};

type HmacSha256 = Hmac<Sha256>;

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
    outgoing_tx: mpsc::Sender<ControlEnvelope>,
    incoming_rx: Arc<Mutex<mpsc::Receiver<ControlEnvelope>>>,
}

impl ConnectionActor {
    pub fn new(config: ConnectionConfig) -> (Self, mpsc::Sender<ControlEnvelope>, mpsc::Receiver<ControlEnvelope>) {
        let (out_tx, out_rx) = mpsc::channel(128);
        let (in_tx, in_rx) = mpsc::channel(128);

        let actor = Self {
            config,
            outgoing_tx: out_tx.clone(),
            incoming_rx: Arc::new(Mutex::new(out_rx)),
        };

        (actor, out_tx, in_rx)
    }

    /// Generates canonical signature: "UNIDROP_V1\n{account_id}\n{device_id}\n{nonce}\n{timestamp_ms}"
    pub fn compute_signature(secret: &str, account_id: &str, device_id: &str, nonce: &str, timestamp: i64) -> String {
        let canonical = format!("UNIDROP_V1\n{}\n{}\n{}\n{}", account_id, device_id, nonce, timestamp);
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(canonical.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Background loop with exponential backoff and jitter.
    pub async fn run(self, incoming_tx: mpsc::Sender<ControlEnvelope>) {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(30);

        loop {
            let ws_url = format!("{}/ws/control", self.config.server_url);
            log::info!("Connecting to control server: {}", ws_url);

            match connect_async(&ws_url).await {
                Ok((ws_stream, _)) => {
                    log::info!("Connected to control server");
                    backoff = Duration::from_secs(1); // Reset backoff

                    let (mut write, mut read) = ws_stream.split();

                    // 1. Wait for AUTH_CHALLENGE
                    if let Some(Ok(Message::Text(text))) = read.next().await {
                        if let Ok(env) = serde_json::from_str::<ControlEnvelope>(&text) {
                            if env.action == ActionType::AUTH_CHALLENGE {
                                // 2. Send AUTH_REQUEST
                                let now = chrono_now_ms();
                                let nonce = Uuid::new_v4().to_string();
                                let sig = Self::compute_signature(
                                    &self.config.psk_secret,
                                    &self.config.account_id,
                                    &self.config.device_id,
                                    &nonce,
                                    now,
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

                                let _ = write.send(Message::Text(serde_json::to_string(&auth_env).unwrap())).await;
                            }
                        }
                    }

                    // 3. Heartbeat & Forwarding loop
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
                                    timestamp: chrono_now_ms(),
                                    payload: serde_json::Value::Null,
                                };
                                if write.send(Message::Text(serde_json::to_string(&ping_env).unwrap())).await.is_err() {
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

fn chrono_now_ms() -> i64 {
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
