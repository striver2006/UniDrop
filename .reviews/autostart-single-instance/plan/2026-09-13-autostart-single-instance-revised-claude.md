---
schema: trivium.disposition.v1
topic: autostart-single-instance
stage: plan
role: claude
kind: revised
run_id: 20260912T171855Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - AGY-04
  - AGY-05
  - GLM-01
  - GLM-02
  - GLM-03
  - GLM-04
  - GLM-05
  - GLM-06
  - GLM-07
---

# 修订计划：开机自启配置项 + 单实例加固

- 主题：`autostart-single-instance`
- 阶段：plan（修订稿，含逐条裁决）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`2026-09-13-autostart-single-instance-gemini.md`、`2026-09-13-autostart-single-instance-glm.md`

---

## 0. 裁决摘要

两侧均判 `request-changes`，共 12 条意见，**全部接受**，无驳回、无暂缓。

意见收敛为两处真实的设计缺陷，它们各自被两侧从不同角度指出：

1. **`start_minimized` 语义自相矛盾**（AGY-01 阻断 + GLM-03）——原稿一边在真值表里写死
   「手动启动永远显示窗口」，一边在 UI 文案里承诺「手动启动也可能想最小化」。
2. **保存写路径的事务边界未定义**（GLM-01 重要 + AGY-02 重要 + GLM-02 + AGY-04）——
   把 OS 级自启写操作塞进全量配置保存命令，产生 fail-all 事务、缺幂等、
   内存/OS 状态双份且会漂移、前端无错误承接四个连带问题。

对第 2 点，两侧各给了一半解法（AGY-02 说「解耦错误粒度」，GLM-01 说「拆成独立命令
与表单保存解耦」）。本修订**采纳拆分方案**，它一次性消掉全部四个连带问题：
`autostart_enabled` 不再进 `AppSettings`、不落 SQLite，OS 成为唯一事实源（§3.1）。

---

## 1. 逐条裁决

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | blocker | 接受 | 矛盾属实：原稿 §3.2 真值表写死手动启动恒显示，§4.2 文案却承诺手动也能最小化，二者不可能同时为真。采纳其**推荐分支**（冷启动全局生效），理由见 §3.2——用户已明确要求两个开关「自由组合」，若 `start_minimized` 只对自启生效，它就不是独立开关。 | §3.2 新真值表；§3.3 函数签名重写；§4.1 `lib.rs` 显隐判定改为只看 `start_minimized` |
| AGY-02 | gemini | major | 接受 | 无条件重写 OS 自启项属实：原稿写路径不做比对，改 PSK 也会触发 LaunchAgent 重写，macOS 13+ 会反复弹后台项横幅。错误捆绑同样属实。 | §3.1 拆分为独立命令（消除捆绑）＋ §3.4 幂等：先查 OS 现值，目标态==现值则直接返回成功，不触碰 OS |
| AGY-03 | gemini | major | 接受 | 属实且是明确的功能缺陷：Windows 上对已最小化窗口只调 `show()` + `set_focus()` 无法还原。原稿完全没考虑最小化到任务栏这一形态。 | §3.5 `second-instance` 回调补 `is_minimized()` → `unminimize()`；§3.6 首次启动显示路径同样补上 |
| AGY-04 | gemini | minor | 接受 | 断层属实，且原稿对 `cmd_get_settings` 的描述有误——它读的是内存 `state.settings`（`settings_cmd.rs:15-18`），不是 SQLite，原稿 §3.1 写错了。采纳拆分方案后该字段不再有内存/SQLite 副本，断层从根上消失。 | §3.1：`autostart_enabled` 退出 `AppSettings`，无缓存即无漂移 |
| AGY-05 | gemini | nit | 接受 | 属实的范围蔓延，且我在原稿 §4.3 自己就标注了「可驳回」。`client-ci.yml:53-59` 已跑 `cargo check` / `cargo test`，本地出口的缺口由 §6.1 的强制手动执行补齐，不必动编排系统配置。 | 删除原稿 §4.3；`.trivium/config.yaml` **不改**；§6.1 把 cargo 两条列为 S10 前必须手动执行并报告结果 |
| GLM-01 | glm | major | 接受 | 最要害的一条。原稿只说「失败返回 Err」，没决定其余字段怎么办；且 `SettingsModal.tsx:27-33` 提交后无条件 `onClose()`、不 `await onSave`，错误 toast 在弹窗关闭后才出现，「不能静默吞掉」根本落不了地。 | §3.1 拆分使两个失败域彻底隔离；§4.2 `SettingsModal.handleSubmit` 改为 `await onSave(...)` 成功才 `onClose()`，`App.tsx:150-163` 的 `handleSaveSettings` 改为失败时 `throw` |
| GLM-02 | glm | minor | 接受 | 「陈旧值复活」这条比 AGY-02 更准确：比对基准必须是 **OS 现值**而非旧内存值，否则会话中途用户在系统设置里关掉自启、再保存无关设置会把它悄悄打开——这正是 G4 要防的漂移，而读时覆盖防不住写路径。 | §3.4 比对基准明确为 `is_enabled()` 的实时返回值；§6.2 新增手工用例 7b |
| GLM-03 | glm | minor | 接受 | 与 AGY-01 同一处矛盾。**核心诉求（消除文案与实现的矛盾）被满足，但采用的是 AGY-01 的方向而非本条推荐的「维持 §3.2、文案改为仅对自启生效」**——如实记录此差异：拆开写在这里是因为两条建议指向相反分支，选择理由见 §3.2。 | §3.2 语义统一为「冷启动全局生效」；§4.2 文案随之改为「启动时不弹出主窗口」；§6.2 新增用例 5b（手动启动 + 开关开 → 不弹窗，再次双击 → 弹窗） |
| GLM-04 | glm | minor | 接受 | 属实的测试缺口：`--silent` 早退分支是本轮单实例加固的唯一新分支，原稿 9 条用例无一覆盖。 | §6.2 新增手工用例 10；§6.1 纯函数单测覆盖该分支 |
| GLM-05 | glm | minor | 接受 | 属实且价值高：`#[serde(default)]` 是本轮风险最高的一行代码，只靠手工验证，将来任何人删掉它都无人拦截。 | §6.1 新增 `settings_cmd.rs` 的 `#[cfg(test)]`：用旧格式 JSON 字面量反序列化，断言旧字段保真、新字段取默认 |
| GLM-06 | glm | nit | 接受 | 属实的自相矛盾：原稿 §4.2 声明前端不碰插件 JS API，§4.1 却要加 `autostart:default`——那是放开 webview→插件的 IPC 命令面，纯增攻击面。Rust 侧经 manager 调用不需要该权限。 | §4.1 删除 `capabilities/default.json` 的改动项，该文件**不改** |
| GLM-07 | glm | nit | 接受 | 属实：`lib.rs` 全文无 `set_activation_policy`，macOS 默认 Regular 策略下隐藏窗口仍有 Dock 图标。「只在系统托盘常驻」的文案在 macOS 上不准确。 | §5 macOS 行注明该差异并明确取舍（接受 Dock 图标，不在本轮引入 Accessory 策略）；§4.2 副标题措辞避开「只在托盘」 |

**驳回 0 条，暂缓 0 条。**

编排器附带的结构提示（gemini 侧 evidence 引用了 `settings_cmd.rs` / `SettingsModal.tsx` /
`lib.rs` 但未列入 consulted）经核对为误报：该侧「实际查阅的项目文件」清单第 148-150、154 行
确实列出了这三个文件，只是 evidence 里用了相对 `client/` 的短路径而 consulted 用了全路径。
不影响任何一条意见的采信。

---

## 2. 修订后的目标

- G1 新增「开机自动启动」开关：开 / 关操作系统级自启注册项，三端可用。
- G2 新增「启动时最小化到托盘」配置项：冷启动时不弹主窗口。
- G3 两项在 `SettingsModal` 可见可改，重启后保持。
- G4 自启的**唯一**事实源是操作系统——不再有本地副本，因此无漂移可言。（修订：原为「读时覆盖」）
- G5 单实例加固：`second-instance` 正确处理 `args` 与最小化状态；显隐决策抽成纯函数并单测。

非目标不变（不做安装包写注册表、不做签名公证、不碰 `docs/需求.md` 第 3~8 条、
不修 §7 遗留）。**新增一条非目标**：不改 `.trivium/config.yaml`（AGY-05）。

---

## 3. 修订后的设计

### 3.1 自启与本地配置彻底分离（解决 GLM-01 / AGY-02 / AGY-04）

原稿把 `autostart_enabled` 塞进 `AppSettings`，随全量保存走同一条命令。
这条路径同时承担网络配置持久化、内存态更新、actor 配置更新、清空设备表、
触发重连（`settings_cmd.rs:41-70`），把一个 OS 系统调用挂上去会让两个
毫不相干的失败域互相污染。

**修订**：

| 配置项 | 归属 | 事实源 | 读写路径 |
| :--- | :--- | :--- | :--- |
| `autostart_enabled` | 不进 `AppSettings` | **操作系统** | 新增 `cmd_get_autostart` / `cmd_set_autostart`，直读直写，无任何本地副本 |
| `start_minimized` | 进 `AppSettings` | SQLite | 沿用既有 `cmd_get_settings` / `cmd_save_settings` |

由此：

- OS 调用失败 **不会**阻断 server_url / PSK 的保存（GLM-01）；
- 保存无关配置 **不会**触碰 OS 自启项（AGY-02）；
- 没有本地副本就没有内存/SQLite/OS 三方同步问题（AGY-04）；
- G4 从「读时覆盖以缓解漂移」升级为「结构上不可能漂移」。

### 3.2 `start_minimized` 语义：冷启动全局生效（解决 AGY-01 / GLM-03）

两侧给出相反的分支建议，此处记录选择理由：

- 用户在本轮开工前已明确要求「再加一个独立开关」，意图是两个开关**自由组合**。
  若 `start_minimized` 仅在自启时生效，它就不是独立开关，而是 `autostart` 的子选项，
  违背确认过的需求。
- 后台托盘类工具（常驻型）的通用行为就是「配置了最小化启动，则任何冷启动都最小化」。
- 逃生口充分：托盘图标常驻，且**再次双击图标会经由 single-instance 强制唤起窗口**——
  这条逃生口比原方案更干净，因为它就是用户的自然动作。

新真值表（`--silent` 不再参与首次启动的显隐判定，只用于第二实例）：

| 场景 | 显示主窗口 |
| :--- | :--- |
| 冷启动（手动或自启），`start_minimized == false` | ✅ 显示 |
| 冷启动（手动或自启），`start_minimized == true` | ❌ 仅托盘 |
| 第二实例，启动参数**不含** `--silent` | ✅ 强制显示并聚焦（含 unminimize） |
| 第二实例，启动参数**含** `--silent` | ❌ 保持当前状态，不抢焦点 |

自启注册项仍带 `--silent`，含义收窄为「这是系统拉起的、不是用户主动点的」，
仅供第二实例判定是否抢焦点（GLM-04 覆盖的分支）。

### 3.3 新模块 `core/startup.rs`（修订签名）

```rust
/// 启动参数中是否含自启标记 --silent
pub fn is_autostart_launch<S: AsRef<str>>(args: &[S]) -> bool;

/// 冷启动时是否显示主窗口：只取决于用户配置
pub fn should_show_on_launch(start_minimized: bool) -> bool { !start_minimized }

/// 第二实例是否应抢焦点：系统拉起的不抢，用户点的要抢
pub fn should_focus_second_instance<S: AsRef<str>>(args: &[S]) -> bool {
    !is_autostart_launch(args)
}
```

`should_show_on_launch` 平凡但保留：它是真值表第 1、2 行的唯一落点，
让单测能锚定「手动启动也遵从配置」这条被 AGY-01 指出过一次的语义。

### 3.4 `cmd_set_autostart` 的幂等与比对基准（解决 AGY-02 / GLM-02）

```
1. current = manager.is_enabled()?        // 基准是 OS 实时值，不是旧内存值
2. if current == target { return Ok(()) } // 目标态已达成，不触碰 OS
3. if target { manager.enable()? } else { manager.disable()? }
4. 复查 manager.is_enabled()，与 target 不符则返回 Err
```

第 1 步用 OS 实时值而非前端提交的旧值，是 GLM-02 的核心诉求：
会话中途用户在系统设置里关掉自启后，应用内再点开关不会被陈旧状态误导。
第 4 步的复查确保「返回成功」等于「OS 里真的是这个状态」。

### 3.5 `second-instance` 回调（解决 AGY-03 / GLM-04）

```rust
.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
    if !core::startup::should_focus_second_instance(&args) {
        log::info!("Second instance is an autostart launch, keeping window state");
        return;
    }
    match app.get_webview_window("main") {
        Some(win) => {
            if win.is_minimized().unwrap_or(false) {
                let _ = win.unminimize();       // AGY-03：任务栏最小化态必须先还原
            }
            let _ = win.show();
            let _ = win.set_focus();
        }
        None => log::warn!("Second instance signalled but main window is gone"),
    }
}))
```

### 3.6 统一的窗口唤起助手（AGY-03 的连带修复）

`unminimize → show → set_focus` 这一串在四处重复（`second-instance`、托盘「显示主窗口」、
托盘「偏好设置...」、托盘左键、macOS `Reopen`）。原稿只在 `second-instance` 补，
会留下四处行为不一致。抽成 `lib.rs` 内的私有助手 `fn reveal_main_window(app: &AppHandle)`，
上述位置统一调用。

### 3.7 `AppSettings` 兼容性（不变，仍是最高风险项）

`start_minimized` 作为新字段必须带 `#[serde(default)]`，理由与原稿 §3.6 一致：
现有结构体无任何 default 属性，老库 JSON 反序列化失败会走 `lib.rs:35` 的
`unwrap_or_else` 静默全量回落，冲掉用户的 `server_url` / `account_id` / `psk_secret`。

`autostart_enabled` 已退出结构体，此处只剩一个新字段。

### 3.8 消除启动闪窗（不变）

`tauri.conf.json` 的 `"visible": true` 改为 `false`，显示统一由 setup 里的
`should_show_on_launch` 决定。托盘在 `lib.rs:333-386` 先于显隐判定构建，
「显示主窗口」菜单项是始终可用的逃生口。

---

## 4. 修订后的改动清单

### 4.1 Rust 侧（`client/src-tauri/`）

| 文件 | 改动 |
| :--- | :--- |
| `Cargo.toml` | 新增 `tauri-plugin-autostart = "2.0.0"` |
| `tauri.conf.json` | `app.windows[0].visible` → `false` |
| `capabilities/default.json` | **不改**（GLM-06：前端不直接调插件 JS API，Rust 侧 manager 调用无需该 capability） |
| `src/core/startup.rs` | **新建**：§3.3 三个纯函数 + `#[cfg(test)]` 覆盖真值表四行 |
| `src/core/mod.rs` | 导出 `pub mod startup;` |
| `src/commands/settings_cmd.rs` | ① `AppSettings` 加 `#[serde(default)] pub start_minimized: bool`；② `cmd_save_settings` **不做任何 autostart 相关改动**；③ 新增 `cmd_get_autostart` / `cmd_set_autostart`（§3.4）；④ 新增反序列化兼容单测（GLM-05） |
| `src/lib.rs` | ① 注册 `tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--silent"]))`；② `second-instance` 回调重写（§3.5）；③ 新增 `reveal_main_window` 助手并在五处调用点统一（§3.6）；④ 两处默认值字面量抽成 `AppSettings::default_config()` 并补新字段；⑤ L388-391 改为按 `should_show_on_launch(start_minimized)` 判定；⑥ `invoke_handler` 注册两个新 command |

### 4.2 前端侧（`client/src/`）

| 文件 | 改动 |
| :--- | :--- |
| `types/index.ts` | `AppSettings` 加 `start_minimized: boolean;`（**不加** `autostart_enabled`） |
| `App.tsx` | ① `defaultSettings` 补 `start_minimized: false`；② `handleSaveSettings` 失败时 `throw`（GLM-01，让弹窗能感知失败） |
| `components/SettingsModal.tsx` | ① `handleSubmit` 改为 `async`，`await onSave(...)` 成功才 `onClose()`，失败保持弹窗打开并就地显示错误（GLM-01）；② 「启动时最小化到托盘」走表单；③ 「开机自动启动」**独立于表单**：打开弹窗时 `invoke("cmd_get_autostart")` 拉 OS 实时状态，`onChange` 直接 `await invoke("cmd_set_autostart", { enabled })`，失败则回滚 UI 开关并就地提示 |

`package.json` 不加 `@tauri-apps/plugin-autostart`——前端只经 Rust command，
与 GLM-06 的裁决保持一致。

UI 文案（修订）：

- 「开机自动启动」/ 副标题「登录系统后自动运行，修改立即生效」
- 「启动时最小化到托盘」/ 副标题「启动后不弹出主窗口；点击托盘图标可随时唤起」
  （GLM-07：避开「只在托盘常驻」，macOS 上 Dock 图标仍在）

### 4.3 文档

- `docs/需求.md`：为第 1、2 条标注实现状态与落点。
- `docs/design/DESIGN.md:1282`：SettingsModal 描述与实际项对齐。

（原稿 §4.3 的 `.trivium/config.yaml` 改动已按 AGY-05 删除。）

---

## 5. 三端行为与风险（修订）

| 平台 | 自启机制 | 注意事项 |
| :--- | :--- | :--- |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` | 开发态注册的是 target 目录 exe 路径，安装后需重新开关一次；杀软可能对写 Run 键报警 |
| macOS | `MacosLauncher::LaunchAgent` → `~/Library/LaunchAgents/*.plist` | **GLM-07**：应用未设 `ActivationPolicy::Accessory`，默认 Regular 策略下即使隐藏窗口，Dock 图标仍在且可 cmd-tab 唤起。**本轮接受该现状**——引入 Accessory 会改变手动启动场景的行为，超出本轮范围。验收时 Dock 出现图标不算失败 |
| Linux | `~/.config/autostart/*.desktop` | AppImage 被移动后 `Exec` 路径失效；deb 安装路径稳定 |

风险：

- R1 双写冲突：已由「统一走应用内 API」规避（非目标）。
- R2 事实源漂移：**已消除**——`autostart_enabled` 无本地副本（§3.1）。
- R3 隐藏启动导致「以为没启动」：托盘图标 + tooltip 即反馈；双击图标必唤起（§3.2）。
- R4 窗口不可见的逃生口：托盘菜单先于显隐判定构建（§3.8）。
- R5 **新增**：`visible:false` 后若 `should_show_on_launch` 逻辑错误，首屏体验受损。
  由 §6.1 单测覆盖真值表四行锚定。

---

## 6. 修订后的验证方案

### 6.1 自动化（S10 出口 + 强制手动执行）

`trivium verify` 的 `verify.build` / `verify.test` 覆盖 `pnpm build` 与 server 侧
（按 AGY-05 裁决不修改该配置），因此 **Rust 侧两条必须在 S10 之前手动执行并如实报告结果**：

- `cd client/src-tauri && cargo check`
- `cd client/src-tauri && cargo test`，覆盖：
  - `core::startup` 真值表四行：冷启动 × `start_minimized` 真假；
    第二实例带 / 不带 `--silent`（GLM-04）；
  - 边界：空参数数组、`--silent` 前后夹带其他参数、大小写与近似参数不误判；
  - **`AppSettings` 反序列化兼容**（GLM-05）：用改动前格式的 JSON 字面量
    （无 `start_minimized` 字段）反序列化，断言 `server_url` / `account_id` /
    `psk_secret` / `auto_inject` / `rate_limit_mb` 全部保真，`start_minimized` 取 `false`。

出口命令（`trivium verify`）照常执行：`cd client && pnpm build`（TS 与 Rust 字段名
不一致会在此暴露）、`cd server && go build ./... && go test ./...`（确认未触碰服务端）。

### 6.2 手工回归

插件行为与 OS 交互无法单测，以下必须实机执行：

1. 全新环境启动：两项默认关，行为与改动前一致（窗口正常弹出）。
2. **老库兼容**：用改动前版本存一份非默认 `server_url` / PSK，再用新版本启动，
   确认配置未被冲掉（§3.7）。
3. 开启「开机自动启动」→ 检查 OS 注册项确实存在（Windows 查 Run 键 / macOS 查
   `~/Library/LaunchAgents` / Linux 查 `~/.config/autostart`）→ 重启系统 → 应用自动运行。
4. 关闭「开机自动启动」→ OS 注册项被清除。
5. 开启「启动时最小化」+ 关闭自启 → **手动**启动应用 → 主窗口不弹出，托盘图标在。
6. **5b**（GLM-03）：承接用例 5，双击图标 → 主窗口弹出并聚焦。
7. 开启自启 + 开启最小化 → 重启系统 → 进程在、托盘在、主窗口不弹。
8. 开启自启 + 关闭最小化 → 重启系统 → 主窗口正常弹出。
9. **幂等**（AGY-02）：开关保持不动，只改 PSK 后点保存 → OS 自启项的修改时间不变，
   macOS 上不出现后台项横幅。
10. **7b / 陈旧值**（GLM-02）：应用运行中，在系统设置里手动关掉自启 → 回应用打开设置弹窗
    → 开关显示为「关」；此时只改 PSK 并保存 → 自启**仍为关**（未被复活）。
11. **失败隔离**（GLM-01）：构造 `cmd_set_autostart` 失败（如 Linux 上把
    `~/.config/autostart` 置为只读）→ 自启开关回滚并就地提示，**同一弹窗内改的
    server_url / PSK 仍能正常保存**。
12. **保存失败不关窗**（GLM-01）：令 `cmd_save_settings` 失败 → 弹窗保持打开、显示错误，
    用户的输入不丢失。
13. **单实例**：应用运行中再次双击图标 → 不产生第二进程，已有窗口被唤起并聚焦。
14. **最小化态唤起**（AGY-03）：把主窗口最小化到任务栏（非隐藏到托盘），再次双击图标
    → 窗口从最小化状态还原并聚焦。Windows 上必测。
15. **`--silent` 第二实例**（GLM-04）：应用可见运行中，从终端以 `--silent` 再次启动
    可执行文件 → 不产生第二进程且窗口不抢焦点；应用隐藏时同样操作 → 保持隐藏。

---

## 7. 遗留（不变，登记备查）

- **L1** `lib.rs:317-330` 的 `start_clipboard_listener` 被 `#[cfg(target_os = "macos")]`
  包住，`platform/mod.rs:176-181` 三端都导出——Windows / Linux 的监听器是死代码。
  影响「自启常驻」的实际价值，建议另开一轮。
- **L2** `AppSettings.rate_limit_mb` 全仓库无消费点，是占位字段。
- **L3** `docs/需求.md` 第 3~8 条未实现。
- **L4**（新增，源自 AGY-05 的连带）`.trivium/config.yaml` 的 `verify` 不覆盖 Rust 侧，
  本轮按裁决不改，靠 §6.1 的强制手动执行与 `client-ci.yml` 兜底。若将来 Rust 侧
  测试增多，应独立成轮评估。
- **L5**（新增，源自 GLM-07）macOS `ActivationPolicy::Accessory` 未评估，
  「最小化启动」在 macOS 上仍有 Dock 图标。
