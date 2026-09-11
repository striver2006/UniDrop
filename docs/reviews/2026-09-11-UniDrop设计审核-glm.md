# UniDrop 系统详细设计说明书(LLD)审核意见

| 项目 | 内容 |
| :--- | :--- |
| 审核对象 | [DESIGN.md](../design/DESIGN.md)(UniDrop 系统详细设计说明书 v1.0.0,状态:Approved / Ready for Implementation) |
| 审核日期 | 2026-09-11 |
| 审核人 | GLM |
| 审核维度 | 架构合理性、协议完备性、安全性、平台实现正确性、可靠性、可测试性、文档自洽性 |

---

## 一、总体结论

文档整体完成度较高:控制面/数据面分离的拓扑清晰,信令与二进制帧职责划分合理,三平台剪贴板注入的技术细节(CF_HDROP 内存布局、Preferred DropEffect、nspasteboard ConcealedType 等)体现了对领域的真实理解,工程要素(断点续传、限流、LRU/TTL、目录规范)覆盖面广。

但以文档自标的 **"Approved / Ready for Implementation"** 状态衡量,**尚不具备直接进入编码的条件**。存在 6 项 P0 阻塞问题:数据传输缺少 ACK/重传闭环(可靠性模型不成立)、E2EE 设计存在 MITM 漏洞与协议预留不足、路径穿越安全漏洞、macOS 注入代码存在内存安全问题、核心交互模型前后矛盾、数据面承载协议未收敛。此外文档多处出现 "A or B" 式未决选型,与 "Approved" 状态不符。

**建议:修订后复审。** 问题统计:P0 × 6,P1 × 12,P2 × 14,P3 × 10。

---

## 二、P0 —— 阻塞级问题(必须在实现前解决)

### P0-1 数据传输缺少 ACK / NACK / 重传闭环,可靠性模型不成立

- **位置**:DESIGN.md:113-133(ActionType 枚举)、DESIGN.md:222-269(二进制帧)、DESIGN.md:665-690(滑动窗口)
- **问题**:文档宣称面向"公网高丢包、高延迟网络跑满带宽"(DESIGN.md:667),设计了"滑动窗口(Window Size = 4)",但:
  1. `ChunkType` 枚举只有 4 种数据帧(文本/图片/中间块/EOF),**没有任何确认帧类型**;ActionType 中也没有逐块 ACK / NACK / 重传请求信令,`TRANSFER_PROGRESS` 仅是"可选汇报"(DESIGN.md:127),无法承载逐块确认语义。
  2. 没有 ACK,"In Flight → Done" 的状态迁移条件就不存在,窗口无法滑动——当前设计实为**并发流水线**,不是滑动窗口,"高丢包网络下跑满带宽"的目标(丢包必须重传)直接落空。
  3. 全文件 SHA-256 校验(DESIGN.md:690)失败后的恢复路径完全未定义:整文件重传还是逐块修复?无 `TRANSFER_FAILURE` 类信令,接收端 CRC 校验失败、写盘失败均无反向通知通道。
- **建议**:补充可靠传输设计——定义 CHUNK_ACK / CHUNK_NACK 帧(或信令),明确重传策略(超时重传 / 快速重传)、RTO 估算方式、窗口滑动条件;定义端到端校验失败后的错误恢复流程与 `TRANSFER_FAILURE` 信令。

### P0-2 E2EE 设计存在三处硬伤:MITM、IV 预留不足、明文元数据与"零知识"矛盾

- **位置**:DESIGN.md:268(Reserved 字段)、DESIGN.md:776-801(6.2 E2EE)
- **问题**:
  1. **无身份认证的 MITM 风险**:6.2 采用"生成临时会话密钥对(X25519 Ephemeral Key)"做 ECDH。纯临时 DH 没有任何身份验证,控制面信令又由中继服务器路由——**服务器自身即可发起中间人攻击**,双端毫无感知。"零知识(Zero-Knowledge)隐私保护"的声明不成立。客户端 SQLite 中已有"设备配对公钥"(DESIGN.md:83),但设计中未将其用于会话密钥的签名认证(应采用 Noise XK / SIGMA 类带认证的握手)。
  2. **IV 预留空间不足**:AES-256-GCM 标准 nonce 为 96 位(12 字节),而帧头 `Reserved` 仅 **8 字节**(DESIGN.md:268),放不下 GCM nonce。后期启用 E2EE 时必然破坏 48 字节定长头兼容性,属于协议结构性缺陷。
  3. **明文元数据泄露**:`TRANSFER_OFFER` 中的 `relative_path`(文件名)、`preview_summary`、`total_size`、`sha256` 全程以明文 JSON 经过服务器(DESIGN.md:168-192)。E2EE 只加密了文件内容,文件名和大小对服务器完全可见,与"服务器完全无法解密明文内容"的零知识承诺矛盾。E2EE 模式下 items 元数据应纳入加密或最小化。
  4. 另见 P1-13:GCM 密文块的可重排/重放问题未设计(AAD 未绑定 `chunk_index`/`item_id`)。
- **建议**:E2EE 握手改为基于设备长期身份密钥签名的认证密钥交换;帧头预留扩展到 12 字节以上(或采用加密后独立 trailer);明确 E2EE 模式下信令元数据的保护范围。

### P0-3 `relative_path` 未做路径穿越防护,接收端存在任意文件写入漏洞

- **位置**:DESIGN.md:174-191(items[].relative_path)、DESIGN.md:694-699(缓存目录拼接)
- **问题**:`relative_path` 由发送端完全控制,接收端将其拼接到缓存目录后写入。恶意/被入侵的发送端可传 `"../../.ssh/authorized_keys"` 一类路径实现**接收端任意位置文件写入**(目录穿越),还可借助 `is_dir` 构造目录逃逸。同账号下的设备互发并不天然可信——设备失窃、DeviceID 伪造、同账号多设备场景都构成现实威胁面。
- **建议**:接收端落盘前必须规范化路径(`canonicalize` / 词法清洗)并校验结果仍位于缓存根目录内;拒绝绝对路径、符号链接、`..` 段;同时处理 Windows 保留设备名(CON/NUL 等)、超长路径(260 限制)与 macOS 非法字符等平台边界。

### P0-4 macOS 注入示例代码存在真实缺陷:非 NUL 结尾字符串 UB + ObjC 对象泄漏

- **位置**:DESIGN.md:586-641(clipboard_macos.rs)
- **问题**:
  1. **未定义行为**:`msg_send![nsstring_cls, stringWithUTF8String: path_str.as_ptr()]`(DESIGN.md:622)——`stringWithUTF8String:` 要求 C 风格 NUL 结尾字符串,而 Rust `&str::as_ptr()` **不保证**末尾是 `\0`,越界读取属于 UB。必须使用 `CString`。
  2. **对象所有权完全不管理**:`stringWithUTF8String:`、`arrayWithCapacity:` 返回 autorelease 对象,Rust FFI 线程默认**没有 autorelease pool**,代码中 `msg_send_id` / `Id` 已 import 却未使用,所有 NSString / NSURL / NSMutableArray 在每次注入后泄漏。对于常驻进程 + 高频复制的场景,与"内存 ≤ 30MB"的硬指标直接冲突。
  3. 代码风格停留在 objc2 0.2 时代:`Class::get` 在新版 objc2 已移除/弃用(改为 `AnyClass::get` / 类型安全绑定 `objc2-foundation`),`msg_send!` 新版语法也已变更,照抄无法编译。设计文档中代码应注明所依赖的 crate 版本,并建议直接采用 `objc2-foundation` 的 `NSPasteboard` / `NSURL` 类型安全封装,从根上规避 1、2 两类问题。
  4. 文档 4.2.2 第 1 点声称 macOS 需同时写入 `NSPasteboardTypeFileURL` 与旧版 `NSFilenamesPboardType`(DESIGN.md:579-582),但代码只 `writeObjects:` NSURL,未声明旧类型——**文档自述与实现不一致**(现代 Finder 仅需 fileURL,建议直接修订文字,删去未实现的承诺)。
- **建议**:改用 `CString` + `objc2-foundation` 类型安全 API,补 autorelease pool(`objc2::rc::autorelease_pool`);修正 4.2.2 关于双类型的描述。

### P0-5 核心交互模型前后矛盾:自动进剪贴板 vs 点击通知后注入

- **位置**:DESIGN.md:14(1.1 核心设计理念)、DESIGN.md:63(1.2 拓扑图)、DESIGN.md:749-763(5.1 时序图)
- **问题**:1.1 明确写着 UniDrop 采用**"传输完成即进入系统剪贴板"**的机制(DESIGN.md:14);但 1.2 拓扑图(`Toast_B -->|用户点击| Clip_B`)与 5.1 时序图均为**用户点击通知后才注入剪贴板**。两种模型对剪贴板占用、误覆盖用户当前剪贴板内容的风险、通知交互设计的影响完全不同。核心产品行为在文档内没有定论,后续 UI、信令(是否需要 CLIPBOARD_INJECTED 回执)、测试验收标准都无从落地。
- **建议**:明确二选一(建议:点击通知注入,避免静默覆盖用户剪贴板),并全文统一;若保留自动注入,需补充剪贴板占用保护与用户当前内容的备份/恢复策略。

### P0-6 数据面承载协议未收敛:"HTTP/2 or WSS Binary" 二选一未定,连接复用规则缺失

- **位置**:DESIGN.md:69(拓扑图)、DESIGN.md:285-286(3.1 接口)、DESIGN.md:373-387(3.3.2)
- **问题**:
  1. 拓扑图标注数据面为 "HTTP/2 or WSS Binary",3.1 中 `POST /api/relay/upload` 却标注 "(备用)"——**主用路径到底是同一条 WSS 连接、独立 WSS 连接,还是 HTTP/2 上传,文档没有结论**。三者的心跳关联、断线语义、连接数、背压实现完全不同。
  2. 若数据帧与 JSON 信令复用同一条 WebSocket,必须定义消息区分规则(WebSocket text/binary 帧类型区分?还是加帧类型前缀?),以及二进制大帧阻塞控制信令的优先级/多路复用问题,文档均未提及。
  3. 3.1 文字说数据中继用 "双向 io.Pipe 与环形缓冲池"(DESIGN.md:299),3.2 的代码却是 `chan []byte`——**文字与代码不一致**。
- **建议**:数据面定为独立二进制通道(或 WSS 上 text=信令 / binary=数据的明确复用规范),给出连接生命周期图;统一 io.Pipe 与 chan 的表述。

---

## 三、P1 —— 重要设计缺陷(实现阶段必须补齐)

### P1-1 发送方向"剪贴板监听"机制完全缺失
1.2 拓扑图中存在"剪贴板监听 / 注入器"(DESIGN.md:29),但全文没有任何监听机制设计:Windows(`AddClipboardFormatListener` 消息窗口)、macOS(`NSPasteboard.changeCount` 轮询)、X11(`SelectionNotify` 事件)的事件源与线程模型,轮询频率与 CPU 开销(与"CPU < 0.1%"验收指标相关,DESIGN.md:912),去重策略(同一内容反复复制的抑制),以及 6.3 隐私标签过滤在监听链路中的落地点。这是发送方向的核心功能,属于**整章缺失**。

### P1-2 服务端拥塞超时静默丢块,且无重传兜底
`HandleDataChunk` 在接收端拥塞时 5 秒超时直接返回 `ErrReceiverCongested` 并**丢弃该块**(DESIGN.md:383-386)。结合 P0-1(无 ACK),发送端永远不知道块丢了,最终表现为文件悄悄损坏、只能靠整文件 SHA-256 兜底。应改为:拥塞时反压发送端(信令通知降速)或至少保证丢块可被上层重传机制发现。

### P1-3 鉴权与账号体系不完整
1. `auth_token: "hmac_sha256_or_jwt_signature"`(DESIGN.md:153)——HMAC 与 JWT 是不同机制,设计文档不应留 "or"。
2. HMAC-SHA256(SecretKey, Timestamp + DeviceID + Nonce) 的签名串拼接顺序、编码、SecretKey 的**分发方式**(配对二维码?预置?)、账号注册/设备绑定/密钥轮换与撤销流程全部未设计。`account_id` 字段孤立存在(DESIGN.md:148),账号模型(多用户?单用户自部署?)未界定。
3. nonce 防重放依赖服务端在 60s 窗口内查重缓存(DESIGN.md:774),但服务端为纯内存设计,**重启即失忆**,重启后 60s 内可重放握手。未设计 nonce 持久化或容忍边界说明。
4. DeviceID 由客户端自报(DESIGN.md:144),生成规则、与签名的绑定关系未定义,同账号下设备可互相冒充——威胁模型应明确写出。

### P1-4 服务端自称"无状态"名不副实,重启恢复与水平扩展未设计
3 章开头称"全生命周期基于内存会话路由,做到'无状态'或'轻会话'"(DESIGN.md:275),但 `DeviceRegistry`(sync.Map)与 `RelayPipe` 均为内存状态:服务端重启即全部在线状态丢失、传输中会话全部中断;多实例部署时 A、B 两端连到不同实例,`TRANSFER_OFFER` 按 to_device 路由即失败(sync.Map 不跨进程)。应明确**单实例部署假设**或补充实例间路由/会话迁出方案,并说明重启后客户端的重连与传输恢复流程(与断点续传联动)。

### P1-5 RelayPipe 孤儿会话回收与容量上限缺失
`RelayPipe` 有 `CreatedAt` 字段但全文无超时清理逻辑;双端掉线后管道滞留(32MB 缓冲)造成内存泄漏。且每会话 `DataChan` cap 8 × 4MB ≈ 32MB,**单实例并发中继会话数无上限、无全局内存水位控制**——100 个并发传输即最坏 3.2GB。8.1 的 10,000 并发 WebSocket 测试目标也未给出对应的内存容量估算。应定义:中继会话空闲超时、单实例最大并发传输数、全局缓冲配额与拒绝策略(`ErrServerBusy` + 客户端退避)。

### P1-6 断点续传协议是半成品
`TRANSFER_ANSWER.resumed_items` 汇报已有分片(DESIGN.md:210-215),但:发送端收到后如何响应(从块 2 续传的信令确认)、会话中断后 SessionID 如何复用(重新 OFFER 同 ID?)、块位图在 SQLite 的持久化 schema、缓存文件被 LRU **部分淘汰**后位图与实际文件不一致的检测,均未定义。P0-1 的 ACK 机制缺失也使续传状态无法对账。

### P1-7 Windows 代码剪贴板句柄泄漏
`SetClipboardData` 失败时系统**不接管** `hGlobal` 所有权,调用方必须 `GlobalFree`;代码在 CF_HDROP 写入失败(DESIGN.md:536-539)与 DropEffect 写入失败路径均未释放,长期驻留进程会累积泄漏。另建议用 `CF_HDROP` 常量替代硬编码 `15`(DESIGN.md:472),并说明剪贴板消息窗口(HWND 传 `0` 时 `EmptyClipboard` 将 owner 置空的边界)。

### P1-8 X11 Selection Owner 生命周期与线程模型未设计
X11 下剪贴板内容由 owner 窗口存活期决定:**owner 窗口/连接销毁,剪贴板即失效**。文档提到"守护虚拟 Window"(DESIGN.md:650),但未说明:该窗口由谁创建(托盘常驻的 Rust core?)、`SelectionRequest` 事件循环跑在哪个线程(与 Tokio runtime 的整合方式,x11rb 连接非线程安全)、用户退出应用后剪贴板内容即失效的产品预期。

### P1-9 Linux Wayland 两方案可行性存疑
1. **方案二事实性风险**:`org.freedesktop.portal.Clipboard` 并非 xdg-desktop-portal 的标准已实现接口(仅有提案),主流发行版上不可用,不能作为设计依赖。
2. **方案一**:`wl-copy` 为外部二进制(Ubuntu 默认不带 wl-clipboard),需明确打包/检测/缺失降级策略。
3. 8.2 手动验收只覆盖 Windows/macOS,**Linux 无验收用例**,与"跨平台"定位不符。

### P1-10 GCM 分块加密与乱序/重放的交互未设计
E2EE 下分块密文经公网中继,攻击者(或恶意中继)可**重排、重放、截断**密文块。GCM 的 AAD 必须绑定 `(session_id, item_id, chunk_index)`,解密端必须校验块序单调性与 TotalChunks 完整性,文档均未提及;断点续传 `resumed_chunks` 与密文块的对应关系也未定义(见 P0-2、P1-6 交叉)。

### P1-11 服务端滥用防护缺失
作为公网服务,缺少:每设备/每账号的连接数与带宽速率限制、信令 payload 大小上限(items 数组可被灌到 MB 级)、二进制帧 `PayloadLen` 上限校验(必须强制 ≤ 4MB,防恶意声明 4GB 触发内存攻击)、慢速连接(Slowloris)超时。8.1 有"极端溢出长度"测试**意图**,但防护设计本身未写。

### P1-12 帧图与字段表的细节错漏及标识映射缺失
1. 2.2.1 ASCII 图中 SessionID(16 字节)只画了 3 行(每行 4 字节),应为 4 行;图注 "UUIDv4 ASCII / Binary" 含糊——ASCII UUID 需 36 字节,16 字节只能是 binary,应定死格式。
2. **SessionID 两处格式不一致**:信令中是可读字符串 `"sess_20261001_894192"`(DESIGN.md:169),帧内是 16 字节二进制 UUID,两者映射规则未定义。
3. **ItemID 类型不一致**:信令 `item_id` 是字符串 `"item_01"`,帧内是 `uint32` 序号(DESIGN.md:263),字符串→数字的映射约定(数组下标?)未写明。

---

## 四、P2 —— 建议改进

| # | 位置 | 问题与建议 |
| :--- | :--- | :--- |
| P2-1 | DESIGN.md:91、267 | CRC32 描述为"防篡改"不准确——CRC32 仅能检错,防篡改依赖 TLS/E2EE,应修正措辞。 |
| P2-2 | DESIGN.md:261 | `ChunkType`(0x01 文本/0x02 图片/0x03 中间块/0x04 EOF)与信令 `data_type`(TEXT/IMAGE/FILES)语义冗余;"文件最后一块"类型与 `TotalChunks` 字段冗余(可由 chunk_index 推导);TEXT/IMAGE 如何分片(TotalChunks 语义)未定义。建议精简枚举、明确各 data_type 的分片规则。 |
| P2-3 | DESIGN.md:339-343 | `DeviceRegistry` 同时持有 `sync.Map` 与 `sync.RWMutex`,双并发原语并存意图不明(sync.Map 本身并发安全),属设计混乱,应二选一并写明保护范围。 |
| P2-4 | DESIGN.md:383-386 | 热路径 `select` 中使用 `time.After` 每次分配 timer(Go 1.23 前有 GC 压力),应改 `timer.Reset` 或 `context.WithTimeout`。 |
| P2-5 | DESIGN.md:361-366 | 应用层心跳(15s/45s)与 WebSocket 协议层 Ping/Pong(RFC 6455)的关系未说明(应优先用协议层保活);反向代理(Caddy/Nginx)的 idle timeout 配置要求未提,生产环境极易出现 60s 静默断连。 |
| P2-6 | DESIGN.md:409 | 指数退避重连未提 jitter(随机抖动),服务端重启/发布时全量客户端将产生重连风暴。 |
| P2-7 | DESIGN.md:694-710 | LRU/TTL 淘汰与剪贴板活跃期存在竞态:24h TTL 到期物理删除后,用户再次 Ctrl+V 时 Explorer 持有的 CF_HDROP 指向已删文件,粘贴静默失败;X11 下更无法感知目标端是否还在读。虽有"剪贴板读取锁定"(DESIGN.md:704)但锁定解除条件与竞态窗口未定义,建议把"剪贴板注入后 N 小时内不淘汰"固化为规则,并考虑提供"另存为"兜底。 |
| P2-8 | DESIGN.md:7、80、912 | "内存 ≤ 30MB / 20~30MB"指标未定义统计口径:Tauri 主进程之外,Windows WebView2 / macOS WKWebView 子进程常驻 50MB+,应明确"主进程 RSS"口径,否则验收必然扯皮。 |
| P2-9 | DESIGN.md:113-133 | 信令集缺少:TRANSFER_FAILURE(见 P0-1)、CLIPBOARD_INJECTED 回执(配合 P0-5 交互模型)、鉴权失败错误码规范、信令级错误(Rejected 的 reason 枚举)。 |
| P2-10 | DESIGN.md:299、373-387 | "零拷贝流式透传"表述不严谨:`chan []byte` 转发若发送侧复用读缓冲则存在 use-after-send 数据竞争,若每帧新分配则 GC 压力大;应写明缓冲所有权约定(如每帧独占分配 + sync.Pool 复用)。 |
| P2-11 | 全文 | 可观测性整章缺失:trace_id 字段有了(DESIGN.md:102)但服务端无日志/指标/追踪落地方案,传输类产品至少需要每会话吞吐、重传率、在线设备数等核心指标与告警。 |
| P2-12 | DESIGN.md:7-8、79、153、874、877 | 多处选型未收敛与 "Approved" 状态矛盾:gorilla/nhooyr、React/Vue、SQLite/Redb、HTTP/2/WSS、HMAC/JWT。实现前应逐项定稿。另外 nhooyr.io/websocket 已归档并迁移至 github.com/coder/websocket,选型需更新。 |
| P2-13 | DESIGN.md:121-122 | `DEVICE_ONLINE/OFFLINE` 增量广播与 `DEVICE_LIST_SYNC` 全量下发的时序一致性(重连后先全量后增量的窗口)未定义。 |
| P2-14 | DESIGN.md:8、916-922 | 阶段划分(Phase 1/2/3)仅一句话带过,各 Phase 的功能裁剪边界(如 Phase 1 是否含断点续传/E2EE)应明确,便于排期与验收。 |

---

## 五、P3 —— 细节与风格

| # | 位置 | 说明 |
| :--- | :--- | :--- |
| P3-1 | DESIGN.md:302 | STUN 引用 RFC 5389,已被 RFC 8489(STUNbis)取代,建议更新;3478/UDP 在容器部署时的防火墙/端口映射要求应写入部署说明。 |
| P3-2 | DESIGN.md:823 | `config.yaml` 模板含 PSK,密钥明文落盘有泄露风险,建议改为环境变量/密钥管理服务注入,配置文件只留占位。 |
| P3-3 | — | 客户端分发合规未提:Windows 代码签名(SmartScreen/杀软对"剪贴板监听+自启动+网络常驻"程序误报率高)、macOS 公证(notarization)与 MAS 沙盒 entitlements。 |
| P3-4 | DESIGN.md:83 | SQLite 提了但无表结构设计(任务状态、块位图、配对公钥、传输历史至少 4 张表的 schema 应给出)。 |
| P3-5 | DESIGN.md:100 | 协议 `version` 字段有了,但新旧客户端/服务端混用的兼容策略未提。 |
| P3-6 | DESIGN.md:773 | HMAC 签名串 `Timestamp + DeviceID + Nonce` 的拼接分隔符与编码(字符串还是字节)未精确指定,实现易出分歧。 |
| P3-7 | DESIGN.md:127 | `TRANSFER_PROGRESS` 未定义发送端口径(已发送)与接收端口径(已落盘)差异,UI 展示应取哪个。 |
| P3-8 | DESIGN.md:749-763 | 时序图中数据帧与信令混在同一参与者轴上流转,与"数据面分离"架构呼应不足,建议按控制面/数据面分泳道。 |
| P3-9 | — | 大文件多任务并发(多设备同时互发)时 TransferEngine 并发池与全局带宽的分配策略未提。 |
| P3-10 | DESIGN.md:78 | "单实例冷启动约 15MB" 等性能数字未标注测量环境,建议注明基准条件。 |

---

## 六、值得肯定的亮点

1. **控制面/数据面分离**的拓扑与"信令 JSON + 二进制定长帧"的协议分层清晰合理,48 字节帧头字段表排布自洽(各字段偏移合计正确)。
2. **示例数值严谨自洽**:24MB = 18MB + 6MB、18MB → 5 块、100MB → 25 块等换算全部正确,体现作者细节功力。
3. **零磁盘流式中继**的定位(隐私不留痕 + 保护 VPS 磁盘)取舍明确,符合产品形态。
4. **剪贴板平台细节真实可信**:DROPFILES 20 字节内存布局、`fWide=TRUE`、Preferred DropEffect 声明拷贝语义、`org.nspasteboard.ConcealedType` / `Clipboard Viewer Ignore` 隐私标签过滤、text/uri-list 的 `\r\n` 行结尾等,均为该领域的正确实践。
5. 断点续传意识(块位图)、令牌桶限流、LRU/TTL 双策略缓存、工程目录规范、含中文文件名的平台验收用例等工程化要素覆盖较全。

---

## 七、审核结论与修订建议

**结论:不通过(Revise & Re-review)。** 架构方向正确、领域细节扎实,但 P0 的 6 项问题(可靠传输闭环、E2EE 三处硬伤、路径穿越、macOS 代码 UB/泄漏、交互模型矛盾、数据面协议未收敛)任何一项都足以造成实现返工或线上事故。

**修订优先级建议**:
1. 先做**决策收敛**:定稿 P0-5(交互模型)、P0-6(数据面承载)与 P2-12 的全部 "A or B" 选型——这些是其他修订的前置。
2. 重写 2.2/4.3:补 ACK/NACK/重传与错误恢复(P0-1、P1-6 随之闭环),同步修正帧头 Reserved 尺寸(P0-2.2)与标识映射(P1-12)。
3. 补安全设计:路径穿越防护(P0-3)、带认证的 E2EE 握手与 AAD 绑定(P0-2、P1-10)、密钥分发与账号模型(P1-3)、服务端滥用防护(P1-11)。
4. 修订平台章:macOS 代码改 objc2-foundation 类型安全 API + autorelease pool(P0-4)、Windows 失败路径 GlobalFree(P1-7)、X11 生命周期(P1-8)、Linux 方案可行性(P1-9),并补"剪贴板监听"整节(P1-1)。
5. 补服务端运维面:单实例假设声明或扩展方案、会话/管道回收、容量上限、可观测性(P1-4、P1-5、P2-11)。

完成上述修订后建议再次提交评审,重点复核 P0 项的闭环情况。
