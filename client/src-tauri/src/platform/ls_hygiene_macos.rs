//! LaunchServices 死记录清理：macOS 26 控制中心「按 bundle id 拉黑状态项」的已知诱因治理。
//!
//! # 根因模型（为什么要有这个模块）
//!
//! macOS 26 (Tahoe) 起菜单栏项由控制中心进程托管。控制中心在为状态项建 host 时按
//! bundle id 查 LaunchServices，**只要撞上一条路径已不存在的陈旧注册记录（死记录），
//! 就把这个 bundle id 整个拉黑**（系统日志特征 `Moving host to blocked list`），
//! 表现为「应用正常运行、状态项创建成功，但菜单栏上永远看不到图标」。
//!
//! 2026-09-14 在本机用最小探针复现并钉死的事实（完整实验记录见
//! `TokenBar/doc/TROUBLESHOOTING_菜单栏图标不显示.md`，本模块是该项目
//! `MenuBarController` 自愈逻辑的 Rust 移植）：
//!
//! 1. 拉黑按 **bundle id** 精确命中：同 bundle id 的 /tmp 裸探针秒拒，
//!    全新 bundle id 的同款探针正常上屏；与应用代码、路径、签名、autosaveName 无关。
//! 2. 拉黑是**粘性会话态**：清光死记录后仍不解除，重启应用、重启控制中心、
//!    `tccutil reset` 全部无效；注销重登 / 重启是唯一已验证的解除手段。
//! 3. 死记录的常见来源：`cargo clean` / 删构建产物、换构建输出目录、反复挂 DMG
//!    装包。本应用 `target/release/bundle/macos/UniDrop.app` 被 LaunchServices
//!    注册着，删 target 目录就会留死记录——对开发机是高频操作。
//! 4. `lsregister -u <path>` 只认路径上真实存在的 bundle，死路径直接失败；
//!    唯一可行的注销方式是**原位重建最小 stub .app → `-u` → 删掉 stub**。
//!
//! # 本模块做什么、不做什么
//!
//! - 做：启动时清掉本 bundle id 名下的死记录。这保证「下一次控制中心评估」
//!   （下次启动 / 注销重登 / 重启之后）不会再被同一批死记录触发拉黑。
//! - 不做：解除**已经**拉上的黑。那是会话态，应用侧无手段（见上 2），
//!   用户侧的恢复路径（注销重登）写在 `docs/USER_GUIDE.md` 与前端横幅里。
//!
//! # 顺序约束（为什么线程要抢跑、托盘前要 join）
//!
//! 控制中心的评估发生在**状态项创建那一刻**。若死记录还挂着就去建托盘，
//! 本次会话直接进黑名单，之后再清理也救不回来。所以：
//! [`spawn`] 在 `run()` 的最开头启动（dump 全程实测 ~3.5s，与建库、建窗并行），
//! `build_tray` 之前由 [`CleanupHandle::finish_before_tray`] 有界等待收结果。
//!
//! 线程内**不打日志**：它开跑时 tauri-plugin-log 还没挂上（见 `run()` 里的
//! startup_log 注释），日志会在收结果的一侧（logger 已就位）统一打。

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 本应用的 bundle id。控制中心按它查 LaunchServices、也按它拉黑。
/// 必须与 `tauri.conf.json` 的 `identifier` 一致——有单测钉住，改要两边一起改。
pub const BUNDLE_IDENTIFIER: &str = "com.unidrop.client";

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// dump 看门狗。全量 dump 本机实测 ~3.5s / 30MB，15s 还没跑完按挂死处理——
/// 这条链路在 setup 里被有界等待，挂死不能拖住启动。
const DUMP_WATCHDOG: Duration = Duration::from_secs(15);

/// 一次核对的结果。只把要进日志的字段带回来，线程内不落日志（见模块头）。
#[derive(Debug)]
pub enum Report {
    /// lsregister 不存在或不可执行。查不了不能静默，否则会被误读成「核对过、没问题」。
    ToolUnavailable,
    /// dump 起不来 / 被看门狗杀掉 / 退出码非 0 / 输出不是 UTF-8。
    /// **刻意与「查成了、确实没有」区分**：工具一失败就伪装成无死记录，
    /// 日志就成了假阴性，这条排查线（工具是否可用）必须能事后倒查。
    DumpFailed,
    /// 核对完成。`stale == 0` 即干净。
    Checked {
        /// 发现的死记录条数（去重后）。
        stale: usize,
        /// 成功注销的条数。小于 `stale` 即有清不掉的，明细在 `uncleanable`。
        purged: usize,
        /// 清不掉的死记录路径（不可写路径，如已卸载的卷），留给人工处理。
        uncleanable: Vec<String>,
    },
}

/// 在后台线程上开跑一次死记录核对与注销。
///
/// 返回的句柄必须在 `build_tray` 之前调 [`CleanupHandle::finish_before_tray`]
/// 收结果——顺序约束见模块头。
pub fn spawn() -> CleanupHandle {
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    // 线程名给活动监视器/采样器看：这个线程会瞬时吃满一个核跑 dump，
    // 无名线程在排障时对不上号。
    std::thread::Builder::new()
        .name("ls-hygiene".to_string())
        .spawn(move || {
            let report = run_cleanup();
            // 接收端超时先走了就丢弃：清理本身仍会完成，只是结果不再上报。
            let _ = tx.send(report);
        })
        .expect("failed to spawn ls-hygiene thread");
    CleanupHandle { rx, started }
}

/// 清理线程的收结果句柄。
pub struct CleanupHandle {
    rx: mpsc::Receiver<Report>,
    started: Instant,
}

impl CleanupHandle {
    /// 等清理线程出结果并打日志。`deadline` 从 [`spawn`] 那一刻起算。
    ///
    /// 超时不等于失败：线程继续在后台跑完（为下次会话把 LS 清干净），
    /// 只是本次会话先按可能仍带死记录的状态建托盘，如实留痕。
    pub fn finish_before_tray(self, deadline: Duration) {
        let elapsed = self.started.elapsed();
        let remaining = deadline.saturating_sub(elapsed);
        match self.rx.recv_timeout(remaining) {
            Ok(report) => {
                log::info!(
                    "LaunchServices hygiene finished in {:?} (ran {}ms before this wait)",
                    elapsed,
                    elapsed.as_millis()
                );
                log_report(&report);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => log::error!(
                "LaunchServices 死记录清理[launch]：超时（>{}s）未出结果，先建托盘。\
                 本次会话可能仍被拉黑；清理线程会继续在后台完成，下次启动生效。",
                deadline.as_secs()
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                log::error!("LaunchServices 死记录清理[launch]：清理线程异常退出，本次未核对")
            }
        }
    }
}

/// 结果落日志。分级对齐 TokenBar 的语义：
/// 干净 → info；发现并处理 → error（这条日志就是「图标为什么消失过」的答案，
/// 必须 retention 期可见）；工具失败 → error。
fn log_report(report: &Report) {
    match report {
        Report::ToolUnavailable => {
            log::error!("LaunchServices 死记录清理[launch]：找不到可执行的 lsregister，跳过核对")
        }
        Report::DumpFailed => {
            log::error!("LaunchServices 死记录清理[launch]：dump 执行失败，跳过核对")
        }
        Report::Checked { stale: 0, .. } => {
            log::info!("LaunchServices 注册核对[launch]：无死记录")
        }
        Report::Checked {
            stale,
            purged,
            uncleanable,
        } => {
            log::error!("LaunchServices 死记录清理[launch]：发现 {stale} 条，注销 {purged} 条");
            if !uncleanable.is_empty() {
                log::error!(
                    "LaunchServices 死记录清理[launch]：{} 条无法自动注销\
                     （路径不可写，如已卸载的卷，需重新挂载对应卷或人工处理）：{}",
                    uncleanable.len(),
                    uncleanable.join(" | ")
                );
            }
        }
    }
}

fn run_cleanup() -> Report {
    if !Path::new(LSREGISTER).is_file() {
        return Report::ToolUnavailable;
    }
    let Some(dump) = dump_with_watchdog() else {
        return Report::DumpFailed;
    };
    let stale = stale_paths_in_dump(&dump, BUNDLE_IDENTIFIER, |p| Path::new(p).exists());
    let mut purged = 0;
    let mut uncleanable = Vec::new();
    for path in &stale {
        if unregister_record(path, BUNDLE_IDENTIFIER) {
            purged += 1;
        } else {
            uncleanable.push(path.clone());
        }
    }
    Report::Checked {
        stale: stale.len(),
        purged,
        uncleanable,
    }
}

/// 跑 `lsregister -dump`，带看门狗。返回 `None` 表示没查成（见 `Report::DumpFailed`）。
fn dump_with_watchdog() -> Option<String> {
    let mut child = Command::new(LSREGISTER)
        .arg("-dump")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // 读端必须独立线程排空：几十 MB 输出远超管道缓冲，不排空子进程会写阻塞，
    // 主循环的 try_wait 永远等不到退出，看门狗形同虚设。
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        stdout.read_to_string(&mut buf).ok().map(|_| buf)
    });

    let deadline = Instant::now() + DUMP_WATCHDOG;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            // 到期仍在跑，或 wait 本身报错：杀掉按挂死处理。
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    reader.join().ok().flatten()
}

/// `lsregister -dump` 的纯文本解析，抽出来便于单测。
///
/// 记录之间用整行 `-`（≥20 个）分隔；每条记录里 `identifier:` 与 `path:` 各占一行
/// （先后顺序不定，本机实测 path 在前）、值前有对齐空白，path 末尾带 ` (0x…)` 序号。
/// 只认本 bundle id 且路径已不存在的记录。路径存在性由 `path_exists` 注入，
/// 这样解析本身可以对任意 fixture 做确定性单测。
pub(crate) fn stale_paths_in_dump(
    dump: &str,
    bundle_id: &str,
    path_exists: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut stale: Vec<String> = Vec::new();
    let mut identifier: Option<String> = None;
    let mut path: Option<String> = None;

    fn flush(
        identifier: &Option<String>,
        path: &Option<String>,
        bundle_id: &str,
        path_exists: &dyn Fn(&str) -> bool,
        stale: &mut Vec<String>,
    ) {
        if identifier.as_deref() != Some(bundle_id) {
            return;
        }
        if let Some(p) = path {
            if !p.is_empty() && !path_exists(p) {
                stale.push(p.clone());
            }
        }
    }

    for raw in dump.split('\n') {
        let line = raw.trim();
        if line.len() >= 20 && line.chars().all(|c| c == '-') {
            flush(&identifier, &path, bundle_id, &path_exists, &mut stale);
            identifier = None;
            path = None;
        } else if let Some(v) = line.strip_prefix("identifier:") {
            identifier = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("path:") {
            let mut value = v.trim();
            // 剥掉尾部的 " (0x…)" 注册序号。只有以 ')' 结尾且能从后往前找到
            // " (0x" 时才剥，真实路径里含 " (" 的（如 "My App (1).app"）不受影响。
            if value.ends_with(')') {
                if let Some(pos) = value.rfind(" (0x") {
                    value = &value[..pos];
                }
            }
            path = Some(value.to_string());
        }
    }
    flush(&identifier, &path, bundle_id, &path_exists, &mut stale);

    // 同一路径可能注册出多条记录，重复注销没有意义。
    stale.sort();
    stale.dedup();
    stale
}

fn run_lsregister(args: &[&str]) -> bool {
    Command::new(LSREGISTER)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 注销一条本 bundle id 的死记录。
///
/// 快路径：路径上有真实 bundle（或记录已被别处清掉）时 `-u` 直接生效。
/// 死路径走 stub 法：原位重建最小 .app → `-u` → 删掉 stub；
/// 期间新建的缺失祖先目录按 rmdir 语义逐级回收（只删空目录，中途落进别的
/// 文件就留着），绝不动任何原本就存在的目录。不可写路径（如已卸载的卷）
/// 会失败，由调用方如实上报留给人工处理。
fn unregister_record(path: &str, bundle_id: &str) -> bool {
    if run_lsregister(&["-u", path]) {
        return true;
    }
    // 双保险：解析时该路径确实不存在；若此刻又出现了（比如用户刚在旧路径
    // 重建应用），那已经不是死记录，绝不能拿 stub 去动它。
    if Path::new(path).exists() {
        return false;
    }

    // 记下原本不存在的祖先目录链，注销后逐级回收。
    let mut missing_ancestors: Vec<std::path::PathBuf> = Vec::new();
    for dir in Path::new(path).ancestors().skip(1) {
        if dir.as_os_str().is_empty() || dir.exists() {
            break;
        }
        missing_ancestors.push(dir.to_path_buf());
    }

    let contents = format!("{path}/Contents");
    let macos_dir = format!("{contents}/MacOS");
    let created = std::fs::create_dir_all(&macos_dir).is_ok()
        && std::fs::write(format!("{contents}/Info.plist"), stub_info_plist(bundle_id)).is_ok()
        && std::fs::write(format!("{macos_dir}/LSTombstone"), b"").is_ok();
    // -u 成败都要把 stub 删掉；建了一半失败同样回收（此刻 path 下只可能有
    // 本次调用刚建的半成品，路径原本不存在是上面双保险保证过的）。
    let unregistered = created && run_lsregister(&["-u", path]);
    if created {
        let _ = std::fs::remove_dir_all(path);
    }
    for dir in missing_ancestors.iter().rev() {
        let _ = std::fs::remove_dir(dir);
    }
    unregistered
}

/// 注销死记录用的最小 stub Info.plist。lsregister 只要求路径上能扫描出一个
/// 合法 bundle；CFBundleIdentifier 必须与要注销的记录一致，否则 `-u` 匹配不上。
fn stub_info_plist(bundle_id: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleName</key>
    <string>LSTombstone</string>
    <key>CFBundleExecutable</key>
    <string>LSTombstone</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与本机真实 dump 逐字段对齐的最小 fixture（2026-09-14 实测格式）：
    /// 记录由整行 80 个 `-` 分隔，`bundle id:`（显示名）在 path 之前，
    /// 真正的标识在 `identifier:` 行，path 尾部带 ` (0x…)` 序号。
    const REALISTIC_DUMP: &str =
        "--------------------------------------------------------------------------------
bundle id:                  UniDrop (0x31fc)
path:                       /Applications/UniDrop.app (0x475c)
name:                       UniDrop
identifier:                 com.unidrop.client
codeInfoID:                 unidrop_client-2cb1438087d4e5b9
executable:                 Contents/MacOS/unidrop-client
--------------------------------------------------------------------------------
bundle id:                  Updater (0x1b00)
Bundle node not found on disk: Error Domain=NSOSStatusErrorDomain Code=-43
container:                  / (0x4)
--------------------------------------------------------------------------------
bundle id:                  SomeOtherApp (0x2a)
path:                       /Applications/Other.app (0x9f)
identifier:                 com.other.app
--------------------------------------------------------------------------------
bundle id:                  UniDrop (0x321c)
path:                       /private/tmp/dead/UniDrop.app (0x1234)
identifier:                 com.unidrop.client
--------------------------------------------------------------------------------
bundle id:                  UniDrop Dead No Path (0x44)
identifier:                 com.unidrop.client
--------------------------------------------------------------------------------
";

    #[test]
    fn finds_only_dead_records_of_own_bundle_id() {
        let stale = stale_paths_in_dump(REALISTIC_DUMP, "com.unidrop.client", |p| {
            // 注入的存在性：/Applications 下视为存在，其余视为已删除
            !p.starts_with("/private/tmp/")
        });
        assert_eq!(stale, vec!["/private/tmp/dead/UniDrop.app".to_string()]);
    }

    #[test]
    fn clean_registry_yields_empty() {
        let stale = stale_paths_in_dump(REALISTIC_DUMP, "com.unidrop.client", |_| true);
        assert!(stale.is_empty());
    }

    #[test]
    fn duplicate_dead_paths_are_deduped() {
        let dump =
            "--------------------------------------------------------------------------------
path:                       /tmp/x/UniDrop.app (0x1)
identifier:                 com.unidrop.client
--------------------------------------------------------------------------------
path:                       /tmp/x/UniDrop.app (0x2)
identifier:                 com.unidrop.client
--------------------------------------------------------------------------------
";
        let stale = stale_paths_in_dump(dump, "com.unidrop.client", |_| false);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0], "/tmp/x/UniDrop.app");
    }

    #[test]
    fn path_without_hex_suffix_and_with_parens_in_name_survives() {
        let dump =
            "--------------------------------------------------------------------------------
path:                       /tmp/My App (1).app
identifier:                 com.unidrop.client
--------------------------------------------------------------------------------
";
        let stale = stale_paths_in_dump(dump, "com.unidrop.client", |_| false);
        // 没剥任何东西：名字里的 " (1)" 不带 0x 前缀也不以 ") 序号" 形态结尾
        assert_eq!(stale, vec!["/tmp/My App (1).app".to_string()]);
    }

    #[test]
    fn short_dash_lines_are_not_separators() {
        // 5 个 '-' 不构成分隔符：字段会跨越这行继续累积，后写的覆盖先写的，
        // 两条记录并作一条、以最后的 identifier 定归属。
        let dump = "-----
path:                       /tmp/dead/UniDrop.app (0x1)
identifier:                 com.unidrop.client
-----
path:                       /tmp/other.app (0x2)
identifier:                 com.someone.else
";
        let stale = stale_paths_in_dump(dump, "com.someone.else", |_| false);
        assert_eq!(stale, vec!["/tmp/other.app".to_string()]);
        // com.unidrop.client 的字段已被覆盖，归属不到它名下
        assert!(stale_paths_in_dump(dump, "com.unidrop.client", |_| false).is_empty());
    }

    #[test]
    fn empty_and_garbage_input_is_safe() {
        assert!(stale_paths_in_dump("", BUNDLE_IDENTIFIER, |_| false).is_empty());
        assert!(
            stale_paths_in_dump("random\nnot a dump\nat all", BUNDLE_IDENTIFIER, |_| false)
                .is_empty()
        );
    }

    /// bundle id 是整套机制的键：控制中心按它拉黑、lsregister 按它匹配注销。
    /// 钉住常量与 tauri.conf.json 的一致性，防止改名只改一边。
    #[test]
    fn bundle_identifier_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(
            conf["identifier"]
                .as_str()
                .expect("identifier in tauri.conf.json"),
            BUNDLE_IDENTIFIER
        );
    }

    /// stub 的 Info.plist 里 CFBundleIdentifier 必须与目标记录一致，
    /// 否则 `-u` 匹配不上——把这条契约钉进测试。
    #[test]
    fn stub_plist_carries_matching_bundle_id() {
        let plist = stub_info_plist("com.unidrop.client");
        assert!(plist.contains("<string>com.unidrop.client</string>"));
        assert!(plist.contains("<key>CFBundlePackageType</key>"));
        assert!(plist.contains("<string>APPL</string>"));
    }

    /// 端到端自测：真实注册一条 `com.unidrop.client` 的记录 → 删掉制造死记录 →
    /// 跑完整清理（dump + 解析 + stub 注销）→ 验证记录消失、目录被回收。
    ///
    /// 会真实读写本机 LaunchServices 数据库、跑两次全量 dump（~7s），
    /// 只覆盖本模块自己的路径、不碰 /Applications 等真实注册，
    /// 供手动验证与回归时用 `cargo test -- --ignored` 执行。
    #[test]
    #[ignore = "真实读写本机 LaunchServices，仅手动验证时运行"]
    fn end_to_end_stub_unregister_removes_dead_record() {
        let dir = std::env::temp_dir().join("unidrop-ls-hygiene-selftest");
        let app_path = dir.join("StubApp.app");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(format!("{}/Contents/MacOS", app_path.display())).unwrap();
        std::fs::write(
            format!("{}/Contents/Info.plist", app_path.display()),
            stub_info_plist(BUNDLE_IDENTIFIER),
        )
        .unwrap();
        std::fs::write(
            format!("{}/Contents/MacOS/LSTombstone", app_path.display()),
            b"",
        )
        .unwrap();
        // LaunchServices 记录的是规范化路径（macOS 上 /var/folders → /private/var/folders），
        // 注册、断言、目录回收判断都必须对齐到它，否则字符串对不上会假失败。
        let app_path = std::fs::canonicalize(&app_path).unwrap();
        let app_path = app_path.to_str().unwrap();
        assert!(run_lsregister(&["-f", app_path]), "注册 stub 失败，测试无法进行");

        // 注册生效的证据：dump 能看到该路径（用恒假的 exists 让它必然算作 stale）
        let dump = dump_with_watchdog().expect("dump 失败");
        assert!(
            stale_paths_in_dump(&dump, BUNDLE_IDENTIFIER, |_| false)
                .iter()
                .any(|p| p == app_path),
            "刚注册的路径没出现在 dump 里"
        );
        drop(dump);

        // 删掉实体 → 死记录成型；这正是“cargo clean / 删构建产物”在现场的形态
        std::fs::remove_dir_all(&dir).unwrap();
        let report = run_cleanup();
        match report {
            Report::Checked {
                stale,
                purged,
                uncleanable,
            } => {
                assert!(stale >= 1, "应至少发现刚制造的这条死记录");
                assert!(purged >= 1, "stub 法应成功注销");
                assert!(
                    uncleanable.iter().all(|p| !p.starts_with(app_path)),
                    "测试路径不应出现在清不掉清单：{uncleanable:?}"
                );
            }
            other => panic!("清理未正常执行：{other:?}"),
        }

        // 终态：记录没了、临时目录链也回收了
        let dump = dump_with_watchdog().expect("复核 dump 失败");
        assert!(
            !stale_paths_in_dump(&dump, BUNDLE_IDENTIFIER, |p| Path::new(p).exists())
                .iter()
                .any(|p| p == app_path),
            "死记录在清理后仍然存在"
        );
        assert!(!dir.exists(), "祖先目录链未被回收");

        // 顺带覆盖 GUI 侧的收结果路径：不起 GUI，只验证 spawn→finish 不挂不死。
        spawn().finish_before_tray(Duration::from_secs(20));
    }
}
