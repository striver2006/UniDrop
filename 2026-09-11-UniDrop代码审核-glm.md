# UniDrop 全项目代码审核报告

| 项目 | 内容 |
|---|---|
| 审核对象 | UniDrop(Go 中继服务端 + Tauri 2 客户端)全仓库 |
| 审核日期 | 2026-09-11 |
| 审核基线 | `main` @ `f3d5125`,工作区干净,无未提交变更 |
| 审核人 | GLM(ZCode) |
| 审核范围 | `server/`(Go)、`client/src-tauri/`(Rust)、`client/src/`(React/TS 前端)、Go/Rust 协议一致性、测试与 CI/脚本/构建设施、文档与实现一致性 |
| 审核方法 | 全量人工读码(逐文件,file:line 级证据)+ 只读命令验证(`go vet` / `go test -race` / `gofmt -l` / `cargo check` / `cargo test` / `pnpm build` / grep 死代码交叉验证) |

## 审核结论:**不通过(Revise & Re-review)**

问题统计:**P0 × 6,P1 × 10,P2 × 12,P3 × 10,共 38 项。**

### 一句话结论

Go 服务端是一个结构清晰、测试尚可、**接近可用**的原型(但数据面完全无鉴权,不可公网部署);Rust 客户端处于**脚手架状态**——控制面连通、设备列表可见,但发送链路是死路、数据面整体缺失、大量承诺功能为无调用死代码;"文件传输"这一核心功能**只存在于 Go e2e 测试代码中,产品层面端到端不可用**。本次审核未发现"编码已完成"声明与实际状态相符的证据。

### 命令验证结果(全部只读,未改动任何代码)

| 验证项 | 结果 |
|---|---|
| `go vet ./...`(server) | ✅ 通过 |
| `go test -race ./internal/...`(server) | ✅ 全部通过 ⚠️ 但 P0-6 的竞争路径未被任何测试触达,通过≠无竞争 |
| `gofmt -l`(server) | ❌ 4 个文件未格式化:`binary_header.go`、`envelope.go`、`pipe.go`、`stun_server.go` |
| `cargo check`(src-tauri,强制重编译) | ✅ 通过,0 警告 |
| `cargo test`(src-tauri) | ✅ 4/4 通过——但**仅有** `path_guard` 的 4 个测试 |
| `pnpm build`(client,tsc 类型检查) | ✅ 通过 |
| grep 死代码交叉验证 | ✅ 本报告所有"无调用"结论均已逐条验证 |

---

## 1. 分层健康度矩阵

| 层 | 规模 | 完成度 | 代码质量 | 安全 | 测试 |
|---|---|---|---|---|---|
| Go 服务端 | 2412 行(含测试 786) | 高:DESIGN 4.x 硬化项多数落地 | 中上:结构清晰、注释得当,但有并发缺陷 | **低:数据面零鉴权、跨域全开、默认弱密钥** | 中上:17 用例含 10MB e2e,但仅 happy path |
| Rust 客户端 | 1817 行(含测试 56) | **低(约 30%)**:仅控制面连通 | 中:PathGuard/剪贴板注入质量高;核心链路为死码 | 低:PSK 占位符硬编码、CSP null | 极低:4 用例,核心模块零测试 |
| React 前端 | 378 行 | 低:设备展示 + 设置表单;发送/进度为占位 | 中 | 中 | 零 |
| 工程设施 | — | 中:双 CI + 构建脚本齐备 | 中 | — | CI 强度弱于本地脚本 |

---

## 2. P0 阻断级问题(6 项)

### P0-1 数据面 `/ws/data` 完全无鉴权,可被任意第三方窃取/注入/DoS

- **位置**:`server/internal/controller/data_ws.go:31-39, 53, 62-66`
- **问题**:数据面连接的身份(`session_id`、`role`、`device_id`、`target_device_id`)全部来自 URL query 参数,仅校验 `session_id` 非空与 `role` 合法,**无任何签名/HMAC/会话绑定校验**。`GetOrCreatePipe` 对已存在的管道不验证请求方是否为属主(`relay_manager.go:47-50` 只按 sessionID 命中即返回)。
- **影响**(三者任一在公网部署下即刻成立):
  1. **数据窃取**:攻击者抢先以 `role=receiver` 连接目标 sessionID,即可从 `ForwardChan` 分流真实文件数据(多个 receiver 竞争消费同一 chan);
  2. **数据注入**:以 `role=sender` 连接,向受害者推送伪造/恶意分块;
  3. **DoS**:批量伪造 sessionID 占满 200 管道上限,所有合法传输收到 `ErrServerBusy`。
  sessionID 是 16 字节 UUID,但它在 `TRANSFER_OFFER` 中以明文经服务端路由,且无 TTL/一次性绑定,枚举难度不构成防线。
- **修复建议**:数据面握手复用控制面 HMAC 体系(同一 canonical string 增加 `role + session_id` 字段);或控制面认证成功后由服务端签发短时效一次性 token,`/ws/data` 建连时校验;同时强制管道属主匹配(`FromDevice/ToDevice`)。

### P0-2 客户端发送链路是死路:outgoing 通道无人消费,`cmd_send_files` 最终永久挂起

- **位置**:`client/src-tauri/src/core/connection_actor.rs:27-31, 34-45, 114-142`;`client/src-tauri/src/commands/clipboard_cmd.rs:48`;`client/src-tauri/src/lib.rs:38`
- **问题**:`ConnectionActor::run()` 的 `tokio::select!` 只轮询心跳 tick 与 WS 读帧,**从不取出 outgoing 通道的消息**。该通道的接收端被包成 `incoming_rx` 字段并标记 `#[allow(dead_code)]`(connection_actor.rs:29-30);`lib.rs:38` 返回的 `_incoming_rx` 直接被丢弃,其发送端 `_in_tx` 在 `new()` 结束时即已 drop(connection_actor.rs:36)。
- **影响**:`cmd_send_files` 把 `TRANSFER_OFFER` 推入 128 容量通道后**永远发不出去**;队列塞满后 `outgoing_tx.send().await` 无限阻塞,Tauri 命令挂死。文件发送功能 100% 不可用。
- **修复建议**:在 `run()` 的 select 中增加 outgoing 分支(`outgoing_rx.recv()` → `write.send()`),删除误导性的 `incoming_rx` 字段与死通道对;`send()` 失败要有超时与错误上抛。

### P0-3 客户端数据面整体缺失:宣称的端到端文件传输仅存在于 Go 测试代码中

- **证据**(grep 全量交叉验证):
  - `client/src-tauri/src/` 中**不存在任何** `ws/data` / 数据面连接代码;
  - `core/sliding_window.rs`(125 行,ARQ 核心:Jacobson/Karels RTO、Karn 算法、快速重传)**无任何调用者**;
  - `protocol/binary_header.rs`(Rust 版 64 字节帧编解码)**无任何调用者**;
  - `storage/bitmap_repo.rs`(断点续传位图)**无任何调用者**;
  - `core/transfer_engine.rs:80` `process_incoming_chunk`(接收路径,含 PathGuard 集成)**无任何调用者**。
- **影响**:README/DESIGN 宣称的核心能力"跨设备文件传输 + ARQ 可靠性"在产品代码中不存在闭环。`server/internal/e2e_test.go` 的 10MB 传输测试里,收发双端都是 **Go 测试代码**模拟的,从未有真实 Rust 客户端走过该链路。commit 信息"complete end-to-end Go transfer test"描述的是服务端内部测试,不构成产品功能证据。
- **修复建议**:实现客户端数据面 actor(连接 `/ws/data`、分块读取、`SlidingWindow` 接管发送窗口、NACK/超时重传、ACK 回送、整文件 SHA-256 校验),并用真实 Rust 客户端对 Go 服务端跑一次跨语言 e2e——这本身就是对双份协议定义的一致性验证(见 P2-6)。

### P0-4 PSK 三处硬编码互不匹配 + 认证结果从不检查:客户端对任何现有服务端配置都无法通过认证,且失败表现为无限静默重连

- **位置**:
  - 客户端:`client/src-tauri/src/lib.rs:32` 与 `app_state.rs:36` → `"YOUR_SHARED_SECRET_KEY_HERE"`(还在两处重复硬编码);
  - 服务端默认:`server/internal/config/config.go:21` → `"default-insecure-psk-change-me"`;
  - 开发脚本:`scripts/dev-server.sh:8` → `"dev-insecure-psk-secret"`。
- **问题(第二半)**:`connection_actor.rs:72-110` 发出 AUTH_REQUEST 后**从不检查 AUTH_RESPONSE** 的 `success` 字段(信封被转发到 `lib.rs`,而 `lib.rs:82` 的 match 对 `AUTH_RESPONSE` 落入 `_ => {}`);认证失败时服务端关闭连接,客户端 `read` 返回关闭 → break → 退避重连,循环往复,**用户侧无任何可见错误**。
- **影响**:按仓库内任意现有配置组合,客户端都无法认证成功;"开箱即用"路径完全不可用;错误被系统性吞掉,排障极难。
- **修复建议**:PSK 一律走外部配置(环境变量/配置文件/首启引导),代码中不得出现任何占位密钥;`AUTH_RESPONSE.success=false` 必须上抛 UI 并停止重连(区分"网络断"与"凭证错")。

### P0-5 重连竞态:旧 handler 的延迟注销会误杀新会话

- **位置**:`server/internal/controller/control_ws.go:113-115` + `server/internal/registry/registry.go:23-31, 34-42`
- **问题**:同 DeviceID 重连时,`Register` 关闭并替换旧会话(registry.go:27-29);但旧连接的 handler 退出时执行 `defer h.registry.Unregister(session.DeviceID)`——`Unregister` 按 DeviceID 键删除并 `Close()`,**不校验 map 中是否仍是自己的实例**。时序:旧连接半开(服务端 ReadPump 无读超时,TCP 半开可存活很久)→ 客户端重连并 Register 新会话 → 旧连接最终死亡、handler 退出 → 延迟注销把**新会话**删除并关闭,并向全账号广播虚假 DEVICE_OFFLINE。
- **影响**:网络切换/快速重连场景(恰是本产品的主场景)下,重连后的新连接被上一连接的"遗言"杀死,设备反复掉线。这不是小概率窗口:旧 TCP 未被服务端感知死亡的时间窗可达数分钟(心跳 15s、ReadPump 无 deadline)。
- **修复建议**:`Unregister` 改为 compare-and-delete(传入 session 实例指针,仅当 map 中存储的正是该实例才删除);或给会话加代际 ID。同时补一个"重连替换"并发测试(现有 registry_test.go 未覆盖此路径)。

### P0-6 `DeviceSession.LastPingAt` 数据竞争(无同步的多字段时间值读写)

- **位置**:写方 `server/internal/registry/session.go:70-72`(`TouchPing` 无锁写 `time.Time`);读方 `server/internal/registry/registry.go:101`(`SweepInactive` 持 registry 锁读——**该锁不覆盖写方**)
- **问题**:`time.Time` 是多字结构,ReadPump 中心跳写入与 10s 周期清扫 goroutine 并发读写,构成 Go 内存模型意义上的数据竞争(撕读)。`go test -race` 通过仅因现有测试从未让"心跳"与"清扫"并发交叠。
- **影响**:撕读出的时间值不可预测 → 误判超时误踢设备,或永不超时;属未定义行为,任何 Go 版本升级都可能放大。
- **修复建议**:改存 `atomic.Int64`(UnixNano),或给 `LastPingAt` 加 session 级 mutex;补一个心跳/清扫并发的 `-race` 测试。

---

## 3. P1 重要问题(10 项)

### P1-1 控制消息在通道满时静默丢弃,TRANSFER_OFFER/ANSWER 可能无痕丢失

- **位置**:`server/internal/registry/session.go:57-67`(满即弃,返回 bool);调用方 `control_ws.go:235, 253` 无视返回值
- **影响**:慢消费者(256 容量 SendChan 塞满)场景下,TRANSFER_OFFER/ANSWER/CLIPBOARD_INJECTED 被静默丢弃,传输挂死且无任何错误信号;被 P0-3 掩盖(客户端本就收不到),一旦数据面修好立刻显形。
- **建议**:丢弃时至少 `slog.Warn` + 计数指标;对 TRANSFER_* 类信令考虑阻塞式投递或回执 TRANSFER_FAILURE。

### P1-2 双端点 `InsecureSkipVerify` + 客户端 CSP 为 null + 全链路无 TLS

- **位置**:`control_ws.go:33-35`、`data_ws.go:41-43`(注释自认"Allow cross-origin");`client/src-tauri/tauri.conf.json:29-31`(`"csp": null`);服务端为纯 HTTP `ListenAndServe`(main.go:73-88),客户端默认 `ws://`(lib.rs:29)
- **影响**:浏览器中任意网页可发起 `ws://localhost:8080` 连接(跨站 WS 劫持),与 P0-1 叠加后攻击面从"网络攻击者"扩大到"用户打开一个网页";CSP 关闭违反 Tauri 安全基线;明文 WS 下 PSK-HMAC 无法保护数据机密性(HMAC 只防伪造不防窃听)。
- **建议**:服务端按 DESIGN 8.1 落地 TLS 或文档化强制反代;Origin 校验恢复(至少校验非 browser origin);客户端 CSP 配置为白名单模型。

### P1-3 不安全默认 PSK 静默生效

- **位置**:`server/internal/config/config.go:21`;`main.go:28-35` 对此无任何告警
- **影响**:`UNIDROP_PSK_SECRET` 未设置时,服务端以**公开在仓库里的已知密钥**正常运行——所有知道该字符串的人都能通过认证。这是 P0-4 的服务端镜像。
- **建议**:未显式配置时 fail-fast 拒绝启动(生产),或至少 `slog.Error` + 指标暴露;开发模式需显式 opt-in。

### P1-4 device_id 双重生成不一致,且每次启动随机重生、不持久化

- **位置**:`client/src-tauri/src/lib.rs:31`(连接身份)vs `app_state.rs:28`(UI/发送身份)——**两个不同的 UUID**;`commands/device_cmd.rs:14` 与 `clipboard_cmd.rs:41` 使用后者;`storage/db.rs:58` 的 `paired_devices` 表从未使用
- **影响**:本机自示信息、offer 的 from_device 与服务端注册身份三者不一致(服务端 `control_ws.go:224` 强制改写 FromDevice 掩盖了症状,但 UI 显示的 ID 与对端看到的 ID 永远不同);每次启动身份漂移,信任关系/断点续传/P2 配对全部无从谈起。
- **建议**:首次启动生成后落库(SQLite 已有现成存储),全局单例注入。

### P1-5 设置仅内存态:不持久化、不生效

- **位置**:`client/src-tauri/src/commands/settings_cmd.rs:21-25`(只改内存);`app_state.rs:33-39`(默认值再硬编码一份);`ConnectionConfig` 在启动时固定(lib.rs:28-36),修改 server_url/PSK 后 **actor 不重建、连接不切换**;`rate_limit_mb` 字段无任何消费者
- **影响**:设置界面形同虚设——用户改完 PSK 保存,界面提示成功,实际连接仍用旧值;重启后全部丢失。与 P0-4 叠加导致"用户甚至无法自救修复认证"。
- **建议**:设置落 SQLite;保存后触发 actor 热重连(重启连接协程);删除无消费字段或实现之。

### P1-6 接收写入路径缺陷:无 truncate、跨会话同名互踩、is_last 误判、无整文件校验

- **位置**:`client/src-tauri/src/core/transfer_engine.rs:96-112`;`core/cache_manager.rs:88`(路径不含 sessionID)
- **问题**:
  1. 打开缓存文件仅 `.create(true)` 无 `.truncate()`——重传/续传场景下,同路径旧文件比新文件长时,尾部残留旧数据;
  2. 缓存路径为 `cache_root/<relative_path>`,**不含 session 隔离**——两个并发会话传输同名文件会交叉写同一物理文件,数据互毁;`cache_entries` 以 file_path 为主键,记录互相覆盖(cache_manager.rs:74-84);
  3. `is_last = chunk_index + 1 == total_chunks`(transfer_engine.rs:111)按序号而非完整性判定——乱序到达时末块先到即报告"完成";
  4. 每收一个 chunk 执行一次 `INSERT OR REPLACE`(transfer_engine.rs:108),DB 写放大;
  5. 文件收满后**从不执行整文件 SHA-256 校验**(payload 里有该字段),CRC32 只在分块级。
- **影响**:当前为死代码(P0-3),但这是数据面接线时**必然带入**的缺陷,届时直接产生静默数据损坏——本条必须在接线前修复。
- **建议**:路径加 `session_id/` 前缀;首块以 `truncate` 创建、续传模式显式区分;完成判定改为位图齐全(`BitmapRepo` 本就该干这个);完成后校验 SHA-256 失败即 `TRANSFER_FAILURE`。

### P1-7 非法 magic/版本帧不断连;反向(ACK)路径完全无校验

- **位置**:`server/internal/controller/data_ws.go:108-112`(解码失败仅 `continue`);DESIGN 2.2.2 明确要求"非法 magic 直接切断连接";`data_ws.go:163-174`(receiver→sender 方向:任何 ≥64 字节的二进制帧原样进 `BackwardChan`,不验 magic/类型/长度)
- **影响**:攻击者可无限倾倒垃圾帧占 CPU 与连接;伪造 ACK/NACK 扰乱发送方窗口(与 P0-1 叠加)。
- **建议**:按设计断连;反向路径同样过 `DecodeBinaryHeader` 并限定 `ChunkType ∈ {ACK, NACK, PROBE}`、`PayloadLen == 0`。

### P1-8 滥用防护未实现:无每 IP 限流、无 items/总量上限执行

- **位置**:DESIGN 4.5.1 承诺"15 次握手/分钟/IP + 15 分钟封禁"——代码中不存在;`config.go:12` `MaxItemsPerOffer` 为死配置,`control_ws.go:237-257` 转发 TRANSFER_OFFER 时从不检查 `items` 数量(≤1000)与 `TotalSize` 上限
- **影响**:认证接口可被暴力尝试(HMAC 比较是常量的,但无次数限制);信令面可被超大 offer 打爆对端。
- **建议**:按设计实现 IP 级令牌桶 + 封禁;信令路由处执行 items/size 上限(用上 `MaxItemsPerOffer`)。

### P1-9 大量承诺功能未接线(客户端"有零件无整机")

- **证据**(grep 验证均为零调用):
  - 剪贴板**监听**:仅 macOS 实现(`listener_macos.rs:10`,500ms changeCount 轮询 + 密码管理器 ConcealedType 过滤,质量不错)但**从未启动**;Windows/Linux 为空函数桩(`listener_windows.rs:7`、`listener_linux.rs:7`);
  - 系统通知 `notification.rs:4` `show_transfer_notification` **从未调用**;
  - 缓存清扫 `cache_manager.rs:87` `sweep_expired_and_lru` **从未调度**——且"LRU"名不副实,只实现了 24h TTL 清理,10GB/8GB 双水位仅有常量(cache_manager.rs:10-11),配额驱逐逻辑不存在;
  - 前端**无任何 `listen()` 订阅**:后端 emit 的 `devices-updated` / `transfer-offer-received` 没有消费者,设备列表只能手动点刷新(App.tsx:35-37 仅 mount 时拉一次);
  - 发送按钮为 `alert('准备发送至设备: ...')` 占位(App.tsx:48-50);进度列表 `const [transfers] = useState([])` 无 setter,**永远为空**(App.tsx:12),进度 UI 永不渲染;**没有文件选择器/拖拽入口**,用户无法挑选要发送的文件。
- **影响**:用户可感知的功能面 = "看到设备列表 + 改一个不生效的设置"。
- **建议**:按"发送文件选择 → offer → 数据面传输 → 进度 → 通知 → 点击注入"的完整用户旅程逐环接线;每环接好即补测试。

### P1-10 阻塞调用占用 async 运行时

- **位置**:`client/src-tauri/src/commands/clipboard_cmd.rs:30`(`prepare_offer` 在 async 命令中同步全文件 SHA-256,大文件直接卡死 worker);`clipboard_cmd.rs:12`(`inject_files_to_clipboard` 同步 FFI,Windows 版含 `thread::sleep` 重试,`clipboard_windows.rs:23`);所有 rusqlite 调用都在 tokio Mutex 下同步执行(如 `app_state.rs:15`)
- **影响**:传输大文件时 UI 冻结、心跳延迟(进一步诱发 P0-6 误清扫)。
- **建议**:哈希/文件 IO/剪贴板 FFI 一律 `spawn_blocking`;SQLite 考虑专用线程 + channel,或换 sqlx。

---

## 4. P2 一般问题(12 项)

| # | 问题 | 位置 | 说明与建议 |
|---|---|---|---|
| P2-1 | NonceSalt 装饰性;nonce 先消费后验签 | `control_ws.go:46-49`;`verifier.go:43-45, 63-72` | 挑战盐未纳入 canonical string,无挑战-响应绑定;更糟的是 `CheckAndAdd` 在签名验证**之前**执行——一次坏签名即烧毁该 nonce,攻击者(或网络重放)可借此对合法认证做拒绝服务。顺序应为:验签 → 通过后才记录 nonce;NonceSalt 应进签名内容 |
| P2-2 | ACK 帧占用 4MB 池化缓冲,无全局内存配额 | `data_ws.go:172-174`;`pipe.go:37`;`buffer_pool.go:7-11` | 64 字节的 ACK 从 4MB+64 的池取缓冲;`BackwardChan` 容量 16 → 单管道反向最深 ~68MB 常驻,200 管道理论最坏 GB 级。DESIGN 4.3 的 1GB 全局配额未实现。建议 ACK 专用小缓冲池 + 全局配额计数器 |
| P2-3 | 缓存路径无会话隔离(P1-6 第 2 点的根因) | `cache_manager.rs:88`、`transfer_engine.rs:88` | `cache_root/<relative_path>`,同名即冲突;修复方案见 P1-6 |
| P2-4 | 测试覆盖结构性缺口 | `server/internal/` | e2e 仅 happy path:无丢块/乱序/重传、无认证失败路径、无拥塞、无畸形帧;`controller` 包无单测(P0-5/P0-6 均因此漏网);无重连替换、心跳并发、STUN、数据面鉴权测试 |
| P2-5 | CI 强度弱于本地脚本,全仓库零 lint 配置 | `client-ci.yml:55`(仅 `cargo check`,无 `cargo test`/clippy);`server-ci.yml`(无 vet/gofmt);无 `.golangci.yml`、`rustfmt.toml`、eslint/prettier | 本地 `build-all.sh:26` 反而跑 `cargo test`;建议 CI 对齐并补 lint 门禁(本次 gofmt 就抓出 4 个不合格文件) |
| P2-6 | 协议双份手工维护,无共享源、无一致性测试 | `binary_header.go:12-43` ↔ `binary_header.rs:4-33`;`envelope.go:8-30` ↔ `envelope.rs`;canonical string `verifier.go:44` ↔ `connection_actor.rs:49`;`version: 1` 在 Rust 侧 6+ 处硬编码 | magic/版本/帧型/flag/4MB 上限全部手工镜像;任何一侧漂移即静默断裂(且当前无跨语言 e2e 兜底)。建议生成代码或至少加跨语言协议一致性测试 |
| P2-7 | 控制面信令与数据面管道零耦合 | `control_ws.go:237-257` 仅盲转发;`data_ws.go:53` 凭 query 自由建管 | 无 offer→answer→授权建管的服务端状态机,sessionID 无归属校验——P0-1 的结构性根因。建议服务端记录"已 answer 的 sessionID → (from, to)",数据面据此校验 |
| P2-8 | 认证失败/握手异常以 `StatusInternalError` 关闭 | `control_ws.go:40, 99` | 应区分策略类错误(如 1008 Policy Violation),便于客户端决策与观测 |
| P2-9 | STUN 半成品:IPv6 返回全零地址、无停机 | `stun_server.go:72-84`(family 硬编码 0x01,`To4()==nil` 时 `resp[28:32]` 留零);`stun_server.go:31-56` 无 shutdown | P3 阶段功能提前实现却不完整;建议 IPv6 直接不响应(或正确编码 family=0x02),并挂到优雅停机 |
| P2-10 | 前端可用性/诚实性缺陷 | `tauri.conf.json:19-22`(decorations:false 且 App.tsx 无 `data-tauri-drag-region` → **窗口无法拖动**;visible:false + skipTaskbar → 首启"无窗口"易被当崩溃);`SettingsModal.tsx:13`(打开时一次性拷贝 settings,外部更新不同步);`App.tsx:112-114`(footer 宣称"PSK 接入安全就绪/支持 Ctrl+V、Cmd+V 原生落地",与实际严重不符) | 补 drag region;首启 show;文案与实际能力对齐 |
| P2-11 | go.mod 依赖标注失真 | `go.mod:5-8` | 两个直接依赖均标 `// indirect`,`go mod tidy` 未运行 |
| P2-12 | 管道生命周期:正常完成不回收 | `relay_manager.go:70-79` `RemovePipe` 无调用者 | 传输完成/取消后管道只能等 60s idle 清扫;建议 TRANSFER_COMPLETE/FAILURE 时主动回收,释放配额 |

---

## 5. P3 建议级问题(10 项)

| # | 问题 | 位置 |
|---|---|---|
| P3-1 | gofmt 未执行,4 文件不合格:binary_header.go、envelope.go、pipe.go、stun_server.go(注释对齐/空格问题),CI 无格式门禁 | `gofmt -l` 实测 |
| P3-2 | Go 死代码:`MaxItemsPerOffer`/`HeartbeatInterval` 死配置(config.go:12-13,后者 HeartbeatTimeout 有用)、`RemovePipe` 死方法、`TotalOffers`/`RetransmitsCount` 死指标(health.go:19-20) | 同左 |
| P3-3 | Rust 死依赖:`governor`/`nonzero_ext`(Cargo.toml:34-35,全仓库零引用)、`x11rb`(Linux 监听是空桩)、`tokio-tungstenite` 的 `rustls-tls-webpki-roots` feature(服务端是明文 ws);死通道对 `_in_tx/in_rx`(connection_actor.rs:36)、`MAX_CACHE_SIZE_BYTES`/`SAFE_LOW_WATERMARK_BYTES` 常量 | 同左 |
| P3-4 | 仓库卫生:`.gitignore` 忽略 `Cargo.lock`(对二进制 crate 是反模式,应提交以保证可重现构建);`gen/schemas/` 4 个生成文件入库(desktop-schema.json 2810 行等) | `.gitignore`、`git ls-files` 实测 |
| P3-5 | 配置文档误导:`configs/config.example.yaml` 暗示支持 YAML 配置,实际仅环境变量;`pipe_idle_timeout_seconds` 等 knob 硬编码不可配;README.md:89 引用不存在的 `docs/` 目录 | `config.go` 全文仅 env |
| P3-6 | 命名与重复:`chrono_now_ms` 名不副实(无 chrono,connection_actor.rs:157);`fastrand_u64` 时间取模弱随机+模偏差(仅抖动,可容忍);`whoami_hostname` 两处重复实现(lib.rs:141 / app_state.rs:56)且 `HOSTNAME` 在 macOS 通常未设,恒回 "localhost" | 同左 |
| P3-7 | macOS 监听器首轮 `changeCount != -1` 必触发一次当前剪贴板上报(listener_macos.rs:12-21)——接线后会把用户启动应用时剪贴板里的内容直接发出去,应首帧只记基线 | 同左 |
| P3-8 | `prepare_offer` 对目录无分支:`is_dir` 只是标记,目录仍走 `File::open`+SHA-256(transfer_engine.rs:25-43),Linux/macOS 上目录打开即报错;多文件 `relative_path` 只取文件名,子目录结构丢失(与 DESIGN 的 relative_path 语义不符) | 同左 |
| P3-9 | metrics 缺口:DESIGN 4.6 承诺的 account/os 维度在线数、重传计数、E2EE 计数均无;`/metrics` 无认证对外暴露规模信息;日志无 trace_id 贯穿(envelope 有 trace_id 字段但 slog 全程不打印) | `health.go:43-57` |
| P3-10 | 前端错误处理仅 `console.error`(App.tsx:28-29, 43-45),用户无感知;无任何前端测试与 test 脚本(package.json scripts) | 同左 |

---

## 6. 设计-实现一致性核对

### 6.1 对照前次设计审核(2026-09-11-UniDrop设计审核-glm.md)的 6 项 P0

| 设计评审 P0 | 代码落地状态 |
|---|---|
| P0-1 无 ACK/NACK/重传闭环 | **部分落地**:服务端帧类型(DATA/ACK/NACK/PROBE)+ 管道 + e2e 已证;但客户端 `SlidingWindow` 未接线,ARQ 闭环在产品中不存在;且 `SlidingWindow::check_timeouts` 未实现 DESIGN 3.x 的"单块最多 5 次重试后放弃"(retries 只增不判) |
| P0-2 E2EE 三缺陷(未认证 DH/nonce 空间/明文元数据) | **未实现**(属 Phase 3 计划,可接受;header 已预留 12 字节 Nonce 与 IS_ENCRYPTED flag)。注意 README 若宣称端到端加密需先删 |
| P0-3 relative_path 任意文件写 | **组件已修复,集成未生效**:`PathGuard` 实现质量高(绝对路径/../Windows 保留名/点空格修剪,4 测试全过),但唯一调用方 `process_incoming_chunk` 本身是死代码 |
| P0-4 macOS objc UB | **已修复**:`objc2-foundation` + `autoreleasepool` + `NSString::from_str`(内部保证 NUL 终止),设计评审所指缺陷全部消除 |
| P0-5 交互模型自相矛盾 | **设计已定稿**(v1.1.0:通知→点击→注入,可选自动+备份);客户端 `auto_inject` 设置项存在,但通知/注入链路未实现 |
| P0-6 数据面承载未定 | **设计已定**(双 WS);服务端已实现;客户端数据面缺失(本报告 P0-3) |

### 6.2 服务端对 DESIGN 4.x 硬化承诺的执行情况

| 承诺 | 状态 | 证据 |
|---|---|---|
| 200 管道上限 | ✅ | relay_manager.go:52 |
| 60s idle 管道清扫 + 10s cleaner | ✅ | relay_manager.go:14;main.go:48-64 |
| 45s 心跳超时清扫 | ✅(但有 P0-6 竞争) | main.go:54 |
| 512KB 信令读上限 / 5s 握手超时 | ✅ | control_ws.go:43, 63 |
| HMAC canonical string / 60s skew / nonce 防重放 | ✅(顺序缺陷见 P2-1) | verifier.go |
| 同账号路由 + FromDevice 防伪造 | ✅ | control_ws.go:224, 251 |
| 1GB 全局缓冲配额 | ❌ | 无任何配额逻辑(P2-2) |
| 15 握手/min/IP + 15min 封禁 | ❌ | 不存在(P1-8) |
| 非法 magic 立即断连 | ❌ | data_ws.go:110-112 continue(P1-7) |
| items ≤ 1000 / offer 大小上限 | ❌ | 死配置(P1-8) |
| Prometheus 按账号维度 + 重传计数 | ❌ 部分 | 仅 3 个全局指标(P3-9) |

---

## 7. 测试与工程化缺口汇总

1. **测试金字塔倒挂**:质量最好的测试(e2e)测的是"服务端内部闭环",而真正要交付的产品路径(Rust 客户端 → 服务端)零覆盖;Rust 侧 1817 行代码只有 4 个测试(PathGuard);前端零测试。
2. **controller 包零单测**:P0-5(重连误杀)与 P0-6(数据竞争)都藏在这个包,恰是无测试的包。
3. **CI 门禁不足**:Rust 仅 `cargo check`(本地脚本都跑 `cargo test`,CI 反而不跑);无 gofmt/vet/clippy/eslint 任何静态检查——本次审核用最基础的 `gofmt -l` 就抓出 4 个不合格文件,说明门禁缺位有实际后果。
4. **构建可重现性**:`Cargo.lock` 被 gitignore(二进制 crate 应提交);`gen/schemas` 生成物反而入库。
5. **文档与实现漂移**:config.example.yaml 对应的 YAML 加载不存在;README 引用不存在的 docs/;README 能力清单(文件传输/剪贴板同步/PathGuard/隐私过滤)与产品现状差距显著,建议在 README 顶部明确标注"项目当前状态:开发中,端到端传输尚未打通"。

---

## 8. 修复路线建议(按优先级)

**第一批(解除 P0,预计后其余工作才有意义)**
1. P0-1 数据面鉴权(服务端 session 授权状态机 + token/HMAC 校验)——与 P2-7 一并做;
2. P0-2 actor 消费 outgoing 通道;
3. P0-3 客户端数据面 + SlidingWindow 接线,修复 P1-6 全部缺陷后接线;补跨语言 e2e;
4. P0-4 PSK/配置外部化 + AUTH_RESPONSE 处理(连带 P1-5 设置持久化与热应用);
5. P0-5 Unregister compare-and-delete + 并发测试;
6. P0-6 LastPingAt 原子化 + `-race` 测试。

**第二批(P1)**
7. P1-1 丢弃可观测;P1-3 默认 PSK fail-fast;P1-7 magic 断连;P1-8 限流与上限执行;
8. P1-4 device_id 持久化单例;P1-10 spawn_blocking;
9. P1-9 按用户旅程接线:文件选择 → 发送 → 进度(listen)→ 通知 → 注入;缓存清扫定时任务;Win/Linux 监听器。

**第三批(P2/P3)**
10. ACK 小缓冲池 + 全局配额;协议共享/一致性测试;CI 补 test/clippy/gofmt/eslint + lint 配置;死代码/死依赖清理;Cargo.lock 入库、gen/schemas 出库;gofmt;文档对齐。

---

## 附录:本次审核执行的验证命令与结果

```
gofmt -l .                     → 4 文件不合格(binary_header/envelope/pipe/stun_server)
go vet ./...                   → 通过
go test -race ./internal/...   → 全部通过(controller 包无测试文件;P0-6 竞争路径未被触达)
cargo check(强制重编译)        → 通过,0 警告
cargo test                     → 4/4(仅 core::path_guard)
pnpm build(tsc + vite)         → 通过
grep 死代码交叉验证             → sweep_expired_and_lru / start_clipboard_listener /
                                 show_transfer_notification / process_incoming_chunk /
                                 SlidingWindow / BitmapRepo / governor / RemovePipe /
                                 MaxItemsPerOffer / listen( 前端 / ws/data 客户端
                                 ——全部确认零调用
```

> 本报告仅为审核结论与修复建议,未对任何源代码做修改。
