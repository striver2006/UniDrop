# E2EE 载荷加密

- 主题：`e2ee-payload-encryption`
- 阶段：plan（原稿）
- 日期：2026-09-14
- 角色：Driver (claude)

---

## Context

需求清单第 7 条「全程加密」目前是**部分实现**：上一轮做掉了传输层（TLS 默认校验服务器证书，
已实机验证），E2EE 仍是协议位占坑。本轮补上内容加密这一半。

### 为什么现在做，以及为什么它比想象中便宜

E2EE 通常最贵的三件事，这个项目**都已经预付过了**：

1. **密钥分发**——绝大多数 E2EE 项目的主要成本（设备配对、公钥交换、指纹验证、
   多设备密钥同步）。这里用户已经手动在每台设备填了同一把 PSK，`HKDF(psk, session_id)`
   直接派生会话密钥，**UX 零改动**。
2. **协议设计**——已经做完了。64 字节帧头 offset 40..51 是 `Nonce [12]byte`，
   注释写死 `AES-256-GCM 96-bit Nonce`；`FlagEncrypted = 1 << 0` 已定义；
   `Reserved [8]byte` 还空着。**不改帧头、不升协议版本。**
3. **依赖**——客户端 `rustls` 已带 `ring`（features = ["ring"]），AES-GCM 与 HKDF 都在里面。

### 威胁模型：能达成什么，不能达成什么

必须先把边界钉死，否则这轮会变成安全剧场。

**当前暴露面（已核实，非推测）**：

- [relay/pipe.go:26](../../../server/internal/relay/pipe.go#L26) 是 `ForwardChan chan *[]byte`，
  服务器内存里过的是完整明文；
- 客户端**零 P2P 实现**——STUN 服务端在 [main.go:57](../../../server/cmd/unidrop-server/main.go#L57)
  跑着，但客户端一行都没引用，**没有任何一条路径绕过服务器**；
- 而这台服务器按 DEPLOY.md 是部署在公网云上的。

叠加产品属性：剪贴板同步工具的内容密度特别高——密码、验证码、token、私钥片段，
恰恰是人们会复制粘贴的东西。

**本轮能达成**：服务器、云厂商、拿到 root 的人看不到内容。

**本轮达不成，且必须写进文档而不是含糊过去**：

| 达不成的 | 原因 |
| :--- | :--- |
| 前向保密 | PSK 长期不变，泄露后录下的历史流量全可解 |
| 防设备失窃 | PSK 明文躺在配置文件里（§G 缓解但不根治） |
| 设备间隔离 | 单 PSK 派生的是**组密钥**，任何持 PSK 的设备能解所有会话 |
| 元数据完全隐藏 | 服务端限额必须读 `total_size` / `data_type` / 每项 `size`（§E） |
| 低熵 PSK 的保护 | HKDF 不能凭空创造熵；用户填 `123456` 就是 `123456` |

最后一条要展开说：**这不比现状更弱**——同一把 PSK 今天已经在保护 HMAC 鉴权了。
但它确实意味着 E2EE 的强度上限由用户选的 PSK 决定，文档必须说明，
且 §G 的 keychain 迁移会顺带提示用户使用高熵 PSK。

要真正的前向保密就得每对设备 ECDH + 密钥轮换，成本高一个数量级，
且与「单 PSK 便利分区」的产品定位直接冲突。**本轮明确不走那条路。**

---

## 已核验的事实（供审查员对照，不是推测）

这些是方案的承重前提，每条都实际查过：

| 事实 | 位置 | 对方案的意义 |
| :--- | :--- | :--- |
| 中继 `DecodeBinaryHeader` 并校验 `PayloadLen` 与实际长度一致 | [data_ws.go:131,145](../../../server/internal/controller/data_ws.go#L131) | 密文长度必须如实填进 `PayloadLen` |
| `PayloadLen > MaxPayloadLength(4MB)` 直接报错断连 | [binary_header.go:172](../../../server/internal/protocol/binary_header.go#L172) | **§B 的硬冲突来源** |
| 中继**不验** CRC32（`VerifyPayloadCRC32` 只在测试里调） | data_ws.go 全文无调用 | CRC32 是纯端到端的 |
| 接收端**逐块验** CRC32，失败发 NACK 请求重传 | [transfer_engine.rs:682](../../../client/src-tauri/src/core/transfer_engine.rs#L682) | §D 要处理的正是这条路径 |
| 写盘定位 `offset = chunk_index * MAX_PAYLOAD_LENGTH` | [transfer_engine.rs:947](../../../client/src-tauri/src/core/transfer_engine.rs#L947) | 块大小一变，这里必须跟着变，否则文件静默错位 |
| `total_chunks = ceil(size / MAX_PAYLOAD_LENGTH)` | transfer_engine.rs:127,185 | 同上 |
| 服务端限额只读 `TotalSize` / `DataType` / 每项 `Size` | [limits.go:183,207,228,263](../../../server/internal/limits/limits.go#L183) | **§E 的答案**：不读 `PreviewSummary` |
| `Payload json.RawMessage` 原样透传 | [envelope.go:40](../../../server/internal/protocol/envelope.go#L40) | 加新字段**服务端零改动** |
| `TransferItemPayload` 含 `RelativePath` 与 `SHA256`，全明文 | [envelope.go:127-134](../../../server/internal/protocol/envelope.go#L127) | **§E 的缺口**：比 preview_summary 更严重 |
| `session_id` 是每次 `prepare_offer` 现铸的 UUIDv4 | transfer_engine.rs:89,170 | §C 的唯一性根基 |

**服务端零改动**是本方案的一个重要性质：中继不解密、不验 CRC32、限额只读未加密字段、
payload 原样透传。整轮 diff 预计只落在 `client/`，这也把审查面缩到最小。
（`EncryptedMetadata` 字段服务端 envelope 里已经有了，见 envelope.go:144。）

---

## 范围

**做**：数据面 payload 加密、条目元数据加密、能力协商、PSK 进系统 keychain。

**不做**（各有理由，不是漏了）：

- 前向保密与每设备对密钥——见上文威胁模型；
- 本地 SQLite 与缓存的静态加密——主要威胁（设备失窃）已被 OS 全盘加密覆盖
  （FileVault / BitLocker / LUKS），密钥存放是死结，SQLCipher 会显著抬高编译复杂度，
  且与断点续传要的随机 seek 冲突。**独立评估后判定不值得做**，不是本轮排期不下；
- ACK / NACK / Probe 帧加密——它们只含 `session_id` + `item_index` + `chunk_index`，
  无内容可泄露，加密只增加失败模式。**要写注释说明这是有意的**，
  否则后来人会认为漏了一类帧。

---

## A. 会话密钥派生（焦点 1）

```
key = HKDF-SHA256(
    ikm  = psk_secret 的 UTF-8 字节,
    salt = session_id 的 16 字节 UUID（不是它的字符串形式）,
    info = b"UNIDROP-E2EE-v1|" || account_id 的 UTF-8 字节
) → 32 字节（AES-256）
```

逐项理由，**全部写进代码注释**：

- **为什么不拿 PSK 直接当 key**：PSK 是用户手输的任意长度字符串，
  AES-256 要的是恰好 32 字节的均匀随机量。HKDF 做长度归一化与熵提取，
  这是它的本职。直接截断或补零是典型误用。
- **salt 用 session_id**：每次传输现铸 UUIDv4 ⇒ 每次传输一把新 key。
  这是 §C nonce 唯一性论证的根基——**key 换了，nonce 空间就是全新的**。
- **salt 用 16 字节原始 UUID 而非字符串**：两端必须逐字节一致，
  字符串形式有大小写与连字符的歧义（`Uuid::to_string()` 是小写带连字符，
  但没有任何机制强制它不变）。原始字节没有这个问题。
- **info 带版本前缀**：`UNIDROP-E2EE-v1` 是域分隔，将来换算法/换构造时
  老 key 不会被新用途复用。这是 HKDF 的标准用法，成本为零。
- **info 带 account_id**：把 key 绑定到账号上下文。UUIDv4 碰撞概率可忽略，
  所以这条不是为了防碰撞，而是让「同一 PSK 不同账号」的两条传输在密钥层就分开——
  与 registry 复合键、本地历史分区是同一条思路的延续。

实现用 `ring::hkdf`。**不新增依赖**：`ring` 已经在依赖树里（rustls 的 features）。
但注意它今天是**间接**依赖，要在 `Cargo.toml` 显式加一行 `ring = "0.17"`，
版本必须与 rustls 解析出的一致（实施时 `cargo tree -p ring` 确认，
不一致会让依赖树里出现两份 ring，编译能过但徒增体积）。

---

## B. 块大小与 4MB 上限的硬冲突（必须先解决）

**这是整个方案里最容易被漏掉、漏掉就必然坏的一条。**

AES-GCM 密文 = 明文长度 + 16 字节 tag。而发送端今天按满 `MAX_PAYLOAD_LENGTH`（4MB）切块，
加密后 payload = 4MB + 16 > 4MB，中继 `DecodeBinaryHeader` 当场 `ErrPayloadTooLarge` 断连。
**加密路径会 100% 失败，且失败点在服务端，客户端只看到连接被关。**

解法：**加密时明文块大小取 `MAX_PAYLOAD_LENGTH - 16 = 4194288`**，密文正好 4MB，不触上限。

引入一个显式常量并让它成为唯一事实源：

```rust
/// GCM tag 长度。密文 = 明文 + 这么多字节。
pub const GCM_TAG_LEN: usize = 16;

/// 加密传输的**明文**块大小。密文恰好填满 MAX_PAYLOAD_LENGTH。
/// 不加密时明文块大小仍是 MAX_PAYLOAD_LENGTH。
pub const PLAINTEXT_CHUNK_LEN: usize = MAX_PAYLOAD_LENGTH as usize - GCM_TAG_LEN;
```

**关键：块大小必须由「本次传输是否加密」决定，不能改成全局常量。** 三处跟着走：

| 位置 | 现状 | 改动 |
| :--- | :--- | :--- |
| `prepare_offer` / `prepare_offer_from_bytes` 算 `total_chunks` | `ceil(size / MAX_PAYLOAD_LENGTH)` | 除数换成本次传输的明文块大小 |
| 发送端 `all_chunks` 的 `offset` / `length` | `c * MAX_PAYLOAD_LENGTH` | 同上 |
| 接收端 `write_payload_chunk` 的写入 offset | `chunk_index * MAX_PAYLOAD_LENGTH` | 同上 |

第三处最危险：**它的失败形态是静默的**。offset 用错不会报错、不会 CRC 失败、
不会 tag 失败——每一块都成功解密、成功写入，只是写在了错误的位置，
最后 SHA256 校验才发现文件坏了，而那时用户已经等完了整个传输。

所以：**块大小不能靠三处各自记得改，要从一个函数取**。
计划新增 `fn plaintext_chunk_len(encrypted: bool) -> usize`，三处全部改为调用它，
注释写明「这三处必须同源，否则文件会静默错位」。

---

## C. Nonce 构造与唯一性（焦点 3）

GCM 的灾难性失败是**同一 (key, nonce) 加密两份不同明文**——会直接泄露明文异或值并摧毁认证性。
这一节的全部工作就是论证它不会发生。

### 构造

12 字节 nonce，填进帧头已预留的 `Nonce [12]byte`：

```
nonce[0..4]  = item_index   (u32, 大端)
nonce[4..8]  = chunk_index  (u32, 大端)
nonce[8..12] = 域分隔标记   (u32, 大端)
               0x00000000 = 数据块
               0x00000001 = 条目元数据（§E）
```

`nonce[8..12]` 这 4 字节正好把之前空着的保留空间用起来做**域分隔**：
数据块与元数据用同一把 key，但 nonce 空间完全不相交，
不必论证「item_index 不会恰好取到某个魔数」。

### 唯一性论证（四种情况）

1. **同一 session 内的不同块**：`(item_index, chunk_index)` 唯一标识一个块，
   两者都是 u32 且由发送端顺序生成 ⇒ 不重复。✓
2. **不同 session**：key 由 `HKDF(psk, session_id)` 派生，session_id 是现铸 UUIDv4
   ⇒ **key 不同，nonce 复用无害**。✓
3. **重传**（滑动窗口 + NACK 触发）：重传的是**同一块的同一份明文**。
   相同 (key, nonce, plaintext) 产出相同密文——这不是 GCM 的失败形态，
   它要求的是「相同 (key,nonce) 加密**不同**明文」。✓
   **这一条要写进注释**，否则后来人看到「重传复用 nonce」会以为是 bug 而去"修"它，
   反而可能引入随机 nonce 导致真正的问题。
4. **断点续传**：接收方已有的块不重发，只是少发，不产生新的 (key,nonce) 组合。✓

### 唯一真正的风险，以及它需要被验证

上述论证**完全依赖「session_id 不被复用于第二次、内容不同的传输」**。

如果 UI 上的「重试」走的是复用 session_id 的路径，而用户在重试前改动了文件内容，
就会出现同 key 同 nonce 加密不同明文——**这是本方案唯一的灾难性失败点**。

我核到 `session_id` 在 `prepare_offer` / `prepare_offer_from_bytes` 里现铸
（`Uuid::new_v4()`），但**没有穷尽所有调用路径**。
实施时必须逐一确认，并补一条测试钉死「两次传输的 session_id 必不相同」。
**这一条列为给审查员的重点提示第 1 条。**

### 接收端不信任帧头里的 nonce

发送端填写 nonce 字段（便于抓包调试），但**接收端自行从 `item_index` / `chunk_index` 重算**，
忽略帧头值。

理由：信任帧头会让攻击者通过改 nonce 制造解密失败。虽然后果与直接改密文相同
（都是 tag 校验失败），但"自己算"消除了一整类需要论证的问题，成本为零。
注释要写明这是有意的冗余，不是忘了用。

---

## D. CRC32 与 GCM tag 的职责划分（焦点 2）

### 现状澄清

我一度以为 CRC32 是死字段，**核实后是错的**：接收端在 transfer_engine.rs:682
逐块验证，失败发 NACK 请求重传。它是**活的传输纠错机制**，不是装饰。
服务端确实不验（`VerifyPayloadCRC32` 只出现在测试里）。

所以这不是「清理死代码」，是两个都活着的机制要划清职责。

### 决策：CRC32 保留，且**算在密文上**

```
明文块 --加密--> 密文(含tag) --算CRC32--> 填帧头 --发送-->
接收: 验CRC32 --失败--> NACK 重传
            --通过--> 解密验tag --失败--> §D 的计数逻辑
                              --通过--> 写盘
```

**为什么算密文而不是明文——这是本节最重要的一条**：

CRC32 算在明文上会把明文的 32 位指纹明文写进帧头，而帧头对中继完全可见。
对短内容（剪贴板文本正是短内容）这足以**暴力枚举确认明文**：
攻击者猜一个候选文本，算 CRC32，对上了就确认了。加密了正文却在帧头附赠校验和，
等于自己拆掉一半。

算在密文上则不泄露任何明文信息，且保持「解密前就能拒绝坏块」——
不必对已知损坏的数据做无用的 AES 运算。

**为什么不干脆删掉 CRC32**：它和 GCM tag 的功能确实重叠（tag 覆盖 CRC32 能检测的一切，
且强度高得多），但两者的**响应路径不同**：

- CRC32 失败 ⇒ 传输损坏 ⇒ NACK ⇒ 重传能修复；
- GCM tag 失败 ⇒ 可能是损坏，**也可能是篡改** ⇒ 重传对篡改无效。

删掉 CRC32 会让所有损坏都走 tag 失败路径，而那条路径（见下）必须限次，
于是**偶发的线路损坏会被当成攻击而中止整个会话**。保留 CRC32 让两类失败在
绝大多数情况下自然分流，代价只是每块一次 CRC32（相对 4MB AES-GCM 可忽略）。

### GCM tag 校验失败的处理（必须设计，否则是个洞）

tag 失败**不能简单地无限 NACK 重传**——篡改会稳定复现，于是变成无限重传循环，
表现为传输永远不结束，而用户看不到任何错误。

规则：

- 同一 `(item_index, chunk_index)` 的 tag 失败计数；
- 阈值内（建议 3 次）按损坏处理，发 NACK 重传（CRC32 通过而 tag 失败确实可能是
  极罕见的 CRC32 漏检，给它机会）；
- 超阈值 ⇒ 判定为篡改或密钥不匹配 ⇒ **中止整个会话**，`TRANSFER_FAILURE`
  带专门的 error_code（如 `E2EE_AUTH_FAILED`），历史行落 FAILED，
  界面提示要能区分「对方密钥不一致」与「内容被篡改」——
  实际最常见的原因是**两端 PSK 不同**，提示必须先指向这个。

这条要配测试：构造一个 tag 被改坏的块，断言会话在有限次内中止而不是挂死。

---

## E. 元数据加密与限额（焦点 4）

### 焦点 4 的直接答案

**服务端限额完全不受 `preview_summary` 搬迁影响**——已核实 limits.go 只读
`TotalSize`（183/207/263）与 `DataType`（228），**从不读 `PreviewSummary`**。

### 但只搬 preview_summary 是不够的

`TransferItemPayload` 里还有两个字段明文过服务端，都比 preview_summary 严重：

- **`relative_path`**：完整文件名与目录结构；
- **`sha256`**：内容的密码学指纹。这条尤其糟——不需要暴力枚举，
  **查表就能精确确认你传了哪个已知文件**。加密了正文却附赠内容指纹，
  和 §D 里 CRC32 算明文是同一类错误。

### 切分：服务端必须看见什么

| 字段 | 服务端是否需要 | 处置 |
| :--- | :--- | :--- |
| `total_size` | 需要（总量限额） | 明文保留 |
| `data_type` | 需要（按类型分流限额） | 明文保留 |
| `total_items` | 需要（条目数限额） | 明文保留 |
| `items[].size` | 需要（单文件限额） | 明文保留 |
| `items[].item_index` | 接收端建 bitmap 要 | 明文保留 |
| `items[].total_chunks` | 接收端建 bitmap 要 | 明文保留 |
| `preview_summary` | **不需要** | 加密 |
| `items[].relative_path` | **不需要**（接收端要，解密后取） | 加密 |
| `items[].sha256` | **不需要**（接收端要，解密后取） | 加密 |
| `items[].is_dir` | 不需要 | 加密（顺带，它也泄露结构） |

### 做法

`encrypted = true` 时，把敏感字段打成一个 JSON，用同一把 session key 加密
（nonce 域分隔标记 = `0x00000001`，见 §C），base64 后放进已有的 `encrypted_metadata`：

```json
{
  "preview_summary": "...",
  "items": [ { "item_index": 0, "relative_path": "...", "sha256": "...", "is_dir": false } ]
}
```

明文 `items[]` 里对应字段置空/置零。接收端在收到 OFFER 时即可解密
（它有 PSK 与 session_id），流程上不存在"要先建文件才知道文件名"的循环依赖。

**PathGuard 的位置不变**：接收端解密出 `relative_path` 之后仍走
`PathGuard::sanitize_and_resolve`。加密不改变「路径来自对端、必须当作不可信输入」
这一事实——注释要写明，否则有人会觉得"加密过的路径可信"。

### 剩余的元数据泄露（写进文档，不假装解决了）

服务端仍能看到：谁传给谁、什么时间、多大、几个条目、每个条目多大、是文本还是文件。
这是限额机制的必然代价——**除非放弃服务端限额，二者不可兼得**。
文档要把这句话写出来，而不是让人以为 E2EE 之后服务端就瞎了。

---

## F. 能力协商与兼容性

### 为什么必须协商

老客户端的 `encrypted` 字段是 bool，反序列化能过，但它**不会解密**——
会把密文当明文写盘，SHA256 校验失败，用户看到「传输失败」而不知道为什么。
更糟的是文件已经落盘（坏的）。

### 做法（服务端零改动）

`Payload` 是 `json.RawMessage` 原样透传，所以客户端可以自行加字段：

- OFFER 增加 `e2ee_version: u8`（1 = 本方案）。老客户端忽略未知字段；
- ANSWER 增加 `e2ee_ok: bool`。接收端能解密才置 true；
- **发送端只在 ANSWER 明确 `e2ee_ok = true` 时才加密**。

于是三种组合：

| 发送端 | 接收端 | 结果 |
| :--- | :--- | :--- |
| 新 | 新 | 加密传输 |
| 新 | 老（无 `e2ee_ok`） | **回落明文**，并在发送端界面提示"对端版本过旧，本次未加密" |
| 老 | 新 | 明文传输，接收端提示同上 |

回落必须**可见**——静默回落明文是安全功能里最糟的反模式：
用户以为加密了，实际没有。提示文案要指明是哪台设备版本过旧。

### 设置项

`AppSettings.e2ee_enabled: bool`，`#[serde(default)]`。

**默认值取 `true` 还是 `false`，这是个需要审查员表态的取舍**：

- `true`（倾向）：安全默认。但 `bool` 的 `Default` 是 `false`，
  要写自定义 default 函数——与 `allow_insecure_tls` 恰好相反（那里裸 default 就是安全值），
  两处注释要互相指向，否则看起来像不一致的风格；
- `false`：升级后行为不变，但绝大多数用户永远不会去开它，等于白做。

倾向 `true` + 可见回落。因为回落机制已经保证了「对端老版本不会坏」，
默认开启的风险主要是 PSK 不一致时的失败——而 PSK 不一致时今天的鉴权本来就过不去。

### 断点续传的跨版本兼容

`chunk_bitmaps` 按 session 存，同一 session 的加密状态在 OFFER 时就固定、中途不变，
所以**不存在同一 session 内块边界不一致**的问题。
但升级客户端后恢复一个**升级前**创建的未完成 session：
它的 bitmap 按 4MB 边界记录，而新的加密传输按 4MB-16 切——块边界错位。

处置：`transfer_tasks` 记录本次会话的 `e2ee_version`（NULL = 升级前的老行）。
恢复续传时若记录的版本与当前不一致，**丢弃 bitmap 从头传**，而不是尝试对齐。
理由写注释：续传是优化不是正确性要求，为它做跨版本块边界映射的复杂度
远超重传一次的代价，而映射错了就是静默的文件损坏。

---

## G. PSK 进系统 keychain

今天 PSK 是明文 JSON 存盘（已核实：`Cargo.toml` 无 keyring/keychain 依赖，
`cmd_save_settings` 直接 `serde_json::to_string` 落盘）。

今天它只是接入凭据，泄露了顶多被人蹭中继；**做了 E2EE 之后它变成「解密全部内容的钥匙」**，
那时明文存盘就说不过去了。所以这不是独立的"静态加密"项目，
而是 §A 的**前置依赖**——它决定了 §A 派生出的 key 的实际安全下界。

做法：引入 `tauri-plugin-store` 之外的 `keyring` crate（macOS Keychain /
Windows Credential Manager / Linux Secret Service），PSK 单独存，
配置 JSON 里只留一个占位标记。

**必须处理的失败路径**（这是这一节的主要工作量，不是接 API）：

- Linux 无 Secret Service（headless / 精简桌面环境）⇒ keyring 不可用。
  **不能因此拒绝启动**，要回落到现有的明文存储并在界面明示；
- 已有明文 PSK 的迁移：首次启动读到明文 PSK ⇒ 写入 keychain ⇒ 清除明文副本。
  迁移失败要保留明文，不能两头落空；
- keychain 读取失败（用户拒绝授权、钥匙串锁定）⇒ 提示而不是静默用空 PSK 连接
  （空 PSK 会让鉴权失败，表现成"莫名连不上"）。

**这一节可以拆出去单独做**。如果审查员认为本轮范围过大，
优先级排序是：§B > §C > §D > §E > §F > §G。§G 拆走不影响前面任何一节的正确性，
只是让 §A 的安全下界停留在现状。

---

## H. 测试

风格沿用既有约定：中文 `t.Fatalf` / `assert!` 文案，每条测试上方写明
「这条断言被删掉之后会退回成什么缺陷」。**每条新增测试都要实测撤掉被测逻辑会红**——
这是前两轮反复吃亏的地方（假绿测试、测了逻辑没测接线）。

- **密钥派生**：同 (psk, session_id, account_id) 派生稳定；任一输入变化 ⇒ key 变化；
  key 恰好 32 字节。
- **Nonce 唯一性**：遍历一次多条目多块传输的全部 (item,chunk)，断言 nonce 集合无重复；
  **`two_transfers_never_share_session_id`**——钉死 §C 那个唯一的灾难性失败点。
- **块大小**（§B 的三处同源）：满块加密后密文长度**恰好等于** `MAX_PAYLOAD_LENGTH`
  （不是"小于等于"——等号才能钉死那 16 字节的预留）；
  `total_chunks` 与写盘 offset 用同一个函数取块长。
  再补一条**端到端错位守卫**：构造一个跨 3 块的文件，加密传输后比对字节完全一致——
  offset 用错时这条会红，而单看每块都成功。
- **CRC32 算在密文上**（§D）：断言帧头 checksum **等于**密文的 CRC32、
  **不等于**明文的 CRC32。后半条是真正的守卫——它防的是有人"顺手"改回明文。
- **tag 失败限次**：篡改某块密文，断言会话在有限次重传内中止并落 `E2EE_AUTH_FAILED`，
  而不是无限 NACK。
- **元数据加密**（§E）：加密传输的 OFFER JSON 里**不含**明文文件名与 sha256
  （直接对序列化后的字符串做子串断言，这比检查结构体字段更能防回归）；
  接收端解密后能正确还原并仍走 PathGuard。
- **能力协商回落**（§F）：ANSWER 无 `e2ee_ok` ⇒ 发送端不加密且发出提示事件；
  老 bitmap + 新版本 ⇒ 丢弃重传。
- **服务端不回归**：`go test ./...` 全绿即可——本轮不改服务端，
  这是确认"零改动"这个判断成立的手段。

---

## 验证

```bash
cd server && go test ./... && go test -race ./...   # 应全绿且无改动
cd client/src-tauri && cargo test
cd client && npm test && npx tsc --noEmit
```

手工端到端（自动化覆盖不到"两个真实客户端"）：

1. 两台新版客户端、相同 PSK ⇒ 传文件与剪贴板文本成功，内容正确；
2. 服务端开 debug 日志抓中继帧 ⇒ **确认 payload 不可读、且 OFFER 里无明文文件名**。
   这是本轮唯一能直接证明目标达成的验证，必须做；
3. 两台客户端 **PSK 不同** ⇒ 传输在有限次内失败，提示指向"密钥不一致"而非挂死；
4. 新版发给旧版 ⇒ 明文回落且**界面有可见提示**；
5. 传输途中断网重连 ⇒ 续传正确，文件 SHA256 通过（验 §B 的 offset）；
6. §G 若纳入：macOS 上确认 PSK 已进钥匙串且配置文件里不再有明文。

---

## 给审查员的重点提示

用户点名的四个承重判断，加上我在调研中发现的三个，按重要性排序：

1. **§C 的 session_id 复用风险**——本方案唯一的灾难性失败点。
   我核到 `prepare_offer` / `prepare_offer_from_bytes` 现铸 UUIDv4，
   但**没有穷尽所有调用路径**（UI 重试、失败恢复、断点续传恢复）。
   如果任何一条路径复用 session_id 传不同内容，GCM 会直接崩。
   请独立验证这条，不要接受我的结论。
2. **§B 的块大小三处同源**——漏改任一处的失败形态是**静默文件错位**，
   每块都解密成功、写入成功，只有最后 SHA256 才发现。
   请确认三处（total_chunks、发送 offset、写盘 offset）确实都被覆盖，
   以及我提的"端到端错位守卫"测试是否真能抓住。
3. **§D 的 CRC32 算密文而非明文**——算明文会泄露明文指纹给中继，
   对剪贴板短文本可暴力枚举。请核我这个判断是否成立，
   以及"保留 CRC32 做损坏分流"是否值得（对立方案是删掉它，全靠 tag）。
4. **§E 的 sha256 与 relative_path**——我上一轮只提了 preview_summary，
   这两个是补上的缺口。请核对"服务端限额只读 TotalSize/DataType/items[].Size"
   这个结论，因为整个切分建立在它之上；漏掉任何一个服务端实际会读的字段，
   加密后限额就会静默失效或误拒。
5. **§F 的默认值取舍**——`e2ee_enabled` 默认 true 还是 false，我倾向 true，
   但这直接影响升级后所有用户的行为，请表态。
6. **§D 的 tag 失败限次**——阈值 3 是拍的。请判断这个机制是否会把
   "偶发线路损坏"误判成篡改而中止正常传输。
7. **§G 是否该拆出去**——本轮范围是否过大。我给了优先级排序，
   但如果你们认为 §A 在 PSK 明文存盘的前提下不该上线，请明说。

另外两点方法论请求：

- 我在本轮调研中**自己纠正过一次判断**（先认定 CRC32 是死字段，
  实际接收端在验，是我搜错了函数名）。请不要把我写的"已核验事实"当成前提照单全收，
  那张表里任何一条错了，建立在它上面的设计就是错的。
- 本计划的密码学部分是本仓库第一次引入密码学代码。
  请特别留意"看起来对但实际有标准反模式"的地方——
  nonce 构造、HKDF 参数顺序（ikm/salt/info 用反是经典错误）、
  以及 tag 长度是否被正确计入长度计算。
