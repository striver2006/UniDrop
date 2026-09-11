# UniDrop 全项目代码复审报告(第三轮)

| 项目 | 内容 |
|---|---|
| 审核对象 | UniDrop 全仓库(相对第二轮复审时的未提交变更,本轮新增约 +200/−22 行改动,叠加后相对 `f3d5125` 共 +2007/−360,36 文件,变更仍未提交) |
| 审核日期 | 2026-09-11 |
| 审核人 | GLM(ZCode) |
| 审核方法 | 逐项核对第二轮通过条件(R1–R5)与新发现问题(N1–N10),通读重写段落,全量只读命令验证 |
| 前置文档 | `2026-09-11-UniDrop代码审核-glm.md`(首轮)、`2026-09-11-UniDrop代码复审-glm.md`(第二轮,判定"不通过,差距收敛为 R1–R5") |

## 复审结论:**通过(附遗留问题清单,见第 4 节)**

第二轮设定的 5 项通过条件(R1–R5)**全部关闭且经代码与测试逐项验证**,超出要求的部分:第二轮 10 项新发现中另有 5 项(N4/N5/N6/N7 + STUN 接线)也一并修复。全部验证命令通过(Go 含 race 检测、Rust 强制重编译零警告、前端构建)。

**本轮质量评价**:R1 的修复方式正确且彻底(空 token 无条件 403 + 回归测试断言 403 状态码,正是上轮建议的"覆盖攻击者实际路径"的测试);R2 的修复伴随了一个精确复现错配场景的单元测试(目录 + 不存在文件混入列表);N5 重试上限落在 SlidingWindow 内部并带测试;失败终态(N4)在收发两侧均闭环且接收失败清理沙盒目录。整体没有发现"为过关而修"的表面功夫。

### 命令验证结果(全部只读,未改动任何代码)

| 验证项 | 结果 |
|---|---|
| `gofmt -l`(server) | ✅ 0 个不合格文件 |
| `go vet ./...` | ✅ 通过 |
| `go test -race ./...` | ✅ 6 个包全部通过(非缓存,fresh run) |
| `cargo check`(强制重编译) | ✅ 0 警告 |
| `cargo test` | ✅ 9/9(较上轮 +2:路径对齐测试、重试上限测试) |
| `pnpm build`(tsc + vite) | ✅ 通过 |

---

## 1. 通过条件 R1–R5 验证明细

### R1 ✅ 数据面空 token 旁路已彻底关闭

- `data_ws.go:42-53`:`token == ""` → 记录 Warn 并直接返回 **403**,不再依赖 `HasAuthSessions()` 条件;`CheckAuthorization` 无条件执行;`GetOrCreatePipe` 回退分支已删除,仅剩 `ValidateAndGetOrCreatePipe`(data_ws.go:67)。
- 回归测试已补且正是攻击路径:`control_ws_test.go:43-51` 以**不带 token** 的 URL 拨号,断言连接被拒且 HTTP 状态码为 403。
- `GetOrCreatePipe` 仍保留在 RelayManager 上(测试与内部使用),但生产路径不可达——可接受。

### R2 ✅ items/paths 错配已修复并带精确测试

- `prepare_offer` 签名改为 `Result<(TransferOfferPayload, Vec<PathBuf>), String>`(transfer_engine.rs:46),返回与 items **一一对应**的有效路径(transfer_engine.rs:122)。
- `cmd_send_files` 存入 `pending_outbound` 的是对齐后的 `valid_paths`(clipboard_cmd.rs:37-38, 59);发送任务按此配对,不再使用原始路径列表。
- 新测试 `test_prepare_offer_aligns_valid_paths_and_skips_dirs`(transfer_engine.rs:667-702):输入 `[目录, file1, 不存在路径, file2]`,断言 items 与 valid_paths 均为 2 且逐位对齐——精确复现了上轮报告的错配场景。

### R3 ✅ 发送方 NACK 快速重传已实装

- transfer_engine.rs:276-298:收到 `ChunkType::Nack` → 定位全局分块索引 → `window.on_nack()`(计入重试)→ 立即从磁盘重读并重发该分块。`on_nack` 不再是死代码。

### R4 ✅ auto_inject 改为读取设置,默认手动

- lib.rs:186-199:接收任务启动前读取 `settings.auto_inject` 并作为参数传入,不再硬编码;默认值 `false`(lib.rs:39,48)。
- 通知文案随模式变化:"已自动装载至剪贴板" / "已保存在沙盒,可在面板中点击装载"(transfer_engine.rs:558-564)。
- ⚠️ 注意:该文案承诺的"面板中点击装载"入口尚不存在,见第 3 节 M1(新遗留,不阻断本轮通过)。

### R5 ✅ README 环境变量名已修正

- README:113 已改为 `UNIDROP_PSK_SECRET`。顺带:README 中失效的 `docs/` 目录引用也已删除。

## 2. 第二轮新发现问题 N1–N10 处置状态

| # | 问题 | 状态 | 证据 |
|---|---|---|---|
| N1 | 发送方不处理 NACK | ✅(= R3) | transfer_engine.rs:276-298 |
| N2 | items/paths 索引错配 | ✅(= R2) | 见上;带专项测试 |
| N3 | auto_inject 硬编码 true | ✅(= R4) | lib.rs:186-199;默认 false |
| N4 | 异常中断无终态事件 | ✅ 已修复 | 发送侧:未完成退出 → emit FAILED + `TRANSFER_FAILURE/TRANSFER_ABORTED`(transfer_engine.rs:351-378);接收侧:`fully_completed` 标志,未完成 → 清理沙盒目录 + emit FAILED + `RECEIVER_DISCONNECTED` 失败信令(593-622) |
| N5 | 重传无上限 | ✅ 已修复 | `MAX_RETRIES = 5`(sliding_window.rs:8);`has_exceeded_max_retries`(44-46);发送循环每轮检查并中止(transfer_engine.rs:209-213);`check_timeouts` 过滤超限分块;新测试 `test_sliding_window_max_retries` |
| N6 | 无盐签名回退 | ✅ 已修复 | verifier.go:82-88:仅接受盐化签名,回退分支已删除 |
| N7 | README 三处错误 | ✅ 已修复 | 环境变量名 ✓;`docs/` 引用删除 ✓;STUN"优雅停机"宣称现已真实:`StartSTUNServerWithCloser` 接入 main.go:45-51,并在 SIGINT/SIGTERM 关停序列中 `Close()`(main.go:109-113) |
| N8 | CLIPBOARD_INJECTED 回执 | ✗ 未修 | 仍仅存在于协议常量定义,收发双方均未使用(P3 遗留) |
| N9 | 认证失败无限快速重连 | ◐ 改善 | connection_actor.rs:被服务端明确拒绝时 30 秒退避(区分凭证错误与网络异常);仍无限重试,可接受(P3 遗留) |
| N10 | offer 无条件自动接受 | ✗ 未修 | lib.rs:130-149 仍自动 answer;因 R4 后默认手动注入,隐私影响已大幅下降(P3 遗留) |

**超出通过条件的额外修复**:STUN 优雅停机接线(P2-9 残留关闭)、`x11rb` 无用依赖从 Cargo.toml 移除(P3-3 进一步)、`gen/schemas/` 4 个生成文件已从版本库删除(P3-4 关闭,当前为暂存的 D 状态)、设置保存的 UI 文案改为诚实的"重启客户端后新配置生效"(App.tsx:115)。

## 3. 本轮新发现问题(2 项,均不阻断)

### M1【P2】手动注入是"死胡同":默认模式下文件无法从面板装载

- **位置**:通知文案承诺"已保存在沙盒,**可在面板中点击装载**"(transfer_engine.rs:562),但前端**没有任何**调用 `cmd_inject_files` 的入口(grep 证实仅 SettingsModal 提及 auto_inject 字样);`cmd_inject_files` 命令(lib.rs 注册、clipboard_cmd.rs:10-25)处于零调用状态。
- **影响**:默认配置(auto_inject=false)下,接收的文件永远留在沙盒缓存目录,用户被通知引导去点击一个不存在的按钮。设计的核心交互"通知 → 点击 → 注入"在手动路径上仍未闭环——这是当前**最用户可见**的功能缺口。
- **建议**:在 TransferProgress 或通知点击事件中接一个"装载到剪贴板"按钮调用 `cmd_inject_files`(顺便会打上 2 小时免疫锁,见 M2)。

### M2【P3】自动注入路径未打 2 小时剪贴板免疫锁

- **位置**:`mark_clipboard_injected` 仅在 `cmd_inject_files`(手动路径)中调用(clipboard_cmd.rs:21);自动注入路径(transfer_engine.rs:567-572)注入后未标记。
- **影响**:LRU 配额压力下,刚注入剪贴板的文件可能被清扫删除,用户随后的 Ctrl+V 落空(正是该锁设计要防的场景;24h TTL 平时兜底,仅配额压力时暴露)。
- **建议**:自动注入成功后同样调用 `mark_clipboard_injected`。

## 4. 遗留问题清单(按优先级,供后续迭代)

**部署前必办(安全)**
1. `InsecureSkipVerify: true` 仍在两个 WS 端点(control_ws.go:37、data_ws.go:56),全链路无 TLS。当前 token 鉴权 + HMAC 已显著压缩可利用面(浏览器无法伪造凭据),但明文 ws 下 token 与信令可被链路嗅探。生产部署:反代 TLS + Origin 校验,或服务端原生 TLS。

**重要(P1/P2)**
2. 每 IP 握手限流/封禁仍未实现(DESIGN 4.5.1),认证接口可无限尝试(P1-8 残留);
3. 手动注入入口缺失(M1,本轮新发现);
4. 设置修改需重启生效——UI 已诚实标注,建议下轮实现 actor 热重连(P1-5 残留);
5. 发送任务 `read_file_chunk` 仍在 async 上下文同步读 4MB 分块(P1-10 残留);
6. Windows/Linux 剪贴板监听仍为空桩,`clipboard-updated` 事件无前端消费者(P1-9 残留);
7. 测试缺口:乱序/丢块重传 e2e、真实 Rust 客户端 ↔ Go 服务端跨语言 e2e(P2-4 残留);
8. clippy / eslint / rustfmt 检查与配置文件仍缺(P2-5 残留);
9. Go/Rust 协议定义仍双份手工维护(P2-6,长期项)。

**一般(P3)**
10. CLIPBOARD_INJECTED 回执(N8)、offer 自动接受(N10)、认证拒绝后仍无限重试(30s 间隔,N9)、自动注入未打免疫锁(M2)、`MaxItemsPerOffer`/`HeartbeatInterval` 死配置与 `TotalOffers`/`RetransmitsCount` 死指标(handler 硬编码 1000)、`tokio-tungstenite` rustls feature 未用、`config.example.yaml` 仍暗示不存在的 YAML 加载、SettingsModal 打开时一次性拷贝设置、首启 `visible:false` 无窗口、`/metrics` 缺设计承诺的维度与 trace_id 贯穿、前端零测试。

## 5. 提交建议(当前全部变更尚未提交)

```
git add -A                # 注意必须包含以下未跟踪文件
  client/src-tauri/Cargo.lock        # .gitignore 已放行但仍是 ?? 状态,不入库则可重现构建不成立
  client/src/components/SendModal.tsx
  server/internal/controller/control_ws_test.go
  server/internal/registry/session_race_test.go
# gen/schemas 的删除已暂存,一并提交
```

建议提交信息可概括为:`fix: close code-review pass conditions R1-R5 (data-plane token enforcement, path alignment, NACK retransmit, auto_inject setting, docs) plus terminal states, retry cap, and salt-only auth`。

---

## 附录:本轮执行的验证命令与结果

```
gofmt -l .                       → 0 文件
go vet ./...                     → 通过
go test -race ./...              → 6 包全部通过(fresh run)
cargo check(强制重编译)          → 0 警告
cargo test                       → 9/9(PathGuard×4、SlidingWindow×3、BitmapRepo×1、prepare_offer 对齐×1)
pnpm build(tsc + vite)           → 通过
grep 交叉验证:
  InsecureSkipVerify             → 仍在 ×2(遗留 #1)
  限流实现                       → 无(遗留 #2)
  cmd_inject_files 前端调用      → 0(M1)
  mark_clipboard_injected 调用   → 仅手动路径(M2)
  x11rb                          → 已从 Cargo.toml 移除
  README docs/ 引用              → 已删除
```

> 本报告仅为审核结论与修复建议,未对任何源代码做修改。
