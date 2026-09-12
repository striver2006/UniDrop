---
schema: trivium.review.v1
topic: autostart-single-instance
stage: code
role: gemini
vendor: antigravity
agent_cli: agy/1.2.2
model_requested: gemini-3.8-flash-high
model_effective: gemini-3.8-flash-high
model_effective_source: envelope
effort: high
run_id: 20260912T173028Z
rerun_index: 1
snapshot_hash: sha256:21eabc71353e6784148345070a8a9b27735f7272578724f8cd0623c06c779568
blind: true
started_at: 2026-09-12T17:30:28.740Z
finished_at: 2026-09-12T17:39:27.783Z
duration_s: 224
tool_calls:
  read: 39
  grep: 7
  glob: 6
  run_command: 0
  project_reads: 22
evidence_ok: true
verdict: approve-with-nits
findings_count:
  blocker: 0
  major: 0
  minor: 0
  nit: 1
degraded: false
parse_fallback: false
peer_deny: false
drift: false
heterogeneous: true
session_id: d62b7a2f-ff37-4992-9541-1b2a2f3b84fb
---

# 代码审查：autostart-single-instance

> Critic-A · Google Antigravity ｜ 模型 `gemini-3.8-flash-high`
> ｜ 运行 `20260912T173028Z`

> ⚠️ **路径级 deny 未生效**：本机该侧 CLI 不支持路径级读取拒绝，对方历史产出的屏蔽仅依赖任务书禁读清单与事后轨迹核验。

## 总判断

本代码变更严格落实了已批准修订计划（plan §3-§4）中的全部设计，整体架构清晰稳健。
自启事实源与本地存储彻底解耦，幂等系统调用配合复查机制杜绝了状态漂移。
老库反序列化兼容性及四状态真值表均有完备单测护航，窗口还原与单实例交互逻辑健全。
仅在 SettingsModal 打开时初次拉取自启状态的异步期间存在一处微小的交互竞态（未置 loading 禁用态），属 nit 级别，不影响整体上线安全性。
判定为 approve-with-nits。

**结论**：`approve-with-nits`

## 审查意见（共 1 条：吹毛求疵 1）

### AGY-01 · 吹毛求疵（nit）

| 项 | 内容 |
| :--- | :--- |
| 位置 | `client/src/components/SettingsModal.tsx:29-40` |
| 类别 | concurrency ｜ 层次 code |
| 置信度 | medium |

**问题**：SettingsModal 打开时异步拉取 OS 自启状态期间未置 busy/loading 态，存在竞态覆盖与陈旧值闪烁隐患。

**依据**：在 client/src/components/SettingsModal.tsx:29-40 中，useEffect 在 isOpen 变为 true 时通过 invoke("cmd_get_autostart") 异步拉取操作系统自启状态，但在请求发出到完成期间并未设置 autostartBusy。组件初始状态 autostart 为 false，若系统实际已开启自启且读取稍有延迟，复选框在加载完成前保持可点击且显示为关闭；用户若在此窗口期快速点击复选框，不仅会触发与预期相反的写入，随后的读取结果 resolve 还会再次覆盖用户的最新操作。

**建议**：在拉取自启状态前将 autostartBusy 设为 true，在 Promise 的 finally 中恢复为 false，保证加载期间开关禁用且状态变更不可插队。

## 认为正确的部分

- 开机自启与本地持久化配置彻底解耦：自启状态以操作系统为唯一事实源（不进 AppSettings / 不落 SQLite），消除了内存、本地库与 OS 状态的三方漂移问题。
- 自启写操作具备严格幂等性与校验闭环：cmd_set_autostart 写入前比对 OS 实时值避免无意义重写，写入后复查状态保证返回成功即代表 OS 状态生效。
- 老版本配置反序列化兼容保护完备：AppSettings.start_minimized 添加 #[serde(default)] 并配套单元测试，有效防范老库升级时用户既有网络与密钥配置被静默冲掉。
- 多实例与窗口还原调用链统一：抽离 reveal_main_window 统一处理 unminimize、show、set_focus，第二实例回调正确识别 --silent 参数避免系统拉起时误抢焦点。
- 启动显隐策略抽成纯函数：core::startup 封装无副作用判定函数并提供完整的四状态真值表单测，tauri.conf.json 改为初始 visible:false 彻底消除冷启动闪窗。
- 设置弹窗错误处理闭环：保存配置失败时向外冒泡保持弹窗开启并显示错误，自启修改失败即时回滚 UI 开关。

## 未覆盖范围（本侧盲区）

- 未在真实的 Windows 10/11 实体机环境实测 HKCU\Software\Microsoft\Windows\CurrentVersion\Run 注册表键读写及从任务栏还原窗口的交互。
- 未在 Linux 桌面环境（GNOME / KDE）实测 ~/.config/autostart/*.desktop 文件读写与 AppImage 移动后路径的稳定性。
- 未在 macOS Ventura+ 实体系统上验证 LaunchAgent 注册后的系统设置通知及 Background Items 授权管理交互。
- 未对托盘后台长期常驻运行时的内存开销进行长时间压测。

## 实际查阅的项目文件

- `.github/workflows/build-release.yml`
- `.github/workflows/client-ci.yml`
- `.reviews/autostart-single-instance/code/_meta/changes-20260912T173028Z.diff`
- `.reviews/autostart-single-instance/plan/2026-09-13-autostart-single-instance-revised-claude.md`
- `AGENTS.md`
- `CLAUDE.md`
- `README.md`
- `client/package.json`
- `client/src-tauri/Cargo.lock`
- `client/src-tauri/Cargo.toml`
- `client/src-tauri/capabilities/default.json`
- `client/src-tauri/src/app_state.rs`
- `client/src-tauri/src/commands/mod.rs`
- `client/src-tauri/src/commands/settings_cmd.rs`
- `client/src-tauri/src/core/mod.rs`
- `client/src-tauri/src/core/startup.rs`
- `client/src-tauri/src/lib.rs`
- `client/src-tauri/src/storage/db.rs`
- `client/src-tauri/tauri.conf.json`
- `client/src/App.tsx`
- `client/src/components/SettingsModal.tsx`
- `client/src/types/index.ts`
- `docs/design/DESIGN.md`
- `docs/需求.md`

> 编排器从工具轨迹中记录到的读取次数：{"read":39,"grep":7,"glob":6,"run_command":0,"project_reads":22}

---

*本文档由 TriviumCode 编排器从 `gemini` 侧的结构化输出渲染而成。
审查员无写仓库权限，全部落盘由编排器完成。*
