---
schema: trivium.disposition.v1
topic: macos-dock-hidden
stage: code
role: claude
kind: response
run_id: 20260914T103457Z
addresses:
  - AGY-01
  - AGY-02
  - GLM-01
  - GLM-02
  - GLM-03
---

# 代码审查应答：只在菜单栏显示图标、Dock 不显示

- 主题：`macos-dock-hidden`
- 阶段：code（第 1 轮，含逐条裁决）
- 日期：2026-09-14
- 被审对象：`git diff HEAD`（HEAD = 6b398740），6 个文件，补丁 30.3 KB
- 两侧结论：Antigravity `approve-with-nits`（吹毛求疵 2）；ZCode `approve-with-nits`（次要 3）

## 裁决表

共 5 条，**接受 5 条、驳回 0 条、暂缓 0 条**。两侧零 blocker、零 major。

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GLM-02 | glm | minor | 接受 | **本轮唯一的真实行为缺陷，且是我这次改动引入的。** 核实属实：`cmd_save_settings`（settings_cmd.rs:281）运行期会更新 `AppState::settings`，而 `spawn_placement_watchdog` 在 setup 时把 `start_minimized` 拍成快照往下传。旧代码里这个快照寿命有限（定论 Placed 即 `return`，loop 退出），我删掉两处 `return` 改成常驻之后，它的寿命变成了整个进程生命期——运行期 `Placed→Rejected` 翻转时，`should_force_reveal` 用的是启动那一刻的值：用户后来关掉该设定仍会被强制弹窗，后来打开的人反而拿不到那次安全唤起。GLM 还多指出一层我没意识到的后果：拉通道 `current_payload` 本来就是现读的，快照留着会让推、拉两条通道对同一字段给出不同答案。这是「改了生命周期却没同步审视被延长的状态」的典型疏漏，审查员抓得准。 | 新增 `read_start_minimized(app)`（抄 `current_payload` 的取法），`handle_not_placed` 去掉 `start_minimized` 入参改为内部现读；`spawn_placement_watchdog` 去掉该参数，阶段一与阶段二两处 `build_payload` 调用点各自现读；`lib.rs` 调用点同步简化。推/拉两通道口径就此统一 |
| AGY-01 | gemini | nit | 接受 | 属实，我那句话写错了。`LAUNCH_DOCK_POLICY` 的注释断言「不存在『28 秒里彻底没有入口』的窗口期」，但开了 `start_minimized` 又恰好被拒的用户，在定论之前的那 28 秒里菜单栏没图标、Dock 没图标（乐观押了 Accessory）、窗口也不显示——**确实是没有入口的**。我真正能保证的只是「这段窗口期有确定上界，且定论那一刻会补上入口」，把它写成「不存在」是把结论说过头了。 | 见 GLM-01 落点（同一处，合并改写） |
| GLM-01 | glm | minor | 接受 | 与 AGY-01 同一处，两侧独立命中，合并处理。GLM 给出的角落比 AGY-01 更精确，也更该记进注释：**已点过「下次不要自动打开」的用户在定论时刻根本不会被唤窗**——`should_force_reveal` 因 `opted_out` 返回 false，而能点到那个按钮的人 `guidance_shown` 必然已置位、首次分支也不会再走（handle_not_placed 两个分支都跳过）。他们拿到的入口只有 Dock 图标本身加上「访达/启动台再打开」。GLM 明确说了行为本身可接受、只要求把注释这条「不变量」改弱，我同意：这是用户自己关掉打扰换来的，不该为它改行为，但更不该让后来者据此推理「窗口必弹」而在别处省掉留给他们的入口。 | `tray_placement_macos.rs` 的 `LAUNCH_DOCK_POLICY` 文档注释重写：把「不存在无入口窗口期」改为「有确定上界 28s」，并显式分列「一般情况窗口与 Dock 一起出现」与「opted-out 用户只拿到 Dock 图标 + Reopen 那条路」两种补法，附带写明不要因为记住第一条就省掉第二条的入口 |
| AGY-02 | gemini | nit | 接受 | 属实。`policy_transition_dedupes` 只覆盖了 `Placed`/`Rejected` 的迁移组合，`Unknown` 一次都没进过去重逻辑的断言。而 `Unknown`（判据失效）恰恰是这套启发式最可能的失败形态，测试把最可能出问题的那一态漏掉了。 | `policy_transition_dedupes` 补 4 条断言：`(Some(Hidden), Unknown) → Some(Visible)`、`(Some(Visible), Unknown) → None`、`(None, Unknown) → Some(Visible)`，外加一条 `Unknown ↔ Rejected` 互切不产生多余调用（两态期望策略相同，Dock 图标不该因判据在这两态间抖动而重复弹） |
| GLM-03 | glm | minor | 接受 | 核实属实，且三处文案的受众恰好就是会被这句话误导的人。`FIRST_PROBE_DELAY=3s` + `RETRY_INTERVAL=5s × (MAX_RETRIES-1)=5` = 28s，代码自己的注释都按 28s 计，而面向用户的三处都写着「启动几秒后自动打开」。被拒 + 启动即最小化的用户按「几秒」的预期等不到窗口，会误判成又坏了一层——这正是本模块从一开始就想根除的那类「用户得不到解释」的哑故障，文案不该反过来制造一个。 | 三处统一改为「约半分钟内（最长约 28 秒）」并说明这段等待是在确认系统有没有给菜单栏位置：`docs/INSTALL.md:146`、`docs/USER_GUIDE.md:55`、`MenuBarHiddenBanner.tsx:63` |

## 说明：为什么 5 条全部接受

本轮没有驳回项，不是因为放松了判断，而是这 5 条的性质本身：
GLM-02 是可验证的行为缺陷（我逐行核对了 `cmd_save_settings` 确实运行期写 `state.settings`）；
AGY-01 / GLM-01 是我写下的一句事实错误的注释；
AGY-02 是能真失败的测试缺口；
GLM-03 是代码常量与用户文案对不上的硬矛盾。
每一条都能指向具体行号并当场核实，没有一条属于风格偏好或需要权衡取舍的设计分歧。

两侧同时把「删掉两处 `return`」认定为必要修复而非风险点（GLM 还补充核实了 `CURRENT_POLICY` 单写者无竞态、稳态去重不真调），
也把三个焦点的论证链逐层复核为真（GLM 对着 `Cargo.lock` 核了 tauri 2.11.5 / tao 0.35.3 等五个版本号与我引用的行号全部一致）。
这两点让我对改动主干有了比自测更强的信心。

## 复核后的验证

- `cargo test --lib`：**154 通过 / 0 失败**（`policy_transition_dedupes` 扩充后仍全绿）
- `cargo clippy --all-targets`：改动文件零新增警告（仅剩 `probe_hosted_extra` 里两处既有的 `needless_borrow`，位于 236/239 行，非本次引入）
- `pnpm build`：tsc + vite 通过

## 遗留项

- **L1**：两侧都声明未做真机验证（GLM 明确列为盲区：未构建、未运行、未在 macOS 26 上实测）。我在提交前已实测 release 与 dev 两种形态的 `ApplicationType="UIElement"`、`AXFrontmost=true`、以及 `Placed → switching activation policy to Hidden` 的日志，但 **Rejected 兜底切回 Regular 的端到端未实测**——制造它需要从 IDE 集成终端直接跑二进制，那会把 `com.unidrop.client` 挂到 IDE 名下且不会自动清理（模块头已警告）。该路径目前由 `policy_transition_dedupes` 与 `unknown_takes_the_dock_but_never_warns` 两个单测覆盖，切换动作本身已在 Placed 路径实证生效。
- **L2**：GLM 盲区提到「首次结论为 Placed 时同值 `setActivationPolicy(Accessory)` 的运行时副作用未实证」。实测日志确认该次调用发生且 Dock 始终无图标，无可见副作用；但这是 `policy_transition(None, _)` 首次必落语义的必然结果，保留在此备查。
- **L3**：Accessory 下 Cmd+C / Cmd+V 的 key equivalent，GLM 从源码层核实了 tauri 默认菜单含 Edit 子菜单且派发不依赖菜单栏可见性，两侧均未实机敲键。留作真机验收项。
