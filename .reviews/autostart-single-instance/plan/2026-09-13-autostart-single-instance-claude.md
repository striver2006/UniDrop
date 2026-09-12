# 开机自启配置项 + 单实例加固 — 实施计划

- 主题：`autostart-single-instance`
- 阶段：plan（原稿）
- 日期：2026-09-13
- 角色：Driver (claude)
- 任务目标：增加配置项：是否开机启动；实现一台主机只能运行一个实例。

---

## 1. 背景与现状核验

需求来自 `docs/需求.md` 20260912 清单的第 1、2 条：

> 1. 一台主机只运行一个实例。
> 2. 可以设置开机启动。

起草前已对代码做过核验，结论与需求文本并不完全对应：

### 1.1 单实例 —— 已实现，本轮做加固

`client/src-tauri/Cargo.toml:20` 已依赖 `tauri-plugin-single-instance = "2.0.0"`，
并在 `client/src-tauri/src/lib.rs:94-100` 作为**第一个**插件注册（Tauri 官方要求的顺序）：

```rust
.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
    log::info!("Another instance attempted to start, focusing existing window");
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}))
```

「第二次启动 → 唤起已有窗口并退出新进程」的语义已经具备，全仓库没有手写
mutex / lock file / flock。因此本轮**不重做**单实例，只补两个真实缺口（见 §3）：

- 回调丢弃了 `_args`，自启场景下第二实例传来的 `--silent` 无从判别；
- 没有任何自动化测试覆盖启动语义。

### 1.2 开机自启 —— 完全未实现

全仓库检索 `autostart` / `LoginItem` / `CurrentVersion\Run` / `LaunchAgents`
在 Rust / TS / JSON / CI 中零命中：无 `tauri-plugin-autostart` 依赖、
无 `@tauri-apps/plugin-autostart`、`capabilities/default.json` 无对应权限、
`AppSettings` 无对应字段、`SettingsModal.tsx` 无对应控件。

仅在文档里是待办：`docs/design/DESIGN.md:1282` 规划了
「SettingsModal.tsx # 自启动、限流与 E2EE 偏好设置」，`DESIGN.md:1290`
规划了 Windows 用安装包写 `HKCU\...\Run`。

### 1.3 与自启强相关的既有行为

- 关闭窗口不退出、隐藏到托盘：`lib.rs:395-400`（`CloseRequested` → `hide()` + `prevent_close()`）
- 托盘菜单（显示主窗口 / 偏好设置... / 退出）与左键切换显隐：`lib.rs:333-386`
- **启动时无条件显示窗口**：`lib.rs:388-391`
- `tauri.conf.json:23` `"visible": true`
- macOS Dock 重开：`lib.rs:418-427`（`RunEvent::Reopen`）

即：常驻托盘的骨架齐全，缺的只是「自启注册」与「自启后不打扰」。

### 1.4 用户已确认的范围决策

1. 主题名用 `autostart-single-instance`（不用推导出的长名）。
2. 单实例：**保留插件，加固并补测**，不推倒重做为手写锁。
3. 自启形态：**拆成两个独立开关**——「开机自动启动」与「启动时最小化到托盘」，
   由用户自由组合，而不是把「静默」硬绑在自启上。

---

## 2. 目标与非目标

### 目标

- G1 新增配置项 `autostart_enabled`：开 / 关操作系统级别的开机自启注册项，三端可用。
- G2 新增配置项 `start_minimized`：启动时不弹主窗口，只在托盘常驻。
- G3 两个开关在 `SettingsModal` 中可见可改，保存后即时生效，重启后保持。
- G4 自启的**事实源是操作系统**，不是本地 SQLite，界面必须反映 OS 的真实状态。
- G5 单实例加固：`second-instance` 回调正确处理 `args`；启动显隐决策抽成纯函数并补单元测试。

### 非目标（本轮不做）

- 不做 Windows 安装包写注册表的方案（`DESIGN.md:1290`）——那会与应用内开关产生
  双写冲突，且卸载残留难清理。统一走应用内 API。
- 不做代码签名 / 公证（杀软对「剪贴板监听 + 自启 + 网络常驻」的误报风险已在
  `docs/reviews/2026-09-11-UniDrop设计审核-glm.md:150` 记录，属独立议题）。
- 不处理 `docs/需求.md` 的第 3~8 条。
- 不修复顺带发现的两个既有缺口（见 §7 遗留），避免污染本轮 diff。

---

## 3. 设计

### 3.1 事实源：OS 是权威，SQLite 只是缓存

自启状态真正存在于操作系统（Windows 注册表 `HKCU\...\Run`、macOS LaunchAgent plist、
Linux `~/.config/autostart/*.desktop`）。用户完全可能绕过本应用、在系统设置里
把自启关掉。如果只信 SQLite 里的 `autostart_enabled`，界面会显示一个假状态。

因此：

- **读**：`cmd_get_settings` 返回前，调用 `autostart_manager.is_enabled()` 取 OS 真实状态，
  覆盖从 SQLite 读出的 `autostart_enabled` 字段。OS 查询失败时保留 SQLite 的值并记日志，
  不让设置面板整体失败。
- **写**：`cmd_save_settings` 中按新值调用 `enable()` / `disable()`，**成功后**才把
  该字段写进 SQLite。OS 调用失败要向前端返回明确错误，不能静默吞掉——否则用户
  以为开了、实际没开。
- `start_minimized` 是纯本地行为，SQLite 即事实源，无此问题。

### 3.2 启动显隐的判定逻辑

`tauri-plugin-autostart` 的注册参数是 `init()` 时**固定**的，不能随设置动态变化。
所以不能用「注册时带不带 `--silent`」来表达 `start_minimized`。

约定：自启注册项**永远**带 `--silent`，它只表示「本次是开机自启拉起的」，
是否真的隐藏由 `start_minimized` 决定。

| 场景 | 启动参数含 `--silent` | `start_minimized` | 显示主窗口 |
|---|---|---|---|
| 用户手动启动 | 否 | 任意 | ✅ 显示 |
| 开机自启 | 是 | `false` | ✅ 显示 |
| 开机自启 | 是 | `true` | ❌ 只驻留托盘 |

手动启动永远显示窗口——用户刚双击了图标，意图明确，不该被配置吞掉。

### 3.3 新模块 `core/startup.rs`

把上表固化为两个**无副作用的纯函数**，这是本轮可单测的部分：

```rust
/// 判断本次启动是否由开机自启拉起（参数中含 --silent 标记）
pub fn is_autostart_launch<S: AsRef<str>>(args: &[S]) -> bool;

/// 依据启动参数与配置决定是否显示主窗口
pub fn should_show_window<S: AsRef<str>>(args: &[S], start_minimized: bool) -> bool {
    !is_autostart_launch(args) || !start_minimized
}
```

`lib.rs` 的 setup 与 `second-instance` 回调都调用同一个函数，避免两处逻辑漂移。

### 3.4 消除启动闪窗

`tauri.conf.json` 的 `"visible": true` 改为 `false`，窗口的显示统一由
`setup` 里的 `should_show_window` 决定。否则 `start_minimized` 场景会出现
「窗口先闪一下再被 hide」的观感缺陷。

风险兜底：窗口初始不可见后，若显示逻辑出错用户将无法唤起界面。托盘在
`setup` 中先于显隐判定构建（`lib.rs:333-386` 已在 388 行之前），
「显示主窗口」菜单项始终是可用的逃生口。

### 3.5 `second-instance` 回调加固

```rust
.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
    // 第二实例若同样带 --silent（例如登录时被拉起两次），不应打扰用户
    if core::startup::is_autostart_launch(&args) {
        log::info!("Second instance is an autostart launch, keeping window state");
        return;
    }
    log::info!("Another instance attempted to start, focusing existing window");
    match app.get_webview_window("main") {
        Some(win) => { let _ = win.show(); let _ = win.set_focus(); }
        None => log::warn!("Second instance signalled but main window is gone"),
    }
}))
```

窗口缺失从静默跳过改为告警日志——这是排查「进程还在但界面唤不出来」的唯一线索。

### 3.6 `AppSettings` 加字段的兼容性（必须处理的坑）

`commands/settings_cmd.rs:5-12` 的 `AppSettings` **当前没有任何 `#[serde(default)]`**。
设置整条以 JSON 字符串存在 SQLite `local_config` 表的 `key='app_settings'` 行里
（`storage/db.rs:122-133`）。直接加字段会导致老库里的旧 JSON 反序列化失败，
`lib.rs:35` 的 `unwrap_or_else` 会**静默回落到全部默认值**，把用户已配置的
`server_url` / `account_id` / `psk_secret` 一并冲掉——这是静默的用户数据损坏。

因此新字段必须带 `#[serde(default)]`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub server_url: String,
    pub account_id: String,
    pub psk_secret: String,
    pub auto_inject: bool,
    pub rate_limit_mb: u32,
    #[serde(default)]
    pub autostart_enabled: bool,
    #[serde(default)]
    pub start_minimized: bool,
}
```

两者默认 `false`：升级上来的老用户不会被莫名其妙地注册自启，符合最小惊讶。

---

## 4. 改动清单

### 4.1 Rust 侧（`client/src-tauri/`）

| 文件 | 改动 |
|---|---|
| `Cargo.toml` | 新增 `tauri-plugin-autostart = "2.0.0"`（与既有插件同段，L19-22 附近） |
| `capabilities/default.json` | `permissions` 数组追加 `"autostart:default"` |
| `tauri.conf.json` | `app.windows[0].visible` 由 `true` 改为 `false` |
| `src/core/startup.rs` | **新建**：`is_autostart_launch` / `should_show_window` + `#[cfg(test)]` 单测 |
| `src/core/mod.rs` | 导出 `pub mod startup;` |
| `src/commands/settings_cmd.rs` | `AppSettings` 加两个 `#[serde(default)]` 字段；`cmd_get_settings` 用 OS 状态覆盖 `autostart_enabled`；`cmd_save_settings` 调 `enable()`/`disable()` 并在失败时返回 `Err` |
| `src/lib.rs` | ① 注册 `tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--silent"]))`；② `second-instance` 回调改用 `args`（§3.5）；③ 两处默认值字面量（L37-43、L50-56）补齐新字段；④ L388-391 的无条件 `show()` 改为 `should_show_window(&std::env::args().collect::<Vec<_>>(), settings.start_minimized)` 判定 |

注：`lib.rs` 两处默认值字面量重复是既有问题，本轮顺手抽成一个
`AppSettings::default_config()` 关联函数，避免第三次漏改——这是本轮
必须触碰的同一处代码，不算范围蔓延。

### 4.2 前端侧（`client/src/`）

| 文件 | 改动 |
|---|---|
| `types/index.ts` | `AppSettings` 接口加 `autostart_enabled: boolean; start_minimized: boolean;` |
| `App.tsx` | `defaultSettings`（L22-28）补两个字段，均为 `false` |
| `components/SettingsModal.tsx` | 在 `auto_inject` 开关（L84-95）之后照其样式增加两个 checkbox 块；`start_minimized` 在 `autostart_enabled` 关闭时不置灰（手动启动也可能想最小化），但文案说明二者关系 |

`package.json` 不需要加 `@tauri-apps/plugin-autostart`——前端不直接调该插件的 JS API，
全部经由既有的 `cmd_get_settings` / `cmd_save_settings` 走 Rust 侧，
保持「设置只有一条读写路径」。

UI 文案（与既有中文风格一致）：

- 「开机自动启动」/ 副标题「登录系统后自动运行 瞬贴，保持设备在线」
- 「启动时最小化到托盘」/ 副标题「启动后不弹出主窗口，仅在系统托盘常驻」

### 4.3 流水线配置（`.trivium/config.yaml`）

当前 `verify.build` 为 `cd client && pnpm build` + `cd server && go build ./...`，
`verify.test` 只有 `cd server && go test ./...`。**Rust 侧完全不在出口校验范围内**——
本轮新增的 Rust 单测跑不到，Rust 编译错误也拦不住（`client-ci.yml` 虽然跑了
`cargo check` / `cargo test`，但那是 CI，不是本地出口）。

建议本轮一并补上：

- `verify.build` 追加 `cd client/src-tauri && cargo check`
- `verify.test` 追加 `cd client/src-tauri && cargo test`

此项改的是流水线配置而非业务代码，若审查认为应独立成轮，可驳回，本轮范围不受影响。

### 4.4 文档

- `docs/需求.md`：为第 1、2 条标注实现状态与落点（第 1 条注明原已实现、本轮加固）。
- `docs/design/DESIGN.md:1282`：把 SettingsModal 的描述与实际项对齐。

---

## 5. 三端行为与风险

| 平台 | 自启机制 | 需要注意 |
|---|---|---|
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` | 开发态注册的是 target 目录下的 exe 路径，安装后路径变化需重新开关一次；杀软可能对写 Run 键报警 |
| macOS | `MacosLauncher::LaunchAgent` → `~/Library/LaunchAgents/*.plist` | 必须用 LaunchAgent 而非 AppleScript LoginItem，后者在新系统上会弹权限提示 |
| Linux | `~/.config/autostart/*.desktop` | AppImage 场景下 `.desktop` 的 `Exec` 指向的 AppImage 路径若被移动则失效；deb 安装路径稳定 |

其他风险：

- **R1 双写冲突**：应用内开关与未来可能的安装包写注册表会互相覆盖。已在 §2 非目标中
  明确统一走应用内 API 规避。
- **R2 事实源漂移**：用户在系统设置里关掉自启后，SQLite 的值变旧。§3.1 的读时覆盖处理。
- **R3 隐藏启动导致「以为没启动」**：`start_minimized` 开启后无任何可见反馈。
  缓解：托盘图标本身即反馈，且 tooltip 已是「瞬贴 (UniDrop) - 跨平台剪贴板与文件分发」。
- **R4 窗口不可见的逃生口**：见 §3.4。

---

## 6. 验证方案

### 6.1 自动化

- `cd client/src-tauri && cargo test` —— `core::startup` 的单测覆盖 §3.2 真值表四行，
  含参数大小写 / 前后有其他参数 / 空参数数组等边界。
- `cd client/src-tauri && cargo check` —— 编译通过。
- `cd client && pnpm build` —— TS 类型对齐（`types/index.ts` 与 Rust 字段名必须一致，
  漏改会在此暴露）。
- `cd server && go build ./... && go test ./...` —— 确认本轮未触碰服务端。

### 6.2 手工回归（必须做，插件行为无法单测）

1. 全新环境启动：两个开关默认关，行为与改动前一致（窗口正常弹出）。
2. **老库兼容**：用改动前的版本保存一份非默认的 `server_url` / PSK，
   再用新版本启动，确认配置**没有被冲掉**（验证 §3.6）。
3. 开启「开机自动启动」→ 检查 OS 注册项确实存在（Windows 查 Run 键 /
   macOS 查 `~/Library/LaunchAgents` / Linux 查 `~/.config/autostart`）→ 重启系统 → 应用自动运行。
4. 同时开启「启动时最小化」→ 重启 → 进程在、托盘图标在、主窗口不弹。
5. 仅开「开机自启」不开「最小化」→ 重启 → 主窗口正常弹出。
6. 关闭「开机自动启动」→ OS 注册项被清除。
7. 在系统设置里手动关掉自启 → 回到应用设置面板 → 开关显示为「关」（验证 §3.1 读时覆盖）。
8. **单实例**：应用运行中再次双击图标 → 不产生第二进程，已有窗口被唤起并获得焦点。
9. 单实例 + 最小化组合：应用最小化在托盘时双击图标 → 窗口显示出来。

---

## 7. 遗留（本轮不处理，登记备查）

- **L1** `lib.rs:317-330` 的 `start_clipboard_listener` 被 `#[cfg(target_os = "macos")]`
  包住，而 `platform/mod.rs:176-181` 三端都导出了该函数——Windows / Linux 的监听器
  是死代码。与本轮无关，但影响「自启常驻」的实际价值，建议另开一轮。
- **L2** `AppSettings.rate_limit_mb` 全仓库无任何消费点，是占位字段。
- **L3** `docs/需求.md` 第 3~8 条未实现。
