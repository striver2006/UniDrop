//! 发送侧业务流程（从桌面 `commands/clipboard_cmd.rs` 下沉，两端共享）。
//!
//! 桌面壳的 tauri command 与移动壳的 FFI 入口都调这里的 `*_flow` 函数；
//! 剪贴板读取除外——那是平台能力，桌面走 `platform::read_clipboard`，
//! 移动端由 Dart 读好后以字节形式传入 `send_bytes_flow`。

use std::path::PathBuf;

use uuid::Uuid;

use crate::app_state::AppState;
use crate::core::transfer_engine::{expand_selection, TransferEngine, TransferSource, SelectedFile};
use crate::host::HostBridge;
use crate::protocol::{ActionType, ControlEnvelope, ServerLimits, TransferOfferPayload};
use crate::storage::HistoryRepo;

/// 服务端**未下发**限额时的兜底上限（老服务端，或尚未连上）。
///
/// 这两个数与服务端的默认值（文本 4 MB、图片 64 MB）是**两回事，不得合并**。
///
/// 图片那条一眼能看出来：兜底 32 MB ≠ 服务端默认 64 MB。跟着改成 64 MB 等于
/// 在一台没装本轮改动的旧服务端上单方面放宽了限制——旧服务端不会拒绝，
/// 于是 50 MB 的图片真的会被发出去，而它此前是被这里拦住的。
///
/// **文本那条是更阴的陷阱：兜底 4 MB 与服务端默认 4 MB 恰好相等。** 数值相同
/// 会诱使人把它们收敛成同一个常量，但它们相等是巧合：兜底值表达「旧环境里的
/// 原样」，正确性依据是改造前的取值；服务端默认值表达「新策略」，部署者随时
/// 可能调整。共用一个常量后，将来有人把服务端文本上限调到 8 MB，兜底值会被
/// 静默拖着一起变，旧服务端上的行为随之漂移，而改动者不知道自己动了两样东西。
///
/// 本文件 tests 模块的 `fallback_constants_are_pinned_independently` 把这两个数
/// 分别钉死。
const FALLBACK_TEXT_BYTES: usize = 4 * 1024 * 1024;
const FALLBACK_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// 把字节数格式化成「128.0 MB」这类可读文案。上限数字必须出现在提示里——
/// 只说「文件太大」用户无从判断该删哪个、删到多少。
fn human_bytes(n: i64) -> String {
    const KB: f64 = 1024.0;
    let v = n as f64;
    if v < KB {
        return format!("{} B", n);
    }
    if v < KB * KB {
        return format!("{:.1} KB", v / KB);
    }
    if v < KB * KB * KB {
        return format!("{:.1} MB", v / (KB * KB));
    }
    format!("{:.2} GB", v / (KB * KB * KB))
}

/// 一次 metadata 遍历：过滤掉不可读项，同时累计大小并校验限额。
///
/// 入参是 **`expand_selection` 展开后的清单**——文件夹已经递归成带前缀的
/// 文件条目，所以这里的计数天然含目录内的文件（目录内 60 个文件就是 60 个
/// item），与 `prepare_offer` 的口径一致（两者都跳过不可读项）。
///
/// **这一趟必须发生在 `prepare_offer` 之前。** `prepare_offer` 会把每个文件
/// 完整读一遍算 SHA-256；把预检放在它之后，等于超限的 100 GB 文件也要先花
/// 几分钟算完哈希才弹提示——需求 3 要的「超出限制时弹出提示」就名存实亡了。
/// 文件大小只需 `fs::metadata`，根本不需要哈希。
///
/// 返回过滤后的清单，供调用方原样交给 `prepare_offer`。
fn precheck_file_paths(
    files: &[SelectedFile],
    limits: Option<&ServerLimits>,
) -> Result<Vec<SelectedFile>, String> {
    let mut usable: Vec<SelectedFile> = Vec::with_capacity(files.len());
    let mut total: i64 = 0;

    for f in files {
        let meta = match std::fs::metadata(&f.path) {
            Ok(m) => m,
            Err(_) => continue, // 与 prepare_offer 一致：不可读的跳过
        };
        if meta.is_dir() {
            continue; // 展开后不应出现；与 prepare_offer 的兜底跳过同口径
        }

        let size = meta.len() as i64;
        if let Some(l) = limits {
            if ServerLimits::exceeds(l.max_single_file_bytes, size) {
                let name = f
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "该文件".to_string());
                return Err(format!(
                    "文件太大，无法传输：「{}」为 {}，单个文件最大 {}",
                    name,
                    human_bytes(size),
                    human_bytes(l.max_single_file_bytes)
                ));
            }
        }
        total += size;
        usable.push(f.clone());
    }

    if usable.is_empty() {
        return Err("没有可发送的文件（所选路径不存在、不可读或均为空文件夹）".into());
    }

    if let Some(l) = limits {
        if l.max_items_per_offer > 0 && usable.len() > l.max_items_per_offer as usize {
            return Err(format!(
                "文件太多，无法传输：本次选择了 {} 个，一次最多传输 {} 个",
                usable.len(),
                l.max_items_per_offer
            ));
        }
        if ServerLimits::exceeds(l.max_total_transfer_bytes, total) {
            return Err(format!(
                "内容太大，无法传输：本次共 {}，单次传输总量最大 {}",
                human_bytes(total),
                human_bytes(l.max_total_transfer_bytes)
            ));
        }
    }

    Ok(usable)
}

/// Stores the pending outbound offer, sends TRANSFER_OFFER and records history.
async fn dispatch_offer(
    state: &AppState,
    target_device: &str,
    offer: TransferOfferPayload,
    source: TransferSource,
    e2ee_key: Option<&ring::aead::LessSafeKey>,
) -> Result<String, String> {
    let session_id = offer.session_id.clone();

    // 发往中继的是**密封副本**；本地的 pending_outbound 与历史行继续用带明文
    // 元数据的原件。两者分开是本函数存在密钥参数的唯一理由：
    // 抹空后的 offer 一旦进了本地，发送端自己的卡片与历史就只剩空文件名。
    let wire_offer = match e2ee_key {
        Some(key) => crate::core::e2ee::sealed_offer_for_wire(&offer, key)?,
        None => offer.clone(),
    };

    let offer_env = ControlEnvelope {
        version: 1,
        trace_id: Uuid::new_v4().to_string(),
        action: ActionType::TRANSFER_OFFER,
        from_device: state.device_id.clone(),
        to_device: Some(target_device.to_string()),
        timestamp: crate::core::connection_actor::current_time_ms(),
        payload: serde_json::to_value(&wire_offer).map_err(|e| e.to_string())?,
    };

    {
        let mut pending = state.pending_outbound.lock().await;
        pending.insert(session_id.clone(), (offer.clone(), source));
    }

    state.outgoing_tx.send(offer_env).await.map_err(|e| e.to_string())?;
    log::info!("dispatch_offer: session {} to {} enqueued to actor", session_id, target_device);

    {
        // 账号在这里取而不是更早：settings 锁与 db_conn 锁不要交叠持有。
        let account_id = { state.settings.lock().await.account_id.clone() };
        let conn = state.db_conn.lock().await;
        let _ = HistoryRepo::record_task(&conn, &account_id, &session_id, target_device, "SEND", &offer, "TRANSFERRING");
    }

    Ok(session_id)
}

/// 本次传输的加密决策。
///
/// `key` 为 `None` 时 `fallback_reason` 必然为 `Some`——调用方据此发出可见提示。
/// **静默回落明文是安全功能里最糟的反模式**：用户以为加密了，实际没有。
struct E2eeDecision {
    /// 用 `Arc` 是因为这把密钥要用在两个地方：`prepare_offer`（决定块长）
    /// 与 `dispatch_offer`（密封发往中继的那一份）。`LessSafeKey` 不是 `Clone`，
    /// 而重新派生一次就多一处「用错 session_id」的机会。
    key: Option<std::sync::Arc<ring::aead::LessSafeKey>>,
    fallback_reason: Option<String>,
}

/// 在构造 OFFER **之前**决定加不加密。
///
/// 协商为什么必须前置到这里，而不是等对端在 ANSWER 里回一个能力位：
/// 1. 元数据在构造 OFFER 的那一刻就已经抹空了，等 ANSWER 回来才决定已经太晚——
///    老接收端收到 OFFER 会无条件接受，然后拿着空文件名失败；
/// 2. ANSWER 根本带不回自定义字段：服务端在授权成功后会
///    `env.Payload, _ = json.Marshal(answer)` 重建载荷，未知字段被 Go 静默丢弃
///    （server/internal/controller/control_ws.go:388）。OFFER 路径则是原样转发——
///    这个不对称会绊倒任何下一个想给 ANSWER 加字段的人。
///
/// 所以改读在线设备表里已有的 `app_version`，单次 OFFER 即定形态，服务端零改动。
///
/// **注意这不是抗主动攻击的保证**：协商信息全部经过服务器，恶意服务端可以改写
/// `app_version` 把传输打回明文。本轮是机会性加密，威胁模型里已如实记录。
async fn decide_e2ee(
    state: &AppState,
    target_device: &str,
    session_id: &Uuid,
) -> E2eeDecision {
    let (enabled, psk, account_id) = {
        let s = state.settings.lock().await;
        (s.e2ee_enabled, s.psk_secret.clone(), s.account_id.clone())
    };

    if !enabled {
        // 用户自己关的，不必提示——提示应当只用于「你以为开着但这次没生效」。
        return E2eeDecision { key: None, fallback_reason: None };
    }

    let peer_version = {
        let devs = state.online_devices.lock().await;
        devs.iter()
            .find(|d| d.device_id == target_device)
            .map(|d| d.app_version.clone())
    };

    if !crate::core::e2ee::peer_supports_e2ee(peer_version.as_deref()) {
        // 文案刻意不写死「对端版本过旧」：协商信息经过服务器，
        // 「对端确实老」与「协商字段被剥离」在客户端是**不可区分**的。
        // 只陈述现象与后果，不做无法证实的归因。
        return E2eeDecision {
            key: None,
            fallback_reason: Some("本次传输未加密：未能确认对端支持端到端加密".to_string()),
        };
    }

    match crate::core::e2ee::derive_session_key(&psk, session_id, &account_id) {
        Ok(k) => E2eeDecision { key: Some(std::sync::Arc::new(k)), fallback_reason: None },
        Err(e) => {
            log::warn!("会话密钥派生失败，本次回落明文: {}", e);
            E2eeDecision {
                key: None,
                fallback_reason: Some(format!("本次传输未加密：无法派生会话密钥（{e}）")),
            }
        }
    }
}

/// 回落提示统一从这里发，保证各条发送路径的行为一致。
fn emit_fallback(bridge: &dyn HostBridge, reason: Option<String>) {
    if let Some(r) = reason {
        log::warn!("{}", r);
        bridge.emit("e2ee-fallback", &serde_json::Value::String(r));
    }
}

/// 发送文件的全流程（目录展开 → 限额预检 → E2EE 协商 → 哈希与 OFFER 构造 → 派发）。
///
/// 路径来源由宿主决定：桌面是文件对话框 / 拖拽 / 剪贴板文件列表（可含文件夹）；
/// 移动端是 SAF / DocumentPicker 返回的**可读文件路径**
/// （Flutter 的 file_picker 在两端都把选中内容落在应用可读的临时目录，
/// 因此这里不需要感知 content:// URI）。
/// 文件夹在这里统一递归展开，线上只出现文件条目。
pub async fn send_files_flow(
    state: &AppState,
    bridge: &dyn HostBridge,
    target_device: String,
    paths: Vec<String>,
) -> Result<String, String> {
    if paths.is_empty() {
        return Err("No files specified".into());
    }

    let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let session_id = Uuid::new_v4();

    // 0. 目录展开 + 限额预检——必须发生在算哈希与 dispatch_offer 之前。
    //
    //    展开在前：文件夹在这里递归成带「文件夹名/」前缀的文件清单，
    //    预检的条目计数因此包含目录内的所有文件。
    //
    //    在哈希之前：否则超限的大文件要等全量 SHA-256 算完才被拒（见
    //    precheck_file_paths 的说明）。
    //
    //    在 dispatch_offer 之前：dispatch_offer 会先写 pending_outbound、
    //    再落一行 TRANSFERRING 历史。在那之后才拒绝，会留下一张永远停在
    //    「传输中」的卡片和一行没人收尾的历史——正是要修的那个形态。
    //
    //    两段都是纯文件系统 IO，合并进同一个 spawn_blocking。
    let limits_snapshot = { state.server_limits.lock().await.clone() };
    let files = tokio::task::spawn_blocking(move || {
        let expanded = expand_selection(&path_bufs)?;
        precheck_file_paths(&expanded, limits_snapshot.as_ref())
    })
    .await
    .map_err(|e| e.to_string())??;

    // 1. 协商加密——必须在 prepare_offer 之前，理由见 decide_e2ee。
    let decision = decide_e2ee(state, &target_device, &session_id).await;
    emit_fallback(bridge, decision.fallback_reason);

    // 2. Prepare offer in spawn_blocking（防止哈希阻塞 Tokio worker）
    let e2ee_key = decision.key;
    let key_for_prepare = e2ee_key.clone();
    let (offer_payload, valid_paths) = tokio::task::spawn_blocking(move || {
        TransferEngine::prepare_offer(session_id, &files, key_for_prepare.as_deref())
    })
    .await
    .map_err(|e| e.to_string())??;

    // 3. Dispatch offer envelope (1:1 aligned pending entry)
    dispatch_offer(state, &target_device, offer_payload, TransferSource::Files(valid_paths), e2ee_key.as_deref()).await
}

/// 发送一段内存内容（剪贴板文本 / 图片，或宿主已读好的任意字节）。
///
/// 限额口径与桌面 cmd_send_clipboard 完全一致：TEXT 兜底 4 MB、IMAGE 兜底 32 MB，
/// 服务端限额在场时以其为准。
pub async fn send_bytes_flow(
    state: &AppState,
    bridge: &dyn HostBridge,
    target_device: String,
    data_type: &str,
    name: &str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let session_id = Uuid::new_v4();
    let limits_snapshot = { state.server_limits.lock().await.clone() };

    let decision = decide_e2ee(state, &target_device, &session_id).await;
    let e2ee_key = decision.key;
    let key_for_prepare = e2ee_key.clone();

    // 限额检查仍在 prepare_offer_from_bytes 之前——这条路径改造前就是
    // 哈希前检查，把它挪到之后会是倒退。
    let cap = match data_type {
        "TEXT" => limits_snapshot
            .map(|l| l.max_clipboard_text_bytes)
            .unwrap_or(FALLBACK_TEXT_BYTES as i64),
        "IMAGE" => limits_snapshot
            .map(|l| l.max_clipboard_image_bytes)
            .unwrap_or(FALLBACK_IMAGE_BYTES as i64),
        _ => 0,
    };
    if cap > 0 && ServerLimits::exceeds(cap, bytes.len() as i64) {
        let kind_label = if data_type == "TEXT" { "文本" } else { "图片" };
        return Err(format!(
            "剪贴板内容太大，无法传输：{}为 {}，最大 {}",
            kind_label,
            human_bytes(bytes.len() as i64),
            human_bytes(cap)
        ));
    }

    let dt = data_type.to_string();
    let nm = name.to_string();
    let (offer_payload, source) = tokio::task::spawn_blocking(move || {
        TransferEngine::prepare_offer_from_bytes(session_id, &dt, &nm, bytes, key_for_prepare.as_deref())
    })
    .await
    .map_err(|e| e.to_string())??;

    // 回落提示放在这里而不是 decide_e2ee 之后：内容为空 / 超限时上面的
    // 分支已经返回 Err，那次传输根本没发生，却先弹一条「本次传输未加密」
    // 会让人莫名其妙。send_files_flow 因为 precheck 排在协商之前而天然没有
    // 这个问题，两处顺序对齐。
    emit_fallback(bridge, decision.fallback_reason);

    dispatch_offer(state, &target_device, offer_payload, source, e2ee_key.as_deref()).await
}

/// `send_bytes_flow` 使用的兜底常量的只读出口（测试与宿主展示用）。
pub fn fallback_text_bytes() -> usize {
    FALLBACK_TEXT_BYTES
}

/// 同上，图片兜底。
pub fn fallback_image_bytes() -> usize {
    FALLBACK_IMAGE_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 独占临时目录，Drop 时递归清掉。与 cache_manager 的测试同样不引入
    /// tempfile 依赖——std + 已有的 uuid 足够。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("unidrop-precheck-{}", Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn file(&self, name: &str, size: usize) -> PathBuf {
            let p = self.0.join(name);
            fs::write(&p, vec![0u8; size]).unwrap();
            p
        }
        fn dir(&self, name: &str) -> PathBuf {
            let p = self.0.join(name);
            fs::create_dir_all(&p).unwrap();
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn limits(single: i64, total: i64, items: i32) -> ServerLimits {
        ServerLimits {
            max_single_file_bytes: single,
            max_total_transfer_bytes: total,
            max_clipboard_image_bytes: 0,
            max_clipboard_text_bytes: 0,
            max_items_per_offer: items,
            max_concurrent_transfers: 0,
        }
    }

    /// 直接选中单个文件的展开等价物：relative 就是文件名本身。
    fn sel(path: PathBuf) -> SelectedFile {
        let relative = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unnamed".to_string());
        SelectedFile { path, relative }
    }

    /// 服务端未下发限额（老服务端，或尚未连上）时**不做文件预检**。
    ///
    /// 这条守护「三选一」：None 必须是「沿用改造前行为」，既不能当成
    /// 「全部为 0」把一切拦死，也不该凭空造出限制。改造前 cmd_send_files
    /// 本就不检查大小，所以「照旧」就是不检查。
    #[test]
    fn absent_limits_do_not_reject_anything() {
        let tmp = TempDir::new();
        let paths = vec![sel(tmp.file("big.bin", 4096))];

        let usable = precheck_file_paths(&paths, None).expect("未下发限额时不应拒绝");
        assert_eq!(usable.len(), 1);
    }

    /// 兜底常量的具体数值钉死。
    ///
    /// 把图片兜底跟着服务端默认改成 64 MB，或把文本兜底与服务端默认文本上限
    /// 合并成同一个常量，这条都会红。文本那两个 4 MB 相等纯属巧合，
    /// 肉眼审查发现不了合并，只能靠这里挡。
    #[test]
    fn fallback_constants_are_pinned_independently() {
        assert_eq!(
            FALLBACK_TEXT_BYTES,
            4 * 1024 * 1024,
            "文本兜底必须保持改造前的 4 MB；它与服务端默认值相等是巧合，不得合并"
        );
        assert_eq!(
            FALLBACK_IMAGE_BYTES,
            32 * 1024 * 1024,
            "图片兜底必须保持改造前的 32 MB，不得跟着服务端默认值改成 64 MB——\
             那会在未升级的旧服务端上单方面放宽限制"
        );
    }

    /// **文件夹内的文件计入条目数与总量。**
    ///
    /// 展开发生在预检之前，所以目录里的每个文件都是一个 item、都累计大小。
    /// 这条同时是「文件夹不能发送」的正面断言：文件夹不再被静默丢弃，
    /// 而是展开成文件参与全部限额口径。
    #[test]
    fn directory_contents_count_toward_item_limit_and_total() {
        let tmp = TempDir::new();
        let dir = tmp.dir("photos");
        std::fs::write(dir.join("a.jpg"), vec![0u8; 600]).unwrap();
        std::fs::write(dir.join("b.jpg"), vec![0u8; 600]).unwrap();

        let expanded = expand_selection(&[dir]).unwrap();
        assert_eq!(expanded.len(), 2, "文件夹必须展开成其中的文件");

        // 3 个条目撞上 2 的上限：报错且数字要出现在文案里
        let with_extra = sel(tmp.file("c.bin", 10));
        let mut all = expanded.clone();
        all.push(with_extra);
        let err = precheck_file_paths(&all, Some(&limits(0, 0, 2))).unwrap_err();
        assert!(err.contains("文件太多"), "目录文件计入后应报条目超限：{}", err);
        assert!(err.contains('3'), "文案应写明当前数量：{}", err);

        // 两个 600B 文件撞上 1000B 总量上限
        let err = precheck_file_paths(&expanded, Some(&limits(0, 1000, 0))).unwrap_err();
        assert!(err.contains("总量"), "目录文件的大小应累计进总量：{}", err);

        // 不撞上限时原样通过
        let usable = precheck_file_paths(&all, Some(&limits(0, 0, 3))).unwrap();
        assert_eq!(usable.len(), 3);
        assert!(usable.iter().all(|f| f.path.is_file()));
    }

    /// 不可读/不存在的路径同样跳过，与 prepare_offer 一致。
    #[test]
    fn missing_paths_are_skipped() {
        let tmp = TempDir::new();
        let paths = vec![sel(tmp.file("a.txt", 10)), sel(tmp.0.join("does-not-exist.bin"))];

        let usable = precheck_file_paths(&paths, Some(&limits(0, 0, 1)))
            .expect("不存在的路径不应占用条目额度");
        assert_eq!(usable.len(), 1);
    }

    #[test]
    fn item_count_over_limit_is_rejected_with_the_number() {
        let tmp = TempDir::new();
        let paths = vec![
            sel(tmp.file("a.txt", 10)),
            sel(tmp.file("b.txt", 10)),
            sel(tmp.file("c.txt", 10)),
        ];

        let err = precheck_file_paths(&paths, Some(&limits(0, 0, 2))).unwrap_err();
        assert!(err.contains("无法传输"), "文案应含统一的「无法传输」措辞：{}", err);
        assert!(err.contains('2'), "文案必须写明上限数字，否则用户不知道该删到几个：{}", err);
        assert!(err.contains('3'), "文案应写明当前数量：{}", err);
    }

    #[test]
    fn single_file_over_limit_names_the_file() {
        let tmp = TempDir::new();
        let paths = vec![sel(tmp.file("small.txt", 10)), sel(tmp.file("huge.bin", 5000))];

        let err = precheck_file_paths(&paths, Some(&limits(1024, 0, 0))).unwrap_err();
        assert!(
            err.contains("huge.bin"),
            "必须点名是哪个文件超限，否则多选时无从下手：{}",
            err
        );
    }

    #[test]
    fn total_over_limit_is_rejected() {
        let tmp = TempDir::new();
        let paths = vec![sel(tmp.file("a.bin", 600)), sel(tmp.file("b.bin", 600))];

        // 单文件都不超，总量超
        let err = precheck_file_paths(&paths, Some(&limits(1024, 1000, 0))).unwrap_err();
        assert!(err.contains("总量"), "应报总量超限：{}", err);
    }

    #[test]
    fn exactly_at_limit_passes() {
        let tmp = TempDir::new();
        let paths = vec![sel(tmp.file("a.bin", 1024))];

        precheck_file_paths(&paths, Some(&limits(1024, 1024, 1)))
            .expect("恰好等于上限必须通过");
    }

    /// 限额为 0 = 不限制，三项都要覆盖。
    #[test]
    fn zero_limits_mean_unlimited() {
        let tmp = TempDir::new();
        let mut paths = Vec::new();
        for i in 0..50 {
            paths.push(sel(tmp.file(&format!("f{}.bin", i), 2048)));
        }

        precheck_file_paths(&paths, Some(&limits(0, 0, 0))).expect("限额全为 0 时不应拒绝");
    }

    /// 只选了空文件夹（或全部不可读）时给出可操作的提示，而不是笼统的
    /// 「没有文件」。走完整链路（展开 → 预检），和 send_files_flow 同序。
    #[test]
    fn only_empty_directories_yield_actionable_error() {
        let tmp = TempDir::new();
        let paths = vec![tmp.dir("only-a-dir")];

        let expanded = expand_selection(&paths).unwrap();
        assert!(expanded.is_empty(), "空文件夹不应产生条目");
        let err = precheck_file_paths(&expanded, Some(&limits(0, 0, 0))).unwrap_err();
        assert!(
            err.contains("空文件夹"),
            "应说明可能是空文件夹，否则用户不知道为什么发不出去：{}",
            err
        );
    }

    #[test]
    fn human_bytes_is_readable() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(128 * 1024 * 1024), "128.0 MB");
    }
}
