# 真机验收记录：macOS 菜单栏图标与系统通知

- 主题：`macos-tray-notification`
- 角色：Driver（claude）
- 日期：2026-09-14
- 阶段：verify（人工验收，在 S10 编译测试通过之后）

## 为什么需要这份记录

S10 的 `TV verify` 只跑编译与测试，而本主题的两个症状都只在真实 macOS 桌面环境显现。
打包安装后的实测推翻了原计划 §1.1 的一处根因判断，并带出一处计划外的代码改动，
故单独留痕。

## 验收环境

- macOS 26.6.2（Darwin 25.6.0），单显示器 3440x1440，无刘海
- 包体：`pnpm tauri:build` 产出，Developer ID 签名 + **已公证**
  （`spctl` 判定 `source=Notarized Developer ID`）
- 安装位置：`/Applications/UniDrop.app`

## 发现一：`/Applications` 下的旧包并非正式构建产物

安装前对比签名：

| 包 | 代码签名 identifier | 签名类型 |
| :--- | :--- | :--- |
| 旧（2026-09-14 09:11） | `unidrop_client-7a6b90db6ae38e34` | `adhoc, linker-signed` |
| 新 | `com.unidrop.client` | Developer ID + hardened runtime |

旧包是 linker 自动 ad-hoc 签名的裸二进制被塞进 bundle，签名 identifier 与
`CFBundleIdentifier` 不一致。这种包本就拿不到通知授权。因此用户最初报告的
「通知不弹」是两个原因叠加：废弃的 NSUserNotificationCenter API，
以及一个签名身份不合法的包。

## 发现二：托盘图标的真实根因是**创建时机**，不是图标颜色

### 原判断与实测的出入

原计划 §1.1 判定为「彩色深色图标在深色菜单栏上与背景融为一体」。
换成模板图标后重新安装，**图标仍然完全不可见**。进一步排查证明原判断只对了一半：
模板图标确实是必需的（见下方验证），但它不是图标不可见的原因。

### 排查过程与证据

1. **状态项确实存在且功能完好**：通过辅助功能 API 点击它能正常弹出菜单，
   返回「显示主窗口 / 偏好设置... / 退出 瞬贴 (UniDrop)」——
   说明托盘创建、菜单绑定、事件处理全部正常；
2. **但它没有参与菜单栏布局**：坐标恒为 `(3405, -1)`，而同机其它状态项
   （clash-verge、TokenBar、Snipaste、TextInputMenuAgent 等）全部落在
   `2600~3060` 区间且 `y=3`。`3405 ≈ 3440 - 36`，是「屏幕最右减自身宽度」的
   兜底值，且正好被系统时钟盖住。重启 `ControlCenter` 后其它项重新排布，
   唯独它坐标不变；
3. **像素级确认未渲染**：截取该坐标区域做亮度分析，只有时钟数字
   （亮度 111~240），无任何图标图形，也没有接近纯黑的像素——
   排除了「黑图融进深色背景」；
4. **排除系统与图标文件**：用原生 `NSStatusItem` 加载**同一张** `tray-macos.png`
   做对照，稳定拿到正常位置 `x=2727`、`isVisible=true`。分别以
   `.accessory` 与 `.regular` 激活策略各测一次，结果一致；
5. **排除 tray-icon 的 API 用法**：在对照程序里逐步复现 tray-icon 的完整做法
   （图标 → 设置 menu → 往 button 上 addSubview 覆盖 frame 的自定义 view），
   三种形态全部正常，坐标依次 2727 / 2693 / 2659；
6. **排除其它变量**：单显示器（非多屏归属问题）、
   `com.unidrop.client` 的 UserDefaults domain 不存在（无隐藏偏好记录）、
   菜单栏空间充足、窗口可见性与之无关（显示主窗口后坐标不变）。

至此变量只剩创建时机。

### 根因

托盘原先在 `setup()` 中创建，而 `setup()` 跑在 `NSApplication` 完全就绪之前。
此时 `NSStatusBar::systemStatusBar().statusItemWithLength()` 能成功返回对象，
菜单与点击回调也都正常工作，但该状态项不会被纳入菜单栏的布局链，
只能拿到兜底坐标，于是肉眼不可见。

### 修复

新增 `build_tray()`，macOS 上改为在 `RunEvent::Ready` 中调用；
其余平台维持在 `setup()` 内创建，行为不变。

实测结果：坐标由 `(3405, -1)` 变为 `(2726, 3)`，与其它正常状态项一致；
截图确认图标可见，且在深色菜单栏上呈**白色**——证明模板图标同时也在正常工作
（纯黑源图由系统自动反色）。因此 §3.1 的模板图标改动仍然必要，
只是它单独不足以解决问题。

## 发现三：通知授权链路已打通

首次启动新包时日志为：

```
[WARN] Notification authorization request failed: Notifications are not allowed for this application
```

用户在系统设置中开启该应用通知后重启，日志变为：

```
[INFO] Notification authorization granted
```

这条从「静默失败」到「明确写出原因」再到「明确成功」的轨迹，
正是本次把四处 `let _ =` 改成记 warn 的直接价值——
在改动之前，这三种状态在外部看来是完全一样的（什么都不发生）。

## 对流程的说明

发现二对应的修复（`build_tray` + `RunEvent::Ready`）发生在 S10 之后，
**未经过本轮 code 双审**。它不在已批准的修订计划范围内，属于真机验收暴露的新缺陷。
建议对这部分改动单独补一次 `/dual-review-code`，或按 `/dual-review-bug` 处理，
由用户决定。在此之前不应认为该改动已获双审背书。

## 待完成的验收项

以下需要真实双设备场景，尚未执行：

- 从另一台设备分别发送文本 / 图片 / 文件，确认三类都弹出系统通知；
- 应用窗口处于前台时再发一次，确认通知仍显示（验证 `willPresent` 分支）；
- 点击通知横幅，确认主窗口被唤起（验证 `didReceive` 分支与主线程派发）；
- 左键点击托盘图标切换窗口显隐、右键弹出菜单。
