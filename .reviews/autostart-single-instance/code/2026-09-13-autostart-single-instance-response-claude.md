---
schema: trivium.disposition.v1
topic: autostart-single-instance
stage: code
role: claude
kind: response
run_id: 20260912T173028Z
addresses:
  - AGY-01
  - GLM-01
  - GLM-02
  - GLM-03
  - GLM-04
---

# 代码审查应答：开机自启配置项 + 单实例加固

- 主题：`autostart-single-instance`
- 阶段：code（应答确认，含逐条裁决）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`code/2026-09-13-autostart-single-instance-gemini.md`、`code/2026-09-13-autostart-single-instance-glm.md`

---

## 0. 裁决摘要

两侧均判 `approve-with-nits`，无 blocker、无 major，共 5 条意见。

- **接受 3 条**（AGY-01、GLM-01、GLM-02）——其中前两条是同一处前端竞态，两侧独立发现；
- **暂缓 2 条**（GLM-03、GLM-04）——均为上游依赖与供应链议题，提出方自己也标注
  「不需要在本补丁内修复」／缺少可登记的基线，本轮登记为遗留。

需要改代码的只有一处：`SettingsModal` 自启开关的初始拉取竞态（§2.1）。
GLM-02 落到文档注释与遗留登记（§2.2）。

---

## 1. 逐条裁决

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 / 修改计划 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | nit | 接受 | 属实。`useEffect` 初始拉取期间没有置 `autostartBusy`（它只在 `handleAutostartChange` 内置位），复选框可点。组件初值 `false`，若 OS 实际为开启且读取有延迟，用户在窗口期点击会触发与预期相反的写入。 | `SettingsModal.tsx`：初始拉取前置 `autostartBusy = true`，`finally` 中恢复；加载期间复选框禁用 |
| GLM-01 | glm | minor | 接受 | 与 AGY-01 同一处，但指出了更完整的失效面：① 迟到的 `.then(setAutostart)` 会覆盖用户刚做的切换；② `handleAutostartChange` 回滚用的 `previous` 快照可能取到过期值；③ 快速关开弹窗时两次 in-flight 拉取可能乱序回填。三条都成立，仅置 busy 不足以覆盖 ②③。 | `SettingsModal.tsx`：① 置 busy（同 AGY-01）；② 用 `cancelled` 标志 + `useEffect` cleanup 丢弃过期回调，解决乱序回填；③ 切换失败时改为 `cmd_get_autostart` **重读真实值**而非用快照回滚 |
| GLM-02 | glm | minor | 接受 | 语义差距属实且核验扎实（直接读了 `auto-launch-0.5.0/src/windows.rs:73-83` 与 `macos.rs:161-176`）：`is_enabled` 只判「注册项/plist 存在」，不比对其中路径是否仍指向 `current_exe`。dev 二进制上开过自启、之后装正式版，就会出现「开关显示开、开机实际不启动」。**但采纳的是建议中的下限（写清语义），不做路径自愈**——理由见 §2.2。 | `settings_cmd.rs`：在 `cmd_get_autostart` / `cmd_set_autostart` 的文档注释中写明「存在 ≠ 会生效」及其触发条件；§3 遗留登记 L6；§4 手工用例 16 |
| GLM-03 | glm | nit | 暂缓 | 缺陷属实（`auto-launch` 的 macOS plist 用 `format!` 直接内插、无 XML 转义；Windows Run 值不给路径加引号），但它在**上游依赖内部**，本补丁无法修复。提出方自己明确写了「不需要在本补丁内修复」，且触发条件是用户目录含 `&`、空格等特殊字符，罕见。绕过它意味着放弃 `manager.enable()` 自行写注册表/plist，那是把整个官方插件的平台适配重写一遍——与本轮范围严重不符。 | 不改代码。§3 遗留登记 L7（含上游文件与行号，便于日后跟进 issue） |
| GLM-04 | glm | nit | 暂缓 | 依赖树属实（`auto-launch 0.5.0 → winreg 0.10.1 → winapi`，RUSTSEC-2020-0017 informational；`dirs` / `redox_users` / `winreg` 三组双版本共存）。但建议的落点是「在 cargo audit 基线中登记」，而**本仓库尚未引入 cargo audit**——`client-ci.yml` 只有 `pnpm build` + `cargo check` + `cargo test`，没有供应链扫描。无基线可登记，现在建立扫描流程属于另一件事。且这是官方插件的既有依赖树，非本补丁可选择。 | 不改代码。§3 遗留登记 L8，待引入供应链扫描时与 advisory ID 一并处理 |

**接受 3 条，驳回 0 条，暂缓 2 条。**

编排器附带的结构提示（两侧 evidence 引用了若干文件但未列入 consulted）经核对为路径写法差异导致的误报：
gemini 侧清单第 111 行、glm 侧第 147 行均列出了 `SettingsModal.tsx`；glm 侧提到的
`src/macos.rs` / `src/windows.rs` 是 `~/.cargo/registry` 下的 **auto-launch 依赖源码**，
不是本项目文件，本就不应出现在项目文件清单里。不影响任何一条意见的采信。

---

## 2. 修改计划（S9 只改已接受项）

### 2.1 前端自启开关的竞态（AGY-01 + GLM-01）

`client/src/components/SettingsModal.tsx` 三处改动：

1. **初始拉取置 busy**：进入 `useEffect` 立即 `setAutostartBusy(true)`，在 `finally`
   中恢复。加载期间复选框 `disabled`，堵住 AGY-01 指出的「显示 false 但实际 true 时
   用户点击触发反向写入」。
2. **丢弃过期回调**：`useEffect` 内声明 `let cancelled = false`，cleanup 里置 `true`；
   `.then` / `.catch` / `finally` 均先检查该标志再 `setState`。这解决 GLM-01 的第 ①③ 条
   （迟到响应覆盖用户操作、快速关开弹窗时乱序回填）。
3. **失败回滚改为重读真实值**：`handleAutostartChange` 的 catch 分支不再用 `previous`
   快照，而是 `invoke("cmd_get_autostart")` 重读 OS 实时状态回填；重读本身也失败时，
   才退回快照并提示。这解决 GLM-01 的第 ② 条。

### 2.2 `is_enabled` 的语义边界（GLM-02）

**只改注释，不做路径自愈**，理由：

- 自愈需要分平台解析注册表值 / plist `ProgramArguments` / `.desktop` 的 `Exec`，
  再与 `current_exe()` 比对——那是三套平台特定的解析代码，恰恰是引入
  `tauri-plugin-autostart` 想避开的东西，且解析失败的兜底策略本身又是新的风险面。
- 强制重写（`disable` + `enable`）可以绕开解析，但会直接破坏 AGY-02（plan 阶段）
  要求的幂等——那条是为了避免 macOS 13+ 反复弹后台项横幅而专门加的。
  两条要求互斥，本轮保幂等。
- 真实影响面窄：正式安装包的路径稳定，主要命中「dev 二进制开过自启后装正式版」
  这一开发者场景。

因此落点为：把「存在 ≠ 会生效」连同触发条件写进两个 command 的文档注释，
让后来者不会误以为 `true` 等价于自启可用；同时登记遗留 L6 与手工用例 16。

### 2.3 不改动的部分

GLM-03、GLM-04 均判暂缓，**不产生任何代码改动**，只在修订计划的遗留清单中登记。

---

## 3. 遗留清单增补

在已批准修订计划 §7 的 L1~L5 之上追加：

- **L6**（GLM-02）`tauri-plugin-autostart` 依赖的 `auto-launch 0.5.0` 中，
  `is_enabled()` 只检查注册项/plist 是否存在，不校验其中路径是否仍指向当前可执行文件
  （`windows.rs:73-83`、`macos.rs:161-176`）。应用被移动或先在 dev 二进制上开启后
  再安装正式版时，开关显示开启但开机自启实际失效。本轮以注释写明语义边界，
  若将来用户反馈命中，再评估分平台路径校验与自愈。
- **L7**（GLM-03）同一依赖的两个边界缺陷：macOS plist 用 `format!` 内插路径与参数、
  无 XML 转义（`macos.rs:87-111`），路径含 `&` 会生成非法 plist 被 launchd 拒绝；
  Windows Run 值写成 `路径 参数` 且路径不加引号（`windows.rs:40-43`），
  含空格路径依赖 CreateProcess 前缀探测。两者均表现为「开关已开但开机不自启」。
  属上游缺陷，待跟进 upstream issue。
- **L8**（GLM-04）新增传递依赖 `auto-launch 0.5.0 → winreg 0.10.1 → winapi`
  （RUSTSEC-2020-0017，unmaintained，informational 级），并使 `dirs`、`redox_users`、
  `winreg` 三组 crate 双版本共存。当前仓库无 cargo audit 流程；引入供应链扫描时
  应同步登记该 advisory 基线，避免 informational 噪音掩盖真问题。

---

## 4. 验证增补

在已批准修订计划 §6.2 的 15 条手工用例之上追加：

- **用例 16**（GLM-02）在 dev 二进制上开启「开机自动启动」，随后安装正式版并运行：
  确认开关显示状态与实际是否自启一致；若不一致，记录为 L6 的实测证据。
- **用例 17**（AGY-01 + GLM-01）打开设置弹窗的瞬间连续快速点击自启开关：
  加载期间开关应为禁用态、不可点击；加载完成后显示的必须是 OS 实时状态。
- **用例 18**（GLM-01 ③）快速连续开关设置弹窗数次：最终显示的自启状态必须与
  OS 实时状态一致，不出现乱序回填导致的错误显示。

自动化侧不变：`cargo check`、`cargo test`、`pnpm build`，以及出口的 `trivium verify`。
本轮改动集中在前端状态机，项目无前端测试基建（glm 侧盲区已指出），
故用例 17、18 只能手工覆盖。
