---
schema: trivium.disposition.v1
topic: macos-tray-notification
stage: code
role: claude
kind: response
run_id: 20260914T051622Z
addresses:
  - AGY-01
  - AGY-02
  - GLM-01
  - GLM-02
  - GLM-03
---

# 代码审查应答（第 2 轮）：macOS 菜单栏图标与系统通知修复

- 主题：`macos-tray-notification`
- 阶段：code（第 2 轮，应答确认，含逐条裁决）
- 日期：2026-09-14
- 被审对象：`git diff af5eae4`（HEAD = 3249004），11 个文件，补丁 39.8 KB
- 两侧结论：Antigravity `request-changes`（重要 1 ｜ 吹毛求疵 1）；ZCode `request-changes`（重要 1 ｜ 次要 1 ｜ 吹毛求疵 1）

## 本轮缘由

第 1 轮 code 双审之后，真机验收暴露了新问题，我在验收期改动了托盘创建时机。
该改动未经双审即随代码提交，故补跑本轮。两侧独立命中了同一处严重缺陷
（AGY-01 / GLM-01），确认这次补审是必要的。

## 裁决表

共 5 条，**接受 5 条、驳回 0 条、暂缓 0 条**。

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | major | 接受 | 属实，是我的疏漏且后果严重。我在验收后期逐一试过「setup 创建」「Ready 创建」「Ready + 800ms」「Ready + 3000ms」「创建后 set_visible 重建」五种形态，生产构建下坐标**全部**是 (3405,-1)；唯一得到 (2726,3) 的是不带 custom-protocol 的构建，而那一侧 webview 连的是根本没启动的 dev server、窗口加载失败，属异常环境，不能作为参照。据此时序竞态假设已被证伪。清理实验代码时我只删了 sleep 与重建逻辑，**注释整段留在了原地**，于是形成「注释详述 800ms 经验值、代码零等待直接调用」的文实割裂，并与 build_tray 文档的权限归因正面冲突。审查员据此判定「无法证明提交代码经过了它自己描述的那种验证」，这个判断是对的。 | 见下方「因果口径裁决」。删除 Ready 分支全部竞态叙事与 800ms 段落；并采纳该条建议，把 build_tray 收拢回 `setup()` 全平台统一创建 |
| GLM-01 | glm | major | 接受 | 与 AGY-01 同一处，两侧独立命中，合并处理。GLM 额外给出了关键约束：两处注释对同一症状给出互斥成因，「后来者无从取舍」——这正是我要根除的那类哑坑。它要求 Driver 裁决出唯一因果口径，并给了两条对称修法（实现等待 / 删除竞态叙事）。我选后者，依据见 AGY-01 理由栏的五种形态实测。另采纳其提醒：若保留等待，也不该在 run 回调里阻塞主线程——这进一步说明维持一个无依据的等待没有价值。 | 同 AGY-01 |
| AGY-02 | gemini | nit | 接受 | 属实。我插入 build_tray 时把它放进了 reveal_main_window 的文档注释中间，只替换了末行，前三行留在原处，导致 build_tray 的 rustdoc 以「唤起主窗口的唯一入口」开头——读文档的人会以为 build_tray 负责唤起窗口。 | `lib.rs` 把该段文本移回 reveal_main_window，build_tray 文档从「构建菜单栏 / 任务栏托盘图标」起头 |
| GLM-02 | glm | minor | 接受 | 与 AGY-02 同一处，合并处理。GLM 多指出一层我没注意到的损失：reveal_main_window 现存文档只剩调用方清单，「隐藏到托盘 vs 最小化到任务栏是两种形态、必须先 unminimize」这段**行为依据**被搬走后丢失了。那段解释的是一个 Windows 上踩过的坑，丢掉比错位更可惜。 | 同 AGY-02，且确保搬回时把行为依据完整还给 reveal_main_window |
| GLM-03 | glm | nit | 接受 | 核实属实：产物在开发机（macOS）生成，而 `--check` 跑在 `ubuntu-latest`（client-ci.yml:17），字节全等比较依赖两边 zlib 对同一像素数据产出相同位流。这一层一旦不成立，CI 会永久红，且失败文案「脚本改了没重跑，或产物被直接编辑」把人指向错误方向——在开发机重跑也修不好。提交者自标 low（未实测跨平台位流），但修复成本极低而失败模式很坏，按接受处理。 | `gen-tray-icon.py` 的 `--check` 改为解码后逐像素（含 alpha）比对，对 zlib 实现差异免疫；三项结构不变量保留不变 |

## 因果口径裁决（对 AGY-01 / GLM-01 的统一答复）

两位审查员都要求给出唯一口径。裁决如下：

**菜单栏图标不可见的真实成因是 macOS 26 新增的菜单栏权限控制，与创建时机无关。**

依据分三层：

1. **穷举实测**：生产构建下，托盘创建放在 setup、Ready、Ready+800ms、Ready+3000ms、
   以及创建后 `set_visible(false)→(true)` 强制重建，坐标全部为 (3405,-1)。
   若属时序竞态，加大延迟应当收敛，实际没有。
2. **对照实验**：同一时刻、同一台机器上，由另一进程创建的原生 NSStatusItem
   稳定拿到 (2727) 正常位置，且加载的正是本项目这张模板图。
   即菜单栏本身与图标文件都没有问题，问题只出在本应用进程自己的状态项上。
3. **上游结论**：tauri-apps/tauri#13770 中维护者的判断是
   「新权限机制通常绑定到 app bundle」「app bundle 还必须是签过名的」。
   这与实测现象（状态项创建成功、菜单与点击均正常、但系统不分配布局位置）吻合。

至于那个一度误导我的 (2726,3)：它来自不带 custom-protocol 的构建，
该构建下 webview 去连未启动的 dev server、窗口加载失败，应用没有被当作正常 GUI
应用，反而绕过了这道检查。**它是假象，不是修复生效。** 我当时据此宣称问题已修好，
是本次工作中最实质的一处判断失误，此处一并记下。

GLM 在盲区里写明「两个成因哪个为真无法从仓库判定，裁决权在 Driver」——
这个边界划得准确：上述依据 2、3 都来自仓库之外的真机观测与上游 issue，
审查员确实无从核实。

**托盘创建位置一并收拢回 `setup()`。** 引入 Ready 分支的唯一动机是修图标不可见，
该动机既已证伪，就不该留下一个没有依据的跨平台分叉。这同时消化掉 AGY-01 指出的
两处派生问题：平台间生命周期不对称、以及冷启动最小化时「窗口已隐藏而托盘尚未创建」
的时序倒挂。build_tray 文档中关于 macOS 26 权限的说明**保留**——
它是有价值的排查指引，只是不再与任何竞态叙事共存。

## 两位审查员一致确认无误的部分

本轮审查焦点之二（通知封装最终形态）两侧均核实通过，记此备查：
delegate 以 `OnceLock<Retained>` 进程级长期持有（匹配 `setDelegate` 的 weak 语义）、
`willPresent` 与 `didReceive` 的 completionHandler 均在当前帧同步调用且全路径恰好一次、
NSWindow 系操作全部经 `run_on_main_thread` 派发且闭包只捕获 `'static` 的 AppHandle。
上一轮两条意见的修复（`minimumSystemVersion=11.0`、图标 `--check` 挂进 CI）
也经 GLM 本机实测确认落地。

## S9 待改动清单（全部为已接受项）

1. `client/src-tauri/src/lib.rs`：删除 `RunEvent::Ready` 分支中的托盘创建与全部竞态注释，
   该分支恢复为仅处理 `Reopen`；
2. `client/src-tauri/src/lib.rs`：`build_tray` 调用收拢回 `setup()`，去掉 `#[cfg]` 分叉，
   全平台统一；
3. `client/src-tauri/src/lib.rs`：修正 `build_tray` 与 `reveal_main_window` 的文档注释错位，
   把行为依据完整还给后者；
4. `client/scripts/gen-tray-icon.py`：`--check` 由字节全等改为解码后逐像素比对。
