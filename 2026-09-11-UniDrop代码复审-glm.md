# UniDrop 全项目代码复审报告(第二轮)

| 项目 | 内容 |
|---|---|
| 审核对象 | UniDrop 全仓库(相对首轮审核基线 `f3d5125` 的全部变更) |
| 审核日期 | 2026-09-11 |
| 变更范围 | 36 个文件修改 + 3 个新文件,+1801 / -338 行(**变更尚未提交**,工作区含未跟踪的 `Cargo.lock`、`SendModal.tsx`、2 个新测试文件) |
| 审核人 | GLM(ZCode) |
| 审核方法 | 逐文件比对 diff 并通读重写模块 + 全量只读命令验证(`gofmt -l` / `go vet` / `go test -race ./...` / `cargo check` 强制重编译 / `cargo test` / `pnpm build` / grep 交叉验证) |
| 前置文档 | `2026-09-11-UniDrop代码审核-glm.md`(首轮,38 项:P0×6 / P1×10 / P2×12 / P3×10) |

## 复审结论:**不通过(差距已大幅收窄,整改项收敛为 4 处代码 + 1 处文档)**

首轮 38 项问题的处置结果:**完全修复 18 项,部分修复 17 项,未修复 3 项**;本轮新引入问题 **10 项(P1×3 / P2×3 / P3×4)**,其中 3 项 P1 与 1 项 P0 残留构成"通过条件"(见第 3 节)。

**总体评价**:这是一轮高质量的整改——6 项 P0 中 5 项修复到位且多数补了针对性测试(CAS 注销、LastPing 并发竞争、无效 magic 断连均有新测试);客户端数据面从零实装(滑动窗口发送、CRC/NACK/位图/SHA-256 接收闭环),产品首次具备真实的端到端传输能力;工程化(CI 门禁、设置持久化、设备身份持久化)明显进步。未通过的唯一原因:数据面鉴权留有一个**可被零凭据触发的回退旁路**,加上三个新 P1(发送方不处理 NACK、文件夹发送必现错配、auto_inject 被硬编码为 true)。这四项都是小改动,修复后预期可直接通过。

### 命令验证结果(全部只读,未改动任何代码)

| 验证项 | 结果 | 对比首轮 |
|---|---|---|
| `gofmt -l`(server) | ✅ 0 个不合格文件 | 首轮 4 个 → 清零,且 CI 已加门禁 |
| `go vet ./...` | ✅ 通过 | 持平 |
| `go test -race ./...` | ✅ 全部通过,`controller` 包从无测试变为有测试 | 测试面扩大 |
| `cargo check`(强制重编译) | ✅ 0 警告 | 持平 |
| `cargo test` | ✅ 7/7(4 PathGuard + 2 SlidingWindow + 1 BitmapRepo) | 首轮 4 → 7 |
| `pnpm build`(tsc + vite) | ✅ 通过 | 持平 |
| grep 交叉验证 | `on_nack` 仍零调用(新问题 N1);`MaxItemsPerOffer`/`HeartbeatInterval`/`TotalOffers`/`RetransmitsCount` 仍为死配置/死指标;`x11rb` 仍零引用 | — |

---

## 1. 首轮 38 项问题逐项处置表

图例:✅ 完全修复 | ◐ 部分修复(残留见第 4 节) | ✗ 未修复

### P0(6 项)

| # | 问题 | 状态 | 核验证据 |
|---|---|---|---|
| P0-1 | 数据面 `/ws/data` 无鉴权 | ◐ | Token 体系完整落地:`TRANSFER_ANSWER` 触发 `AuthorizeSession`(crypto/rand 24 字节 token、5 分钟 TTL、记录收发双方),`CheckAuthorization` 常量时间比较 + 角色-设备映射校验,`ValidateAndGetOrCreatePipe` 校验管道属主(relay_manager.go:66-170);e2e 已改走 token。**但 data_ws.go:43, 66-69 残留空 token 回退旁路 → 必改项 R1(新 N-0)** |
| P0-2 | outgoing 通道无人消费 | ✅ | `ConnectionActor::run` 的 select 新增 `outgoing_rx.recv()` 分支(connection_actor.rs:160-172);死通道对 `_in_tx/in_rx` 已删除;`cmd_send_files` 发送链路打通 |
| P0-3 | 客户端数据面缺失 | ✅ | `start_sender_task`(滑动窗口、超时重传、ACK 统计、进度事件)与 `start_receiver_task`(CRC32 校验、NACK、位图完成判定、整文件 SHA-256、会话隔离目录、TRANSER_FAILURE 上报)实装(transfer_engine.rs:122-524);grep 证实 `ws/data` 连接与帧编解码全部接线。新引入缺陷见 N1/N2 |
| P0-4 | PSK 三处不匹配 + 认证结果不检查 | ✅ | 客户端默认 PSK 改为 `dev-insecure-psk-secret`,与 `scripts/dev-server.sh` 对齐(lib.rs:38,47);AUTH_RESPONSE 逐字段检查,失败时 `auth-failed` 事件 + UI 红色横幅 + footer 状态翻转(connection_actor.rs:114-139, lib.rs:202-208, App.tsx);盐化签名两端对齐。残留:认证失败仍无限重连(N9) |
| P0-5 | 重连误杀新会话 | ✅ | `UnregisterSession` compare-and-delete(registry.go:44-59);deferred 注销仅在匹配自身实例时广播下线(control_ws.go:123-141);新增 `TestDeviceRegistryUnregisterSessionCAS` |
| P0-6 | LastPingAt 数据竞争 | ✅ | `lastPingNano atomic.Int64`(session.go),SweepInactive 改用 `GetLastPing()`;新增 `TestSessionLastPingDataRace`(并发 TouchPing/GetLastPing 过 -race 检测) |

### P1(10 项)

| # | 问题 | 状态 | 核验证据 |
|---|---|---|---|
| P1-1 | 控制消息静默丢弃 | ✅ | `routeToPeer` 检查 `Send` 返回值并 Warn(control_ws.go:286-297);`session.Send` 满时自记 Warn(session.go:67-70)。达到原建议最低线(可观测) |
| P1-2 | InsecureSkipVerify / CSP / TLS | ◐ | CSP 从 null 改为完整白名单策略(tauri.conf.json:30)✅;但 `InsecureSkipVerify: true` 仍在(control_ws.go:51-53、data_ws.go:51-53),服务端仍纯 HTTP 无 TLS |
| P1-3 | 默认 PSK 静默生效 | ◐ | main.go:30-32 增加显著告警日志;未做 fail-fast(按原建议"至少告警"达标,生产建议仍为拒绝启动) |
| P1-4 | device_id 双生成不持久化 | ✅ | `local_config` 表 + `get_or_create_device_id`(db.rs:84-97);单一 UUID 注入连接配置与 AppState(lib.rs:31, 57, 65-70) |
| P1-5 | 设置不持久化不生效 | ◐ | 设置存取 SQLite(`save_persisted_settings`/`get_persisted_settings`)✅;但 `ConnectionConfig` 与 `current_server_url` 启动时固化,保存设置后**不重建连接**,UI 却提示"正在尝试连接..."(名不符实);需重启生效 |
| P1-6 | 接收写入缺陷 | ✅ | 会话隔离目录 `cache_root/<session_id>/`(transfer_engine.rs:350)✅;完成判定改为按 item 位图齐全(453-455)✅;完成后整文件 SHA-256 校验,失败发 TRANSFER_FAILURE(458-520)✅;DB 注册移至校验后(473)✅;首块 truncate(418-419)✅(乱序边缘场景在会话级新目录下无实害) |
| P1-7 | 非法帧不断连 | ✅ | 正向:非二进制/短帧、坏 magic/版本、非 DATA 类型、长度不符 → 全部断连(data_ws.go:120-145);反向:仅接受 ACK/NACK/PROBE 且强制零 payload(196-215);新增 `TestDataWSInvalidMagicFrameDisconnected` |
| P1-8 | 限流与上限 | ◐ | TRANSFER_OFFER 强制 items ≤ 1000(control_ws.go:254-262)✅(但硬编码,`MaxItemsPerOffer` 配置仍死);每 IP 握手限流/封禁仍未实现(grep 无任何实现) |
| P1-9 | 功能未接线 | ◐ | 缓存清扫每小时调度 + 真实 LRU 配额(10GB→8GB 双水位,cache_manager.rs:114-143)✅;macOS 剪贴板监听已启动且修了首轮基线问题(lib.rs:229-242, listener_macos.rs:20-24)✅;通知已调用(transfer_engine.rs:496)✅;前端四类事件全部 listen + 进度状态真实 + SendModal 文件入口(路径输入 + 拖拽)✅。残留:Win/Linux 监听仍空桩;`clipboard-updated` 事件无前端消费者;**auto_inject 被硬编码为 true → 新 N3** |
| P1-10 | 阻塞调用占用 async | ◐ | `cmd_send_files` 哈希、`cmd_inject_files` FFI、接收侧 SHA 校验均 `spawn_blocking` ✅;残留:发送任务 `read_file_chunk` 仍在 async 上下文同步读 4MB 分块(transfer_engine.rs:208, 277) |

### P2(12 项)

| # | 问题 | 状态 | 核验证据 |
|---|---|---|---|
| P2-1 | NonceSalt 装饰性 + nonce 先消费 | ✅ | 盐进入 canonical string(verifier.go:47-52 ↔ connection_actor.rs:43-52,两端一致);**先验签、后记录 nonce**(verifier.go:82-96)。残留:无盐签名回退仍被接受(verifier.go:87-91)→ N6 |
| P2-2 | ACK 占 4MB 缓冲 | ✅ | `AckBufferPool`(64B)专用池,反向通道收发两侧一致(buffer_pool.go:45-77, data_ws.go:91,168,215) |
| P2-3 | 缓存路径无会话隔离 | ✅ | `cache_root/<session_id>/<relative_path>`(transfer_engine.rs:350,405) |
| P2-4 | 测试缺口 | ◐ | 新增 6 个测试(controller×2、race×1、CAS×1、sliding_window×2、bitmap_repo×1)。仍缺:乱序/丢块重传 e2e、数据面旁路回归测试、真实 Rust 客户端对 Go 服务端的跨语言 e2e |
| P2-5 | CI 弱 + 零 lint | ◐ | server CI 加 gofmt 门禁 + go vet + 扩至 `./...`;client CI 加 `cargo test`。仍缺:clippy、eslint/prettier、rustfmt 检查与对应配置文件 |
| P2-6 | 协议双份手工维护 | ✗ | 结构未变(token 字段两侧已对齐,但 magic/帧型/常量仍双份手写,无一致性测试) |
| P2-7 | 控制面/数据面零耦合 | ✅ | ANSWER→`AuthorizeSession` 服务端状态机落地(即 P0-1 主体) |
| P2-8 | 认证失败状态码 | ✅ | 全部改用 `StatusPolicyViolation`(control_ws.go:72,80,88,107) |
| P2-9 | STUN IPv6 全零 + 无停机 | ◐ | 非 IPv4 直接跳过不再回全零(stun_server.go:69-74)✅;`StartSTUNServerWithCloser` 已提供但 **main.go:45 未接线**,README 却宣称"优雅平滑停机" |
| P2-10 | 前端可用性/诚实性 | ◐ | `data-tauri-drag-region` 拖动 ✅、toast 通知 ✅、鉴权横幅 ✅、footer 随鉴权状态翻转 ✅;残留:首启 `visible:false`+`skipTaskbar` 无窗口体验未改;SettingsModal 打开时一次性拷贝设置未改 |
| P2-11 | go.mod indirect 失真 | ✅ | 两个直接依赖标注已修正(go.mod:5-8) |
| P2-12 | 管道完成不回收 | ✅ | COMPLETE/FAILURE/CANCEL 触发 `RemovePipe` + 授权会话清理(control_ws.go:282-292, relay_manager.go:213-220) |

### P3(10 项)

| # | 问题 | 状态 | 核验证据 |
|---|---|---|---|
| P3-1 | gofmt 未执行 | ✅ | 4 文件全部格式化,CI 增加格式门禁 |
| P3-2 | Go 死代码 | ◐ | `RemovePipe` 已有调用 ✅;`MaxItemsPerOffer`/`HeartbeatInterval` 死配置、`TotalOffers`/`RetransmitsCount` 死指标仍在(handler 里硬编码 1000) |
| P3-3 | Rust 死依赖 | ◐ | `governor`/`nonzero_ext` 已删除 ✅;`x11rb` 仍声明但零引用、`tokio-tungstenite` 的 rustls feature 仍无用 |
| P3-4 | Cargo.lock / gen/schemas | ◐ | `.gitignore` 不再忽略 Cargo.lock ✅(注意:当前变更未提交,`Cargo.lock` 尚为未跟踪状态,**提交时务必 `git add`**);`gen/schemas` 4 个生成文件仍入库 |
| P3-5 | 配置文档误导 | ✗ | `config.example.yaml` 仍暗示 YAML 加载;README:93 仍引用不存在的 `docs/`;**README:112 新引入错误:环境变量写成 `UNIDROP_SERVER_PSK`,实际是 `UNIDROP_PSK_SECRET`** |
| P3-6 | 命名与重复 | ◐ | `chrono_now_ms`→`current_time_ms` ✅、`whoami_hostname` 去重统一 ✅;`fastrand_u64` 时间取模弱随机仍在(仅抖动,可容忍) |
| P3-7 | macOS 监听首轮误报 | ✅ | 首轮只记基线(listener_macos.rs:20-24) |
| P3-8 | prepare_offer 目录处理 | ◐ | 目录/不可读路径安全跳过 ✅——**但引入新 bug N2(索引错配)** |
| P3-9 | metrics 缺口 | ✗ | health.go 未动;README 反而新增了更多宣称 |
| P3-10 | 前端错误处理/测试 | ◐ | toast 通知 ✅;前端测试仍为零、无 test 脚本 |

---

## 2. 本轮新发现问题

### N1【P1】发送方完全不处理 NACK,快速重传失效

- **位置**:`client/src-tauri/src/core/transfer_engine.rs:242-268`(发送循环只认 `ChunkType::Ack`);接收方在 CRC 失败时确实会发 NACK(transfer_engine.rs:388-395);grep 证实 `on_nack` 在 `sliding_window.rs` 之外**零调用**。
- **影响**:NACK 到达后被静默丢弃,`SlidingWindow::on_nack` 的快速重传路径是死代码;丢包恢复完全依赖 200ms 轮询的 `check_timeouts`,高丢包/高延迟环境下吞吐与 DESIGN 3.x 承诺的"快速重传"差距显著。
- **建议**:在发送 select 的 ACK 分支中并列处理 `ChunkType::Nack` → `window.on_nack(idx)` → 立即重发对应分块。

### N2【P1】offer.items 与文件路径按索引错配——拖入文件夹时发送必坏

- **位置**:`prepare_offer` 会跳过目录与不可读路径(transfer_engine.rs:51-63),因此 `offer.items` 只含有效文件;但 `cmd_send_files` 把**原始** `path_bufs`(含被跳过项)存入 `pending_outbound`(clipboard_cmd.rs:49-53);发送任务用 `file_paths.get(item_idx)` 按索引重新配对(transfer_engine.rs:162-163)。
- **影响**:SendModal 明确鼓励"将文件或文件夹直接拖动至窗口"——只要列表中混入一个文件夹或不可读项,后续所有 item 都会配到错误的路径:发错文件内容(对端 SHA-256 必然校验失败)或在目录上 `File::open` 失败导致传输中断。这是主流程上的确定性 bug。
- **建议**:`prepare_offer` 返回 `(offer, Vec<PathBuf>)`(与 items 一一对应的有效路径),`pending_outbound` 存这份对齐后的列表。

### N3【P1】auto_inject 被硬编码为 true:设置开关失效 + 违反设计的默认交互 + 隐私风险

- **位置**:`client/src-tauri/src/lib.rs:192` 调用 `start_receiver_task(..., true, ...)`——`auto_inject` 参数硬编码 `true`,从未读取 `settings.auto_inject`;SettingsModal 中的"静默自动装载剪贴板"开关形同虚设。
- **影响**:任何同账号设备发来的文件,接收方**无需任何用户确认**即被写入系统剪贴板(覆盖用户当前剪贴板内容,且无 DESIGN v1.1.0 要求的剪贴板备份)。这与设计定稿的"通知 → 用户点击 → 注入"默认模型直接冲突,也是对用户剪贴板的无感知改写。
- **建议**:传参读取当前设置;自动模式按设计先备份原剪贴板;手动模式由通知点击触发 `cmd_inject_files`。

### N4【P2】接收/发送异常中断无终态事件,进度条永久悬死

- **位置**:两个任务的对端消失路径:接收循环 `read.next()` 返回 None 后直接退出,不 emit 任何 FAILED(transfer_engine.rs:368-372);发送侧同理仅在正常完成时发 COMPLETED。
- **影响**:传输中断后 UI 里该条目永远停留在 TRANSFERRING(App.tsx 只处理 COMPLETED/FAILED),用户只能手动移除;违反状态机完备性。
- **建议**:连接断开/循环退出时统一 emit `status: "FAILED"` 终态。

### N5【P2】重传无次数上限、无总超时

- **位置**:`SlidingWindow::check_timeouts` 的 `retries` 只增不判(sliding_window.rs:86-102),DESIGN 3.x 承诺"单块最多 5 次重试后 `TRANSFER_FAILURE` 放弃"未实现;整个传输任务也无总超时。
- **影响**:对端中途死亡且服务器管道未及时清扫时,发送任务以最长 15s RTO 无限重传,任务与内存泄漏(服务端 60s idle 清扫最终会兜底关闭连接,但依赖该间接机制)。
- **建议**:重试 ≥5 次即失败退出并上报;任务级加总超时(如 10 分钟)。

### N6【P2】无盐签名回退保留,削弱挑战-响应绑定

- **位置**:`server/internal/auth/verifier.go:87-91`——盐化验签失败后仍接受旧式(无盐)签名。
- **影响**:任何捕获到的旧式 AUTH_REQUEST(如嗅探明文 ws)在其 60 秒时间窗内仍可跨连接重放,盐绑定对此类流量形同虚设。当前不存在任何需要兼容的旧客户端,回退纯负资产。
- **建议**:删除回退分支;e2e 与客户端均已使用盐化签名,无兼容负担。

### N7【P3】README 文档错误(3 处)

- README:112 环境变量名写错:`UNIDROP_SERVER_PSK` → 实际为 `UNIDROP_PSK_SECRET`(照抄会导致用户设置无效、静默落入默认密钥);
- README 架构图宣称 STUN"优雅平滑停机",但 `main.go:45` 仍调用不返回 closer 的 `StartSTUNServer`;
- README:93 仍引用不存在的 `docs/` 目录。

### N8【P3】CLIPBOARD_INJECTED 回执仍未实现

DESIGN 定义的"接收方注入完成后向发送方回执"闭环缺失(接收侧仅发 TRANSFER 相关状态,注入后不回执;协议常量已定义两侧均未使用)。

### N9【P3】认证失败仍无限快速重连

connection_actor.rs:135-139:凭证错误与网络错误不区分,固定 3 秒无限重试(且跳过指数退避)。凭证错误应当停机并要求用户修正设置(UI 已有 auth-failed 横幅,后端却仍在打服务器)。

### N10【P3】TRANSFER_OFFER 无条件自动接受

lib.rs:125-151:接收方对一切 offer 自动 answer(accepted=true),无用户确认或白名单。单独看与"自动下载、手动注入"模型可辩护,但与 N3 叠加后构成"全自动接收+全自动注入",放大隐私影响。

---

## 3. 复审通过条件(必改清单)

| # | 对应问题 | 改动量 | 要求 |
|---|---|---|---|
| **R1** | P0-1 残留:data_ws.go:42-69 空 token 回退旁路 | 小(~10 行) | 删除 `token != "" || HasAuthSessions()` 条件与 `GetOrCreatePipe` 回退分支,`CheckAuthorization`/`ValidateAndGetOrCreatePipe` 无条件执行。e2e 与 controller 测试均已使用 token,无测试依赖该回退。同时补一条"空 token 必须被 403"的回归测试(当前 `TestDataWSUnauthorizedAccessRejected` 只测了坏 token,恰好绕过了旁路路径) |
| **R2** | N2:items/paths 索引错配 | 小 | `prepare_offer` 返回与 items 对齐的有效路径列表 |
| **R3** | N1:发送方处理 NACK | 小 | ACK 分支并列处理 NACK → `on_nack` → 立即重传 |
| **R4** | N3:auto_inject 硬编码 | 小 | 读取设置;默认手动注入 |
| **R5** | N7:README 环境变量名 | 一行 | `UNIDROP_SERVER_PSK` → `UNIDROP_PSK_SECRET` |

以上完成后本轮即可判定通过;第 4 节残留项可进入下一轮迭代。

## 4. 残留问题跟踪(部分修复项的未竟部分,按优先级)

1. **P1-2 残留**:`InsecureSkipVerify: true` 双端点仍在、全链路无 TLS(明文 ws 下 token/信令可被嗅探,N6 的重放窗口即来源于此);
2. **P1-5 残留**:保存设置后连接不重建(需重启),UI 文案需同步修正;
3. **P1-8 残留**:每 IP 握手限流/封禁未实现,认证接口仍可无限尝试;
4. **P1-9 残留**:Windows/Linux 剪贴板监听空桩;`clipboard-updated` 无消费者;
5. **P1-10 残留**:发送任务同步读 4MB 分块;
6. **P2-4 残留**:乱序/丢块 e2e、跨语言(Rust 客户端 ↔ Go 服务端)e2e;
7. **P2-5 残留**:clippy / eslint / rustfmt 与 lint 配置;
8. **P2-6**:协议双份手工维护(建议 code-gen 或一致性测试);
9. **P2-9 残留**:STUN closer 接入 main 优雅停机;
10. **P3 群**:死配置/死指标/死依赖(x11rb、rustls feature)、gen/schemas 出库、Cargo.lock 提交、config.example.yaml 与 README 其余失真、metrics 维度、前端零测试、SettingsModal 状态拷贝、首启窗口可见性。

## 5. 工程提醒

- 本轮全部变更**尚未提交**。提交时注意:`Cargo.lock` 目前是未跟踪文件(`.gitignore` 已放行),**务必一并 `git add`**,否则可重现构建承诺不成立;建议同时将 `gen/schemas/` 从版本库移除并忽略。
- 新增测试质量良好(特别是 race 测试与 CAS 测试),但安全回归测试要覆盖"攻击者实际会走的路径"——本轮 `TestDataWSUnauthorizedAccessRejected` 只验证了带坏 token 的路径,而真实旁路恰恰是**不带 token** 的路径,R1 修复时请补上。

---

## 附录:复审执行的验证命令与结果

```
gofmt -l .                      → 0 文件(首轮 4 个,已清零)
go vet ./...                    → 通过
go test -race ./...             → 全部通过(controller 包新增 2 测试)
cargo check(强制重编译)         → 0 警告
cargo test                      → 7/7(PathGuard×4 + SlidingWindow×2 + BitmapRepo×1)
pnpm build(tsc + vite)          → 通过
grep 交叉验证:
  on_nack 调用                  → 0(→ N1)
  MaxItemsPerOffer/HeartbeatInterval/TotalOffers/RetransmitsCount → 仍无消费者
  x11rb 引用                    → 0(依赖仍声明)
  README 环境变量/docs/ 引用     → 错名与失效引用仍在(→ N7)
  StartSTUNServer closer 接线   → 未接线(→ N7/P2-9)
```

> 本报告仅为审核结论与修复建议,未对任何源代码做修改。
