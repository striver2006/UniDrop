//! 控制面信令分发路由（从桌面壳 `lib.rs` 下沉，两端共享）。
//!
//! 消费 `ConnectionActor` 产出的 `ControlEnvelope`，驱动设备列表、
//! 传输握手、接收策略（v1 §1.3-A 的修复落点）与历史落库。
//! 宿主差异（emit / 通知）全部经 [`HostBridge`]。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, Mutex};

use crate::app_state::AppState;
use crate::core::connection_actor::{current_time_ms, ConnectionActor};
use crate::core::transfer_engine::{ActiveTransfer, TransferEngine};
use crate::host::{emit_json, HostBridge, NetworkKind, ReceivePolicy};
use crate::protocol::{
    ActionType, ControlEnvelope, DeviceListSyncPayload, DeviceOfflinePayload, DeviceOnlinePayload,
    TransferAnswerPayload, TransferOfferPayload,
};
use crate::storage::HistoryRepo;

/// Ask 档的确认等待窗口。超时未应答按拒收处理（走既有 reject_reason 通路），
/// 而不是无限挂着——发送端那张卡片等不起。
pub const CONFIRM_TIMEOUT_SECS: u64 = 30;

/// WifiOnly 档下，蜂窝网络中仍自动接收的小内容阈值。
/// 高于它的大文件 / 文件束在蜂窝下转确认。
pub const CELLULAR_AUTO_ACCEPT_BYTES: i64 = 8 * 1024 * 1024;

/// 按会话 ID 字符串派生密钥。
///
/// session_id 在信令里是字符串，而密钥派生要的是 16 字节原始 UUID
/// （字符串形式有大小写与连字符的歧义，见 `e2ee::derive_session_key`）。
/// 解析失败必须报错而不是回落到一个随便造的 UUID——那会让两端派生出不同的 key，
/// 表现为整批 tag 失败，而真正的原因（ID 格式不对）完全看不出来。
pub(crate) fn derive_key_for(
    session_id: &str,
    psk: &str,
    account_id: &str,
) -> Result<ring::aead::LessSafeKey, String> {
    let uuid = uuid::Uuid::parse_str(session_id)
        .map_err(|e| format!("session_id 不是合法 UUID（{session_id}）: {e}"))?;
    crate::core::e2ee::derive_session_key(psk, &uuid, account_id)
}

/// 待确认的入站 OFFER。
pub struct ConfirmPending {
    pub offer: TransferOfferPayload,
    pub from_device: String,
    pub received_at: Instant,
}

/// 可克隆（字段全为 Arc）：宿主通常持有一份 `Arc<SignalRouter>` 供确认应答
/// （`respond_pending_offer`）使用，`spawn` 内部再克隆一份进消费循环。
#[derive(Clone)]
pub struct SignalRouter {
    state: Arc<AppState>,
    bridge: Arc<dyn HostBridge>,
    /// Map to track inbound offers waiting for transfer token
    pending_inbound: Arc<Mutex<HashMap<String, (TransferOfferPayload, String)>>>,
    /// Ask 档下等待用户确认的入站 OFFER
    pending_confirm: Arc<Mutex<HashMap<String, ConfirmPending>>>,
}

impl SignalRouter {
    pub fn new(state: Arc<AppState>, bridge: Arc<dyn HostBridge>) -> Self {
        Self {
            state,
            bridge,
            pending_inbound: Arc::new(Mutex::new(HashMap::new())),
            pending_confirm: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 启动连接 actor 与信令消费循环。立即返回，全部在后台 task 中运行。
    ///
    /// 必须在 tokio runtime 上下文中调用（内部裸 `tokio::spawn`）；
    /// 桌面壳在 `tauri::async_runtime::spawn` 里调它，移动 FFI 在自建
    /// runtime 的 block_on 里调它，两者都满足。
    pub fn spawn(self: &Arc<Self>, actor: ConnectionActor) {
        let (internal_tx, mut internal_rx) = mpsc::channel(128);
        tokio::spawn(actor.run(internal_tx, self.bridge.clone()));
        let router = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(env) = internal_rx.recv().await {
                router.handle_envelope(env).await;
            }
        });
    }

    /// 用户对 Ask 档确认弹窗的应答入口（桌面面板 / 移动 FFI 均调这里）。
    ///
    /// 会话不在待确认表里时返回 Err——最常见的原因是超时任务已把它拒掉了，
    /// 界面应据此撤掉弹窗而不是再发一次 ANSWER。
    pub async fn respond_pending_offer(&self, session_id: &str, accept: bool) -> Result<(), String> {
        let entry = {
            let mut pending = self.pending_confirm.lock().await;
            pending.remove(session_id)
        };
        let Some(ConfirmPending { offer, from_device, .. }) = entry else {
            return Err(format!("会话 {} 不在待确认列表中（可能已超时）", session_id));
        };

        if accept {
            self.accept_offer(offer, from_device).await;
        } else {
            self.reject_offer(&offer.session_id, &from_device, "对方拒绝了本次传输").await;
        }
        Ok(())
    }

    /// 构造并发送一条 TRANSFER_ANSWER。
    async fn send_answer(
        &self,
        session_id: &str,
        to_device: &str,
        accepted: bool,
        reject_reason: Option<String>,
    ) {
        let answer = TransferAnswerPayload {
            session_id: session_id.to_string(),
            accepted,
            reject_reason,
            resumed_items: Vec::new(),
            token: None,
        };
        let answer_env = ControlEnvelope {
            version: 1,
            trace_id: uuid::Uuid::new_v4().to_string(),
            action: ActionType::TRANSFER_ANSWER,
            from_device: self.state.device_id.clone(),
            to_device: Some(to_device.to_string()),
            timestamp: current_time_ms(),
            payload: serde_json::to_value(&answer).unwrap(),
        };
        let _ = self.state.outgoing_tx.send(answer_env).await;
    }

    /// 接受：pending_inbound 登记 + ANSWER + 界面事件 + 历史落库（原桌面路径原样）。
    async fn accept_offer(&self, offer: TransferOfferPayload, from_device: String) {
        let session_id = offer.session_id.clone();
        self.pending_inbound
            .lock()
            .await
            .insert(session_id.clone(), (offer.clone(), from_device.clone()));
        self.send_answer(&session_id, &from_device, true, None).await;
        emit_json(self.bridge.as_ref(), "transfer-offer-received", &offer);

        // Persist incoming transfer in history
        {
            // settings 锁先取先放，不与 db_conn 锁交叠
            let record_account_id = {
                let settings = self.state.settings.lock().await;
                settings.account_id.clone()
            };
            let conn = self.state.db_conn.lock().await;
            let _ = HistoryRepo::record_task(
                &conn, &record_account_id, &session_id, &from_device, "RECEIVE", &offer, "TRANSFERRING",
            );
        }
    }

    async fn reject_offer(&self, session_id: &str, from_device: &str, reason: &str) {
        self.send_answer(session_id, from_device, false, Some(reason.to_string())).await;
    }

    /// Ask 档：登记待确认、通知界面、起超时看门狗。
    async fn defer_offer_for_confirmation(&self, offer: TransferOfferPayload, from_device: String) {
        let session_id = offer.session_id.clone();
        log::info!(
            "Transfer offer for session {} deferred for user confirmation (policy ask)",
            session_id
        );
        self.pending_confirm.lock().await.insert(
            session_id.clone(),
            ConfirmPending { offer: offer.clone(), from_device: from_device.clone(), received_at: Instant::now() },
        );

        // 界面事件：载荷字段与 transfer-offer-received 同构，另带过期时刻，
        // 供前端在断连重启等边界下自行作废弹窗。
        emit_json(
            self.bridge.as_ref(),
            "confirm-receive",
            &serde_json::json!({
                "session_id": offer.session_id,
                "from_device": from_device,
                "data_type": offer.data_type,
                "preview_summary": offer.preview_summary,
                "total_size": offer.total_size,
                "total_items": offer.total_items,
                "timeout_secs": CONFIRM_TIMEOUT_SECS,
            }),
        );

        // 超时看门狗：到点仍在待确认表里则按超时拒收。
        let pending = self.pending_confirm.clone();
        let state = self.state.clone();
        let bridge = self.bridge.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(CONFIRM_TIMEOUT_SECS)).await;
            let expired = {
                let mut map = pending.lock().await;
                match map.get(&session_id) {
                    Some(p) if p.received_at.elapsed() >= Duration::from_secs(CONFIRM_TIMEOUT_SECS) => {
                        map.remove(&session_id)
                    }
                    _ => None,
                }
            };
            if let Some(ConfirmPending { offer, from_device, .. }) = expired {
                log::info!("Confirm timeout for session {}, rejecting", session_id);
                let answer = TransferAnswerPayload {
                    session_id: offer.session_id.clone(),
                    accepted: false,
                    reject_reason: Some("接收确认超时未响应".to_string()),
                    resumed_items: Vec::new(),
                    token: None,
                };
                let answer_env = ControlEnvelope {
                    version: 1,
                    trace_id: uuid::Uuid::new_v4().to_string(),
                    action: ActionType::TRANSFER_ANSWER,
                    from_device: state.device_id.clone(),
                    to_device: Some(from_device.clone()),
                    timestamp: current_time_ms(),
                    payload: serde_json::to_value(&answer).unwrap(),
                };
                let _ = state.outgoing_tx.send(answer_env).await;
                emit_json(bridge.as_ref(), "confirm-expired", &serde_json::json!({ "session_id": session_id }));
            }
        });
    }

    pub async fn handle_envelope(&self, env: ControlEnvelope) {
        match env.action {
            ActionType::DEVICE_LIST_SYNC => {
                if let Ok(payload) = serde_json::from_value::<DeviceListSyncPayload>(env.payload) {
                    let mut devs = self.state.online_devices.lock().await;
                    *devs = payload.devices;
                    emit_json(self.bridge.as_ref(), "devices-updated", &*devs);
                }
            }
            ActionType::DEVICE_ONLINE => {
                if let Ok(payload) = serde_json::from_value::<DeviceOnlinePayload>(env.payload) {
                    let mut devs = self.state.online_devices.lock().await;
                    if !devs.iter().any(|d| d.device_id == payload.device.device_id) {
                        devs.push(payload.device);
                        emit_json(self.bridge.as_ref(), "devices-updated", &*devs);
                    }
                }
            }
            ActionType::DEVICE_OFFLINE => {
                if let Ok(payload) = serde_json::from_value::<DeviceOfflinePayload>(env.payload) {
                    let mut devs = self.state.online_devices.lock().await;
                    devs.retain(|d| d.device_id != payload.device_id);
                    emit_json(self.bridge.as_ref(), "devices-updated", &*devs);
                }
            }
            ActionType::TRANSFER_OFFER => {
                // Receiver received offer
                if let Ok(mut offer) = serde_json::from_value::<TransferOfferPayload>(env.payload.clone()) {
                    log::info!("Received TRANSFER_OFFER from {} for session {}", env.from_device, offer.session_id);

                    // 加密 OFFER：必须**先解密元数据再做任何别的事**。
                    //
                    // 下面几步会 emit 给界面并把历史行落库，而加密 OFFER 里的
                    // relative_path / sha256 / preview_summary 都是空的——
                    // 不先还原就会展示并持久化空文件名，后续展示与修剪都带着它。
                    //
                    // 解密失败就地拒收，不建数据面：最常见的原因是两端 PSK 不一致，
                    // 让它在信令阶段以明确理由失败，比拖到数据面靠 tag 失败发现要好得多
                    // （那条路径给出的现象与真正的原因隔了好几层）。
                    let mut reject_reason: Option<String> = None;
                    if offer.encrypted {
                        let (psk, acct) = {
                            let s = self.state.settings.lock().await;
                            (s.psk_secret.clone(), s.account_id.clone())
                        };
                        match derive_key_for(&offer.session_id, &psk, &acct) {
                            Ok(key) => {
                                if let Err(e) = crate::core::e2ee::open_offer_metadata(&mut offer, &key) {
                                    log::warn!("加密 OFFER 的元数据解密失败，拒收 session {}: {}", offer.session_id, e);
                                    reject_reason = Some("E2EE_KEY_MISMATCH".to_string());
                                }
                            }
                            Err(e) => {
                                log::warn!("接收端密钥派生失败，拒收 session {}: {}", offer.session_id, e);
                                reject_reason = Some("E2EE_KEY_MISMATCH".to_string());
                            }
                        }
                    }

                    if let Some(reason) = reject_reason {
                        self.reject_offer(&offer.session_id, &env.from_device, &reason).await;
                        self.bridge.emit(
                            "e2ee-offer-rejected",
                            &serde_json::Value::String(
                                "收到一份无法解密的传输请求，已拒收：两端密钥可能不一致".to_string(),
                            ),
                        );
                        return;
                    }

                    // 接收策略判定（v1 §1.3-A：不再无条件自动接受）。
                    match self.receive_decision(&offer).await {
                        ReceiveDecision::AutoAccept => {
                            self.accept_offer(offer, env.from_device).await;
                        }
                        ReceiveDecision::Ask => {
                            self.defer_offer_for_confirmation(offer, env.from_device).await;
                        }
                    }
                }
            }
            ActionType::TRANSFER_ANSWER => {
                if let Ok(answer) = serde_json::from_value::<TransferAnswerPayload>(env.payload) {
                    if answer.accepted {
                        if let Some(token) = answer.token {
                            // Check if we are the Sender
                            let outbound_entry = {
                                let mut pending = self.state.pending_outbound.lock().await;
                                pending.remove(&answer.session_id)
                            };

                            if let Some((offer, source)) = outbound_entry {
                                log::info!("Starting Sender task for session {}", answer.session_id);
                                let (active_server_url, tls_trust, psk, acct) = {
                                    let s = self.state.settings.lock().await;
                                    // 数据面必须与控制面用同一套信任策略。
                                    // 解析失败时同样回落到最严的一档——绝不能
                                    // 因为指纹写错就让这条连接比控制面更宽松。
                                    (s.server_url.clone(),
                                     s.tls_trust_config().unwrap_or_default(),
                                     s.psk_secret.clone(), s.account_id.clone())
                                };
                                // offer.encrypted 是发 OFFER 时就定下的，这里按它派生。
                                // 派生失败不能静默降级成明文：接收端已经按加密形态
                                // 准备好了，发明文过去只会让它整批 tag 失败。
                                let sender_key = if offer.encrypted {
                                    match derive_key_for(&answer.session_id, &psk, &acct) {
                                        Ok(k) => Some(k),
                                        Err(e) => {
                                            log::error!("发送端会话密钥派生失败，放弃本次传输: {}", e);
                                            return;
                                        }
                                    }
                                } else {
                                    None
                                };
                                tokio::spawn(TransferEngine::start_sender_task(
                                    active_server_url,
                                    tls_trust,
                                    answer.session_id.clone(),
                                    token.clone(),
                                    self.state.device_id.clone(),
                                    env.from_device.clone(),
                                    source,
                                    offer,
                                    sender_key,
                                    self.state.outgoing_tx.clone(),
                                    self.state.clone(),
                                    self.bridge.clone(),
                                ));
                            } else {
                                // Check if we are the Receiver (token echo)
                                let inbound_entry = {
                                    let mut pending = self.pending_inbound.lock().await;
                                    pending.remove(&answer.session_id)
                                };

                                if let Some((offer, sender_device)) = inbound_entry {
                                    let (auto_inject, active_server_url, tls_trust, psk, acct) = {
                                        let s = self.state.settings.lock().await;
                                        (s.auto_inject, s.server_url.clone(),
                                         s.tls_trust_config().unwrap_or_default(),
                                         s.psk_secret.clone(), s.account_id.clone())
                                    };
                                    let receiver_key = if offer.encrypted {
                                        match derive_key_for(&answer.session_id, &psk, &acct) {
                                            Ok(k) => Some(k),
                                            Err(e) => {
                                                log::error!("接收端会话密钥派生失败，放弃本次接收: {}", e);
                                                return;
                                            }
                                        }
                                    } else {
                                        None
                                    };
                                    log::info!("Starting Receiver task for session {}, auto_inject={}", answer.session_id, auto_inject);
                                    tokio::spawn(TransferEngine::start_receiver_task(
                                        active_server_url,
                                        tls_trust,
                                        answer.session_id.clone(),
                                        token,
                                        sender_device,
                                        self.state.device_id.clone(),
                                        offer,
                                        self.state.cache_manager.clone(),
                                        auto_inject,
                                        receiver_key,
                                        self.state.outgoing_tx.clone(),
                                        self.state.clone(),
                                        self.bridge.clone(),
                                    ));
                                }
                            }
                        }
                    } else {
                        // 接收方明确拒收（accepted=false）。
                        //
                        // 改造前这里没有 else：服务端原样转发拒收的 ANSWER，
                        // 而客户端只处理 accepted 的那一支，于是发送方的
                        // pending_outbound 永不释放、卡片停在等待状态——
                        // 一个不需要任何异常就能稳定复现的静默挂起。
                        //
                        // 它与本轮服务端「授权失败双向拒绝」是同一族问题的
                        // 两半：那边堵的是服务端不肯授权，这边堵的是对端不愿接收。
                        // 只修一半的话「不再有静默挂起」这个说法就只对一半。
                        let sid = answer.session_id.clone();
                        self.state.pending_outbound.lock().await.remove(&sid);
                        self.pending_inbound.lock().await.remove(&sid);

                        let brief = {
                            let conn = self.state.db_conn.lock().await;
                            let brief = HistoryRepo::get_task_brief(&conn, &sid);
                            let _ = HistoryRepo::update_task_status(
                                &conn, &sid, "FAILED", Some("对方拒绝了本次传输"),
                            );
                            brief
                        };

                        let (direction, summary, total_size, data_type) = match brief {
                            Some(b) => (
                                b.direction,
                                format!(
                                    "{} (对方拒绝了本次传输)",
                                    b.preview_summary.unwrap_or_else(|| "传输".into())
                                ),
                                b.total_size,
                                b.data_type,
                            ),
                            None => (
                                "SEND".to_string(),
                                "传输 (对方拒绝了本次传输)".to_string(),
                                0,
                                "FILES".to_string(),
                            ),
                        };

                        emit_json(self.bridge.as_ref(), "transfer-progress", &ActiveTransfer {
                            session_id: sid,
                            preview_summary: summary,
                            total_size,
                            transferred_size: 0,
                            direction,
                            progress: 0.0,
                            status: "FAILED".to_string(),
                            data_type,
                        });

                        crate::core::history_pruner::prune_and_notify(self.state.as_ref(), self.bridge.as_ref()).await;
                    }
                }
            }
            ActionType::TRANSFER_FAILURE => {
                // Peer-reported failure: flip the corresponding card to FAILED
                let session_id = env
                    .payload
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let err_msg = env
                    .payload
                    .get("error_message")
                    .or_else(|| env.payload.get("message"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                if let Some(sid) = session_id {
                    // 失败是这个会话的终点，两侧待处理队列都必须释放。
                    //
                    // 改造前两个 map 都只在 TRANSFER_ANSWER 带 token 的那条
                    // 路径上被删除，失败路径完全不碰。本轮让服务端主动拒绝成为
                    // 常规路径（超限即拒），这条泄漏随之从罕见变高频。
                    //
                    // **两个都要删，不是只删 pending_outbound。** 并发超限的
                    // 拒绝是**双向**的：服务端拒掉 ANSWER 后既不授权也不转发，
                    // 于是 token 回显——pending_inbound 唯一的删除点——永远不会
                    // 发生，接收端那份含完整 items 的 offer 会永久留在 map 里。
                    //
                    // 无条件双删而不判断本机是发送方还是接收方：
                    // HashMap::remove 对不存在的 key 是 no-op，而那个判断本身
                    // 才是出错的来源。
                    self.state.pending_outbound.lock().await.remove(&sid);
                    self.pending_inbound.lock().await.remove(&sid);

                    let brief = {
                        let conn = self.state.db_conn.lock().await;
                        let brief = HistoryRepo::get_task_brief(&conn, &sid);
                        let _ = HistoryRepo::update_task_status(&conn, &sid, "FAILED", err_msg.as_deref());
                        brief
                    };

                    let (direction, mut summary, total_size, data_type) = match brief {
                        Some(b) => (b.direction, b.preview_summary.unwrap_or_else(|| "传输".into()), b.total_size, b.data_type),
                        None => ("SEND".to_string(), "传输".into(), 0, "FILES".to_string()),
                    };
                    if let Some(m) = &err_msg {
                        summary = format!("{} ({})", summary, m);
                    }

                    emit_json(self.bridge.as_ref(), "transfer-progress", &ActiveTransfer {
                        session_id: sid,
                        preview_summary: summary,
                        total_size,
                        transferred_size: 0,
                        direction,
                        progress: 0.0,
                        status: "FAILED".to_string(),
                        data_type,
                    });

                    // 对端报告失败同样是终态写入点，也要修剪历史。
                    // 这里已经在上面取 conn 的 { } 块之外——那个 guard
                    // 随块结束释放，此处再取锁才不会死锁。
                    crate::core::history_pruner::prune_and_notify(self.state.as_ref(), self.bridge.as_ref()).await;
                }
            }
            ActionType::TRANSFER_COMPLETE => {
                // Sender-side completion broadcast. The receiver's own data-plane
                // verification path is authoritative for its card, so we only log
                // here to avoid racing an in-flight SHA-256 verification.
                if let Some(sid) = env.payload.get("session_id").and_then(|v| v.as_str()) {
                    log::info!("Peer reports TRANSFER_COMPLETE for session {}", sid);
                }
            }
            ActionType::AUTH_RESPONSE => {
                if let Ok(resp) = serde_json::from_value::<crate::protocol::AuthResponsePayload>(env.payload) {
                    if !resp.success {
                        self.bridge.emit(
                            "auth-failed",
                            &serde_json::Value::String(resp.error_message.unwrap_or_default()),
                        );
                    } else {
                        // 存下服务端下发的限额，供发送前本地预检与设置面板展示。
                        //
                        // resp.limits 为 None（老服务端不发这个字段）时**原样存 None**，
                        // 不要 unwrap_or_default() —— 那会变成一组全 0，而 0 在这里
                        // 表示「不限制」，等于在老服务端上把限额整组关掉。
                        // None 的含义是「未知」，由发送端退回兜底常量处理。
                        {
                            let mut slot = self.state.server_limits.lock().await;
                            *slot = resp.limits.clone();
                        }
                        emit_json(self.bridge.as_ref(), "server-limits-updated", &resp.limits);
                        self.bridge.emit("auth-success", &serde_json::Value::Null);
                    }
                }
            }
            _ => {}
        }
    }

    /// 接收策略判定（v1 §1.3-A 修复的核心）。
    ///
    /// WifiOnly 下蜂窝网络仍放行小体积 TEXT / IMAGE——剪贴板同步的本体就是
    /// 小内容，一概转确认会把最常见的路径毁掉；文件束（可能上百 MB）转确认。
    /// 网络判定为 Unknown 时按非蜂窝放行：iOS NWPathMonitor /
    /// Android ConnectivityManager 都可靠，Unknown 只在宿主未接线时出现，
    /// 一概保守会让「未接线」退化成「每次都弹窗」。
    async fn receive_decision(&self, offer: &TransferOfferPayload) -> ReceiveDecision {
        let policy = {
            let s = self.state.settings.lock().await;
            s.receive_policy
        };
        match policy {
            ReceivePolicy::Always => ReceiveDecision::AutoAccept,
            ReceivePolicy::Ask => ReceiveDecision::Ask,
            ReceivePolicy::WifiOnly => {
                if self.bridge.network_kind() == NetworkKind::Cellular {
                    let small_clipboard =
                        matches!(offer.data_type.as_str(), "TEXT" | "IMAGE")
                            && offer.total_size <= CELLULAR_AUTO_ACCEPT_BYTES;
                    if small_clipboard {
                        ReceiveDecision::AutoAccept
                    } else {
                        ReceiveDecision::Ask
                    }
                } else {
                    ReceiveDecision::AutoAccept
                }
            }
        }
    }
}

enum ReceiveDecision {
    AutoAccept,
    Ask,
}

/// 后台缓存清扫 worker（从桌面壳下沉，两端共享）。
///
/// 结构是**先 sweep 后 sleep**，不能反过来。改造前用的是
/// `tokio::time::interval`，它的首跳立即返回，因此启动时会先扫一次；
/// 若改成先 sleep，这个「启动即扫」会静默消失——默认间隔下启动后一小时
/// 内不清理，而用户若把间隔设成 1440 分钟又每天关机，sleep 永远睡不满，
/// sweep 可能一次都不执行。
///
/// 每轮都重新读设置，所以间隔与策略都是可变的；代价是改动间隔要等
/// 当前这一轮睡完才生效（重启则立即生效），这一点写在设置项说明里。
pub fn spawn_cache_sweeper(state: Arc<AppState>) {
    let cache_sweep_mgr = state.cache_manager.clone();
    let sweep_settings = state.settings.clone();
    tokio::spawn(async move {
        loop {
            let (policy, interval) = {
                let s = sweep_settings.lock().await;
                (
                    crate::core::retention::RetentionPolicy::from_settings(&s),
                    crate::core::retention::effective_sweep_interval(&s),
                )
            }; // guard 必须在 sweep 之前释放：sweep 内部要取 db_conn 锁，
               // 而这里持有的是 settings 锁，两者不同但没必要交叠持有

            // Err 必须留痕：后台清理的唯一职责就是防磁盘占满，若持续失败
            // 而外部零信号，故障呈现形态恰是这个功能本身要防的那件事。
            // 与 prune_and_notify 的处理对齐。
            match cache_sweep_mgr.sweep(policy).await {
                Ok(purged) if purged > 0 => {
                    log::info!("Cache cleaner purged {} expired or LRU entries", purged)
                }
                Ok(_) => {}
                Err(e) => log::warn!("Cache sweep failed: {}", e),
            }

            tokio::time::sleep(interval).await;
        }
    });
}

/// 启动收敛：先把崩溃残留的非终态行复位，再修剪一次历史（需求 4）。
///
/// 顺序不能反：修剪按约束 A 跳过非终态行，不先复位的话，上次被杀掉的
/// 进程留下的 TRANSFERRING 死行会永久占住保留额度，谁也清不掉。
pub async fn startup_recovery(state: &AppState, bridge: &dyn HostBridge) {
    {
        let conn = state.db_conn.lock().await;
        match HistoryRepo::reset_stale_in_flight(&conn) {
            Ok(0) => {}
            Ok(n) => log::info!("Reset {} stale in-flight transfer(s) to FAILED on startup", n),
            Err(e) => log::warn!("Failed to reset stale in-flight transfers: {}", e),
        }
    } // conn guard 必须在此释放，下一行会重新取同一把锁
    crate::core::history_pruner::prune_and_notify(state, bridge).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cellular_threshold_is_8mb() {
        // 蜂窝自动接收阈值钉死：调大它等于在流量计费下静默放大消耗。
        assert_eq!(CELLULAR_AUTO_ACCEPT_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn confirm_timeout_is_30s() {
        assert_eq!(CONFIRM_TIMEOUT_SECS, 30);
    }
}
