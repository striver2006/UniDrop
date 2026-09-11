# UniDrop 全项目代码复审报告(第四轮·增量确认)

| 项目 | 内容 |
|---|---|
| 审核对象 | 提交 `08d3c17`("fix: resolve code review findings R1-R5, N1-N10, and M1-M2")——第三轮审核时的未提交变更 + 本轮新增的 M1/M2 修复,现已全部入库;工作区干净 |
| 审核日期 | 2026-09-11 |
| 审核人 | GLM(ZCode) |
| 审核方法 | 增量核对第三轮遗留项(重点 M1/M2 与提交卫生),通读新增段落,全量只读命令验证 |
| 前置文档 | 首轮代码审核 → 第二轮复审(不通过,R1–R5)→ 第三轮复审(**通过**,遗留 M1/M2 及清单) |

## 复审结论:**通过(维持第三轮判定,无新增阻断项)**

第三轮的两项新发现(M1 手动注入死胡同、M2 自动注入未打免疫锁)**均已正确修复并闭环**;此前反复提醒的提交卫生问题(Cargo.lock 入库、gen/schemas 出库)也已完成。全部验证命令通过。

### 命令验证结果(全部只读,未改动任何代码)

| 验证项 | 结果 |
|---|---|
| `gofmt -l`(server) | ✅ 0 个不合格文件 |
| `go vet ./...` | ✅ 通过 |
| `go test -race ./...` | ✅ 6 个包全部通过 |
| `cargo check`(强制重编译) | ✅ 0 警告 |
| `cargo test` | ✅ 9/9 |
| `pnpm build`(tsc + vite) | ✅ 通过 |

---

## 1. 上轮遗留项处置验证

### M1 ✅ 手动注入路径已闭环

- **命令侧**:`cmd_inject_files` 签名改为 `paths: Option<Vec<String>>`(clipboard_cmd.rs:10-32)——`paths` 为空时按 `session_id` 从 `cache_entries` 表解析该会话的全部已验证文件(`get_session_files`,cache_manager.rs),查无文件时报错;注入走 `spawn_blocking`,注入后打 2 小时免疫锁。
- **UI 侧**:TransferProgress 对 `status === "COMPLETED"` 且方向为 RECEIVE 的条目渲染"装载到剪贴板"按钮(TransferProgress.tsx:26, 102-124);App.tsx:134-141 的 `handleInjectTransfer` 调用 `cmd_inject_files`(paths 传 null)并给出成功/失败 toast。
- 通知文案承诺的"可在面板中点击装载"现在真实存在。设计的核心交互"接收 → 面板 → 点击 → 注入 → Ctrl+V"首次完整可用。

### M2 ✅ 自动注入已打 2 小时免疫锁

- transfer_engine.rs:573:自动注入成功后调用 `mark_clipboard_injected(&session_id)`,与手动路径一致,LRU 清扫不会再误删剪贴板正在引用的文件。

### 提交卫生 ✅

- `client/src-tauri/Cargo.lock` 已入库(5851 行,可重现构建成立);
- `gen/schemas/` 4 个生成文件已删除,且 `.gitignore` 新增 `**/gen/schemas/` 防回流;`.zcode/` 已忽略;
- 三个审核报告文件随本提交入库(作者选择,无异议)。

## 2. 一处提醒:提交信息与实际范围略有出入(不影响判定)

提交信息称"resolve code review findings R1-R5, **N1-N10**, and M1-M2",实际:
- **N8**(CLIPBOARD_INJECTED 回执)**未实现**——仍无任何发送方(grep 证实);
- **N9** 为改善而非解决(认证被拒后 30 秒退避,但仍无限重试);
- **N10**(offer 无条件自动接受)未改动。

以上均为第三轮已明确"不阻断"的 P3 遗留,处置本身没问题;但提交信息表述强于事实,建议后续提交信息按实际范围措辞,避免误导后续维护者。

## 3. 遗留问题清单(承接第三轮,无新增阻断项)

**部署前必办(安全)**
1. `InsecureSkipVerify: true` 仍在 control_ws.go:37、data_ws.go:56,全链路无 TLS(token/信令在明文 ws 上可被链路嗅探);
2. 每 IP 握手限流/封禁未实现,认证接口可无限尝试。

**迭代建议(P1/P2)**
3. 设置修改需重启生效(UI 已诚实标注,建议实现 actor 热重连);
4. 发送任务 `read_file_chunk` 仍在 async 上下文同步读 4MB 分块(transfer_engine.rs:219, 282, 310);
5. Windows/Linux 剪贴板监听空桩;`clipboard-updated` 事件无前端消费者;
6. 乱序/丢块重传 e2e、真实 Rust 客户端 ↔ Go 服务端跨语言 e2e;
7. clippy / eslint / rustfmt 门禁与配置文件;
8. Go/Rust 协议双份手工维护(长期项)。

**一般(P3)**
9. CLIPBOARD_INJECTED 回执(N8)、offer 自动接受(N10)、认证拒绝无限重试(N9)、`MaxItemsPerOffer`/`HeartbeatInterval` 死配置与 `TotalOffers`/`RetransmitsCount` 死指标、rustls feature 未用、`config.example.yaml` 误导、SettingsModal 一次性拷贝、首启 `visible:false`、metrics 维度与 trace_id、前端零测试。

**本轮补充小注(P3)**
10. 已完成的传输卡片被用户手动移除后,该会话文件仍在沙盒(24h TTL)但面板不再有装载入口;`get_session_files` 未按 item_index 排序,多文件注入顺序可能与原顺序不一致。均为体验细节。

---

## 附录:本轮执行的验证

```
git: HEAD=08d3c17,工作区干净;Cargo.lock 已入库;gen/schemas 已删并忽略
gofmt -l .            → 0 文件
go vet ./...          → 通过
go test -race ./...   → 6 包全部通过
cargo check(重编译)   → 0 警告
cargo test            → 9/9
pnpm build            → 通过
grep:M1 前端调用 cmd_inject_files ✓;M2 mark_clipboard_injected 注入路径 ✓;
     CLIPBOARD_INJECTED 发送方 = 0(N8 未修,提交信息与事实不符处);
     isDone === "COMPLETED"(失败接收不出现装载按钮,正确)
```

> 本报告仅为审核结论与修复建议,未对任何源代码做修改。
