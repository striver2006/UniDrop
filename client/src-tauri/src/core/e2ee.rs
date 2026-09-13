//! E2EE 载荷加密：密钥派生、nonce 构造、块与元数据的 AEAD 封装。
//!
//! 本模块是整个加密方案的**唯一事实源**。分块长度、nonce 构造、AAD 约定
//! 全部只在这里定义一次——它们都是「两端必须逐字节一致」的约定，
//! 散落到调用点就会出现一端改了另一端没改，而失败形态是「解不开」而不是编译错误。
//!
//! 威胁模型边界（文档同步写明，不要在这里假装做到了更多）：
//! - 达成：服务器、云厂商、拿到 root 的人看不到内容；
//! - 达不成：前向保密（PSK 长期不变）、防设备失窃（PSK 明文存盘）、
//!   设备间隔离（单 PSK 派生的是组密钥）、防主动降级（协商信息经过服务器）。

use crate::protocol::envelope::{EncryptedItemMeta, EncryptedMetadata, TransferOfferPayload};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ring::{aead, hkdf};
use uuid::Uuid;

/// AES-256-GCM 认证标签长度。密文 = 明文 + 这么多字节。
pub const GCM_TAG_LEN: usize = 16;

/// 本方案的 E2EE 协议版本，写进 OFFER 的 `e2ee_version`。
///
/// 它**不承担能力协商职责**（协商在发 OFFER 之前就由对端 app_version 定了），
/// 只是接收端的自描述：告诉它这份 OFFER 里的元数据是不是加密的。
pub const E2EE_VERSION: u8 = 1;

/// nonce 的域分隔标记，占 `nonce[8..12]`。
///
/// 数据块与条目元数据用同一把会话密钥，靠这 4 字节让两者的 nonce 空间
/// 完全不相交——于是不必论证「item_index 不会恰好取到某个魔数」。
/// 这 4 字节此前是帧头 `Nonce` 字段里空着的部分。
const NONCE_DOMAIN_DATA: u32 = 0x0000_0000;
const NONCE_DOMAIN_METADATA: u32 = 0x0000_0001;

/// HKDF 的 info 前缀。带版本号是域分隔：将来换算法或换构造时，
/// 同一把 PSK 派生出的旧 key 不会被新用途复用。成本为零，不加才需要理由。
const HKDF_INFO_PREFIX: &[u8] = b"UNIDROP-E2EE-v1|";

/// 一次传输的**明文**块大小。
///
/// 加密时取 `MAX_PAYLOAD_LENGTH - GCM_TAG_LEN`，密文恰好填满 4MB 不触上限。
/// 这不是调优参数：满块加密后是 4MB+16，而中继的 `DecodeBinaryHeader` 会以
/// `ErrPayloadTooLarge` 当场断连（server/internal/protocol/binary_header.go:172），
/// 客户端只看到连接被关、看不到原因。
///
/// **调用点必须全部走这个函数，不得就地写 `MAX_PAYLOAD_LENGTH`。** 目前四处：
/// 1. `prepare_offer` 算 `total_chunks`
/// 2. `prepare_offer_from_bytes` 算 `total_chunks`
/// 3. 发送端切块的 offset / length
/// 4. 接收端 `write_payload_chunk` 的写入 offset
///
/// 外加接收端进度累加按**明文**长度计（见 `transfer_engine` 中的注释）。
/// 漏改其中任何一处的失败形态都是**静默的**：每块都能解密、能写入，
/// 只是写在错误的位置，直到最后 SHA256 才发现文件是坏的，而那时传输已经跑完。
pub fn plaintext_chunk_len(encrypted: bool) -> usize {
    if encrypted {
        crate::protocol::binary_header::MAX_PAYLOAD_LENGTH as usize - GCM_TAG_LEN
    } else {
        crate::protocol::binary_header::MAX_PAYLOAD_LENGTH as usize
    }
}

/// 从 PSK 派生本次会话的密钥。
///
/// ```text
/// key = HKDF-SHA256(
///     ikm  = psk 的 UTF-8 字节,
///     salt = session_id 的 16 字节原始 UUID,
///     info = "UNIDROP-E2EE-v1|" || account_id
/// ) → 32 字节
/// ```
///
/// - **不拿 PSK 直接当 key**：PSK 是用户手输的任意长度字符串，而 AES-256 要的是
///   恰好 32 字节的均匀随机量。HKDF 做长度归一化与熵提取，这是它的本职；
///   截断或补零是典型误用。
/// - **salt 用 session_id**：每次传输现铸 UUIDv4 ⇒ 每次传输一把新 key。
///   这是 nonce 唯一性论证的根基——key 换了，nonce 空间就是全新的。
/// - **salt 用 16 字节原始 UUID 而非字符串**：两端必须逐字节一致，而字符串形式
///   有大小写与连字符的歧义（`Uuid::to_string()` 今天是小写带连字符，
///   但没有任何机制强制它不变）。原始字节没有这个问题。
/// - **info 带 account_id**：把 key 绑定到账号上下文，与 registry 复合键、
///   本地历史按账号分区是同一条思路的延续。
///   注意 `account_id` 是用户自填标识（`[A-Za-z0-9._@-]`，1–64 字节，**不是 UUID**）；
///   两端由服务端按账号隔离路由，能建立传输即保证两侧是同一个字符串，
///   故这条绑定无需依赖它的随机性。
///
/// **HKDF 不能凭空创造熵**：用户把 PSK 填成 `123456`，E2EE 的强度就是 `123456`。
/// 这不比现状更弱（同一把 PSK 今天已在保护 HMAC 鉴权），但文档必须说明。
pub fn derive_session_key(
    psk: &str,
    session_id: &Uuid,
    account_id: &str,
) -> Result<aead::LessSafeKey, String> {
    if psk.is_empty() {
        return Err("PSK 为空，无法派生会话密钥".into());
    }

    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, session_id.as_bytes());
    let prk = salt.extract(psk.as_bytes());

    // info 由前缀与 account_id 拼接而成；ring 会把切片数组按顺序连接。
    let info: [&[u8]; 2] = [HKDF_INFO_PREFIX, account_id.as_bytes()];
    let okm = prk
        .expand(&info, &aead::AES_256_GCM)
        .map_err(|_| "HKDF expand 失败".to_string())?;

    let unbound = aead::UnboundKey::from(okm);
    Ok(aead::LessSafeKey::new(unbound))
}

/// 数据块的 nonce：`item_index ‖ chunk_index ‖ 0x00000000`（全大端）。
///
/// 唯一性论证（四种情况，缺一不可）：
/// 1. **同 session 内不同块**：`(item_index, chunk_index)` 唯一标识一个块。✓
/// 2. **不同 session**：key 由 session_id 派生，key 不同则 nonce 复用无害。✓
/// 3. **重传**（滑动窗口 / NACK）：重传的是**同一块的同一份明文**，
///    相同 (key, nonce, plaintext) 产出相同密文。GCM 的灾难性失败要求的是
///    「相同 (key,nonce) 加密**不同**明文」，相同明文重传是安全的。
///    **不要把这里"修"成随机 nonce**——那才会引入真正的问题（帧头放不下
///    随机 nonce 的同时还要保证接收端能重算）。
/// 4. **断点续传**：本仓库尚未实现（`BitmapRepo` 无生产调用、`resumed_items` 恒空），
///    当前每次重连都是新 session 新 key，天然安全。
///    **若将来实现续传，不得复用 session_id 传不同内容**——这条必须写进续传自己的验收。
pub fn data_nonce(item_index: u32, chunk_index: u32) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0..4].copy_from_slice(&item_index.to_be_bytes());
    n[4..8].copy_from_slice(&chunk_index.to_be_bytes());
    n[8..12].copy_from_slice(&NONCE_DOMAIN_DATA.to_be_bytes());
    n
}

/// 条目元数据的 nonce：`0 ‖ 0 ‖ 0x00000001`。
///
/// 元数据每个 session 只加密一次，所以前 8 字节恒零不产生复用；
/// 域分隔标记让它与任何数据块的 nonce 都不相同。
pub fn metadata_nonce() -> [u8; 12] {
    let mut n = [0u8; 12];
    n[8..12].copy_from_slice(&NONCE_DOMAIN_METADATA.to_be_bytes());
    n
}

/// 加密，产出 `ciphertext ‖ tag`。
///
/// **AAD 统一为空**，数据块与元数据都是。这是个需要解释的选择：
/// 把帧头字段纳入 AAD 看似更严格，但 `item_index` / `chunk_index` 已经通过 nonce
/// 参与了认证——改动它们会直接导致 nonce 不匹配、tag 校验失败。而 `total_chunks`
/// / `flags` 被篡改的后果由上层兜住（块数不符会让传输无法完成）。
/// 引入非空 AAD 只会新增一处两端必须逐字节一致的约定，
/// 而它的失败形态是「本该能解密的数据解不开」。选简单且可论证的一侧。
pub fn seal(key: &aead::LessSafeKey, nonce: [u8; 12], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let mut buf = plaintext.to_vec();
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::empty(),
        &mut buf,
    )
    .map_err(|_| "AEAD 加密失败".to_string())?;
    Ok(buf)
}

/// 解密 `ciphertext ‖ tag`，返回明文。
///
/// tag 校验失败时返回 `Err`。调用方**不得**把它当作普通的传输损坏无限重传：
/// 见 `transfer_engine` 里的双层熔断（单块 3 次 + 会话级累计 3 次）。
pub fn open(
    key: &aead::LessSafeKey,
    nonce: [u8; 12],
    ciphertext_and_tag: &[u8],
) -> Result<Vec<u8>, String> {
    if ciphertext_and_tag.len() < GCM_TAG_LEN {
        return Err(format!(
            "密文长度 {} 小于 tag 长度 {}",
            ciphertext_and_tag.len(),
            GCM_TAG_LEN
        ));
    }
    let mut buf = ciphertext_and_tag.to_vec();
    let plaintext = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::empty(),
            &mut buf,
        )
        .map_err(|_| "AEAD 解密失败（tag 校验未通过）".to_string())?;
    Ok(plaintext.to_vec())
}

/// 产出**发往中继的那一份** OFFER：敏感元数据加密进 `encrypted_metadata`，
/// 明文字段抹空。原 `offer` 不受影响。
///
/// **返回副本而不是原地改，这一点是有意的。** 原先的实现用 `std::mem::take`
/// 就地抹空，而那个 offer 随后同时进了发送端的 `pending_outbound`
/// （→ `transfer-progress` 事件，卡片预览为空）与 `HistoryRepo::record_task`
/// （→ 发送端自己的历史行落空文件名、空 sha256）。线上确实没有明文了，
/// 但**本地也看不见了**。返回副本让「本地那份被抹空」在类型层面难以发生。
///
/// 与 `open_offer_metadata` 是严格对称的一对，**必须放在一起改**：
/// 摘出的字段清单在两边各写一次，漏一个的后果是接收端拿到空值继续跑，
/// 然后在 PathGuard 或 SHA256 校验处以一个和真实原因无关的错误失败。
pub fn sealed_offer_for_wire(
    offer: &TransferOfferPayload,
    key: &aead::LessSafeKey,
) -> Result<TransferOfferPayload, String> {
    let meta = EncryptedMetadata {
        preview_summary: offer.preview_summary.clone(),
        items: offer
            .items
            .iter()
            .map(|it| EncryptedItemMeta {
                item_index: it.item_index,
                relative_path: it.relative_path.clone(),
                sha256: it.sha256.clone(),
                is_dir: it.is_dir,
            })
            .collect(),
    };

    let json = serde_json::to_vec(&meta).map_err(|e| format!("元数据序列化失败: {e}"))?;
    let sealed = seal(key, metadata_nonce(), &json)?;

    let mut wire = offer.clone();
    wire.preview_summary = String::new();
    for it in wire.items.iter_mut() {
        it.relative_path = String::new();
        it.sha256 = String::new();
    }
    wire.encrypted = true;
    wire.e2ee_version = Some(E2EE_VERSION);
    wire.encrypted_metadata = Some(B64.encode(sealed));
    Ok(wire)
}

/// 接收端：解密并把字段还原回 `offer`。
///
/// 失败即整份 OFFER 不可用——调用方**必须**就地拒绝（回 `accepted=false`），
/// 不能带着空文件名继续走：`lib.rs` 收到 OFFER 后会立刻 emit 给界面并写历史行，
/// 空文件名一旦落库，后续展示与修剪都会带着它。
///
/// 最常见的失败原因是**两端 PSK 不一致**，不是篡改。提示文案要先指向这个。
pub fn open_offer_metadata(
    offer: &mut TransferOfferPayload,
    key: &aead::LessSafeKey,
) -> Result<(), String> {
    let b64 = offer
        .encrypted_metadata
        .as_ref()
        .ok_or_else(|| "OFFER 标记为加密但没有 encrypted_metadata".to_string())?;
    let sealed = B64
        .decode(b64)
        .map_err(|e| format!("encrypted_metadata 不是合法 base64: {e}"))?;
    let json = open(key, metadata_nonce(), &sealed)?;
    let meta: EncryptedMetadata =
        serde_json::from_slice(&json).map_err(|e| format!("元数据反序列化失败: {e}"))?;

    offer.preview_summary = meta.preview_summary;
    for m in meta.items {
        // 按 item_index 对位回填，不靠数组下标——两边的顺序没有任何机制保证一致。
        if let Some(it) = offer.items.iter_mut().find(|i| i.item_index == m.item_index) {
            it.relative_path = m.relative_path;
            it.sha256 = m.sha256;
            it.is_dir = m.is_dir;
        }
    }
    Ok(())
}

/// 判断对端 `app_version` 是否支持 E2EE。
///
/// **协商必须发生在构造 OFFER 之前**。原设计是「发 OFFER → 等 ANSWER 带回能力位」，
/// 有两处致命伤：
/// 1. 元数据在发 OFFER 的那一刻就已经抹空了，等 ANSWER 回来才决定已经太晚——
///    老接收端会照常 `accepted=true`（lib.rs 收到 OFFER 即无条件应答），
///    然后拿着空文件名与空哈希必然失败；
/// 2. ANSWER 根本带不回自定义字段：`control_ws.go:388` 在授权成功后
///    `env.Payload, _ = json.Marshal(answer)` 重建载荷，Go 会静默丢弃未知字段。
///    （OFFER 路径不同，它确实原样转发原始 payload——这个不对称会绊倒
///    任何下一个想给 ANSWER 加字段的人。）
///
/// 所以改读在线设备表里已有的 `app_version`，单次 OFFER 即定形态，
/// 且仍然**服务端零改动**（只读已有字段，不加新字段）。
///
/// 取不到版本时**按不支持处理**（保守回落明文）。乐观假设支持会让接收端
/// 收到一份它解不开的 OFFER，正是上面第 1 条要避免的形态。
pub fn peer_supports_e2ee(app_version: Option<&str>) -> bool {
    let Some(v) = app_version else {
        return false;
    };
    parse_semver(v)
        .map(|got| got >= E2EE_MIN_VERSION_TUPLE)
        .unwrap_or(false)
}

/// 支持 E2EE 的最低客户端版本，即发布本功能的版本号。
///
/// 刻意**不**同时留一份字符串形式：两份会漂移，而它只有比较这一个用途。
/// 将来 UI 要展示时从这里格式化，不要再加一个并列的字面量。
const E2EE_MIN_VERSION_TUPLE: (u32, u32, u32) = (0, 3, 0);

/// 解析 `x.y.z`，预发布标识整体忽略。解析不出来就是 `None`——调用方按不支持处理，
/// 不要在这里"尽量猜"：猜错的代价是对端收到解不开的 OFFER。
///
/// **必须先在第一个 `-` 处截断再按点分段**，不能只截 patch 段：
/// `0.3.0-beta.1`（cargo 完全合法的预发布号）按点分会得到四段，
/// 之前的实现会把它判成"非法版本"⇒ 不支持 ⇒ 静默回落明文。
/// 那恰好发生在**用预发布包验证本功能**的时候，而现象（回落 + 提示）
/// 与原因（版本号里多了个点）之间毫无线索。
fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    // 预发布与构建元数据一律丢弃，只比较数字三元组。
    let core = v.trim().split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    if it.next().is_some() {
        return None; // 四段及以上不是我们发布的版本号形态
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_for(session: &Uuid) -> aead::LessSafeKey {
        derive_session_key("test-psk", session, "acct").unwrap()
    }

    /// 派生必须是确定性的：两端各算一次要得到同一把 key，
    /// 否则一切都解不开。这条红了说明 HKDF 参数被改动过。
    #[test]
    fn derive_is_deterministic() {
        let s = Uuid::new_v4();
        let a = derive_session_key("psk", &s, "acct").unwrap();
        let b = derive_session_key("psk", &s, "acct").unwrap();
        let n = data_nonce(0, 0);
        let ca = seal(&a, n, b"hello").unwrap();
        // 用 b 解 a 的密文——能解开就证明两把 key 相同。
        let pt = open(&b, n, &ca).unwrap();
        assert_eq!(pt, b"hello");
    }

    /// 三个输入各自都必须影响 key。删掉其中任一个参与派生的输入，
    /// 这条就会退回成「不同会话/不同账号共用一把 key」。
    #[test]
    fn every_input_changes_the_key() {
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let n = data_nonce(0, 0);
        let base = seal(&derive_session_key("psk", &s1, "acct").unwrap(), n, b"x").unwrap();

        for (psk, sess, acct, what) in [
            ("psk2", &s1, "acct", "psk"),
            ("psk", &s2, "acct", "session_id"),
            ("psk", &s1, "acct2", "account_id"),
        ] {
            let other = derive_session_key(psk, sess, acct).unwrap();
            assert!(
                open(&other, n, &base).is_err(),
                "改变 {what} 之后仍能解开对方的密文——该输入没有真正参与派生"
            );
        }
    }

    /// 空 PSK 必须拒绝而不是派生出一把「空密钥」。
    #[test]
    fn empty_psk_is_rejected() {
        assert!(derive_session_key("", &Uuid::new_v4(), "acct").is_err());
    }

    /// 密文恰好比明文长 GCM_TAG_LEN。这个等号是 §B 块大小的全部依据：
    /// 它变了，`plaintext_chunk_len` 留的余量就不对，满块会超 4MB 上限被中继断连。
    #[test]
    fn ciphertext_is_exactly_tag_len_longer() {
        let k = key_for(&Uuid::new_v4());
        for len in [0usize, 1, 1000, 65536] {
            let pt = vec![7u8; len];
            let ct = seal(&k, data_nonce(0, 0), &pt).unwrap();
            assert_eq!(
                ct.len(),
                len + GCM_TAG_LEN,
                "明文 {len} 字节加密后应为 {} 字节",
                len + GCM_TAG_LEN
            );
        }
    }

    /// 满的加密块密文必须**恰好**等于 MAX_PAYLOAD_LENGTH，不是「小于等于」。
    /// 等号才能钉死那 16 字节的预留：写成 <= 的话，把 plaintext_chunk_len
    /// 改小一截也不会让测试变红，而那会白白浪费带宽。
    #[test]
    fn full_encrypted_chunk_exactly_fills_the_frame() {
        let k = key_for(&Uuid::new_v4());
        let pt = vec![0u8; plaintext_chunk_len(true)];
        let ct = seal(&k, data_nonce(0, 0), &pt).unwrap();
        assert_eq!(
            ct.len(),
            crate::protocol::binary_header::MAX_PAYLOAD_LENGTH as usize,
            "满块密文应恰好填满 4MB 上限"
        );
    }

    /// 不加密时块大小不变——加密路径的余量不能溢出到明文路径，
    /// 否则老服务端/老对端的明文传输会平白变慢且块边界错位。
    #[test]
    fn plaintext_path_keeps_the_original_chunk_size() {
        assert_eq!(
            plaintext_chunk_len(false),
            crate::protocol::binary_header::MAX_PAYLOAD_LENGTH as usize
        );
    }

    /// 遍历一次多条目多块传输的全部 (item, chunk)，nonce 必须两两不同。
    /// 这条是 GCM 安全性的地基：同 key 下 nonce 复用会直接摧毁认证性。
    #[test]
    fn data_nonces_never_collide_within_a_session() {
        let mut seen = std::collections::HashSet::new();
        for item in 0u32..8 {
            for chunk in 0u32..256 {
                assert!(
                    seen.insert(data_nonce(item, chunk)),
                    "nonce 在 item={item} chunk={chunk} 处重复"
                );
            }
        }
    }

    /// 元数据 nonce 与任何数据块 nonce 都不同——这是域分隔标记的全部职责。
    /// 去掉 nonce[8..12] 的域分隔后，metadata_nonce 会等于 data_nonce(0,0)，此条即红。
    #[test]
    fn metadata_nonce_is_disjoint_from_every_data_nonce() {
        let meta = metadata_nonce();
        for item in 0u32..4 {
            for chunk in 0u32..64 {
                assert_ne!(meta, data_nonce(item, chunk), "元数据 nonce 撞上了数据块 nonce");
            }
        }
    }

    /// 篡改密文任意一个字节都必须被 tag 抓住。这是 E2EE 相对 CRC32 的全部增量。
    #[test]
    fn tampering_is_detected() {
        let k = key_for(&Uuid::new_v4());
        let n = data_nonce(1, 2);
        let mut ct = seal(&k, n, b"sensitive clipboard content").unwrap();
        assert!(open(&k, n, &ct).is_ok());
        ct[3] ^= 0x01;
        assert!(open(&k, n, &ct).is_err(), "密文被改了一位却仍然解开了");
    }

    /// 用错 nonce 解不开——间接证明 nonce 真的参与了认证，
    /// 也是"接收端自行重算 nonce"这个决定的安全依据。
    #[test]
    fn wrong_nonce_fails_to_open() {
        let k = key_for(&Uuid::new_v4());
        let ct = seal(&k, data_nonce(0, 0), b"payload").unwrap();
        assert!(open(&k, data_nonce(0, 1), &ct).is_err());
    }

    /// 截断到比 tag 还短时给出明确错误，而不是 panic 或 underflow。
    #[test]
    fn truncated_ciphertext_is_rejected_cleanly() {
        let k = key_for(&Uuid::new_v4());
        assert!(open(&k, data_nonce(0, 0), &[0u8; GCM_TAG_LEN - 1]).is_err());
        assert!(open(&k, data_nonce(0, 0), &[]).is_err());
    }

    /// **门槛必须对我们自己成立**：本 crate 的版本要能通过 peer_supports_e2ee。
    ///
    /// 没有这条守卫时，`E2EE_MIN_VERSION_TUPLE` 与 `Cargo.toml` 的 `version`
    /// 是两个可以各自漂移的数字，而漂移的后果是**功能静默失效**——两台新客户端
    /// 互判不支持、永远回落明文，全套测试却照样全绿。
    /// 更糟的是下面那条 `peer_capability_gate` 一度把当时的包版本钉死为"不支持"，
    /// 等于把矛盾固化成了预期行为。这条守卫让「升版本」与「合并本功能」
    /// 必须在同一个变更里完成。
    #[test]
    fn our_own_version_passes_the_gate() {
        assert!(
            peer_supports_e2ee(Some(crate::app_state::APP_VERSION)),
            "本 crate 版本 {} 低于 E2EE 门槛 {:?}——E2EE 对所有对端都会回落明文",
            crate::app_state::APP_VERSION,
            E2EE_MIN_VERSION_TUPLE
        );
    }

    /// 版本门槛：低于 0.3.0 与取不到版本都必须判为不支持。
    /// 「取不到就按不支持」这条被改成乐观假设的话，对端会收到解不开的 OFFER。
    #[test]
    fn peer_capability_gate() {
        assert!(!peer_supports_e2ee(None), "版本未知必须保守判为不支持");
        // 这两个是**发布 E2EE 之前**已经存在的版本，必须判不支持。
        // 不要写成"当前包版本"——那正是 our_own_version_passes_the_gate 要防的矛盾。
        assert!(!peer_supports_e2ee(Some("0.2.0")));
        assert!(!peer_supports_e2ee(Some("0.1.1")));
        assert!(!peer_supports_e2ee(Some("乱七八糟")));
        assert!(!peer_supports_e2ee(Some("")));
        assert!(peer_supports_e2ee(Some("0.3.0")));
        assert!(peer_supports_e2ee(Some("0.3.1")));
        assert!(peer_supports_e2ee(Some("1.0.0")));
        // 预发布标识整体忽略，带点与不带点都要正确
        assert!(peer_supports_e2ee(Some("0.3.0-beta")));
        assert!(peer_supports_e2ee(Some("0.3.0-beta.1")), "带点的预发布号被误判为不支持");
        assert!(peer_supports_e2ee(Some("0.3.0-rc.1")));
        assert!(peer_supports_e2ee(Some("1.0.0+build.5")));
        // 截断不能把主版本号一起吃掉：0.2.0 的预发布仍然是不支持
        assert!(!peer_supports_e2ee(Some("0.2.0-rc.1")));
    }
}
