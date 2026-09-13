use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use rusqlite::Connection;
use tokio::sync::Mutex;

use crate::core::retention::{RetentionPolicy, CLIPBOARD_LOCK_SECS};

#[derive(Clone)]
pub struct CacheManager {
    cache_root: PathBuf,
    db_conn: Arc<Mutex<Connection>>,
}

impl CacheManager {
    pub fn new(db_conn: Arc<Mutex<Connection>>) -> Result<Self, String> {
        let cache_root = Self::resolve_default_cache_dir();
        fs::create_dir_all(&cache_root).map_err(|e| format!("failed to create cache dir: {}", e))?;

        Ok(Self {
            cache_root,
            db_conn,
        })
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// Resolves standard platform cache directory.
    pub fn resolve_default_cache_dir() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                return PathBuf::from(local_app_data).join("UniDrop").join("cache");
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join("Library").join("Caches").join("UniDrop").join("cache");
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
                return PathBuf::from(xdg).join("unidrop").join("cache");
            } else if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join(".cache").join("unidrop").join("cache");
            }
        }

        std::env::temp_dir().join("unidrop").join("cache")
    }

    /// Marks a session's files as active in clipboard (initiates 2-hour immunity lock).
    pub async fn mark_clipboard_injected(&self, session_id: &str) -> Result<(), String> {
        let conn = self.db_conn.lock().await;
        conn.execute(
            "UPDATE cache_entries SET clipboard_injected_at = CURRENT_TIMESTAMP WHERE session_id = ?1",
            [session_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Returns all file paths registered for a given session_id.
    pub async fn get_session_files(&self, session_id: &str) -> Result<Vec<PathBuf>, String> {
        let conn = self.db_conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT file_path FROM cache_entries WHERE session_id = ?1")
            .map_err(|e| e.to_string())?;
        let paths: Vec<PathBuf> = stmt
            .query_map([session_id], |row| {
                let s: String = row.get(0)?;
                Ok(PathBuf::from(s))
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        Ok(paths)
    }

    /// Registers a newly written file entry in the cache database.
    pub async fn register_entry(&self, file_path: &Path, session_id: &str, file_size: i64) -> Result<(), String> {
        let path_str = file_path.to_str().ok_or("invalid UTF-8 in file path")?;
        let conn = self.db_conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO cache_entries (file_path, session_id, file_size, created_at, last_accessed_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            (path_str, session_id, file_size),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 按「最新 N 条」修剪传输历史，返回实际删除的条数（需求 4）。
    ///
    /// 分三段执行，**磁盘 IO 段不持有 `db_conn` 锁**：
    ///
    /// 1. 选取（持锁）：`select_prune_candidates` 取出可删会话及其文件路径，随即释放锁；
    /// 2. 删文件（不持锁）：逐个 `remove_file`。同步文件 IO 若在持锁状态下做，
    ///    会把整条信令链路一起堵住；
    /// 3. 删行（重新取锁，单事务）。
    ///
    /// **先删文件后删行**，与 `sweep` 同序。反过来先删行的话，
    /// 一旦 `remove_file` 失败，文件就脱离了 `cache_entries` 索引，而全部清理逻辑
    /// 都以表行为遍历源 ⇒ 永远扫不到它，成为不可回收的磁盘孤儿。
    /// 反向留下的「行在、文件没了」则可自愈：下轮修剪会重试，且另存为/打开位置
    /// 本就处理了文件缺失。
    /// 注意本函数带账号而紧邻的 `sweep` 不带，这不是疏漏：
    /// 条数上限是「界面列表保留多少条」，全局语义下一个账号狂传就会把另一个
    /// 账号的历史挤光；而 `sweep` 管的 TTL 与容量约束的是磁盘——物理共享资源，
    /// 按账号各分一份配额就是超卖。两者作用域不同是刻意的，别「统一一下」。
    pub async fn prune_history(&self, account_id: &str, limit: u32) -> Result<usize, String> {
        // 1. 选取段：取完即释放锁
        let candidates = {
            let conn = self.db_conn.lock().await;
            crate::storage::HistoryRepo::select_prune_candidates(&conn, account_id, limit)
                .map_err(|e| e.to_string())?
        };
        if candidates.is_empty() {
            return Ok(0);
        }

        // 2. 删文件段：不持锁。单个文件删不掉就把整个会话留到下轮重试——
        //    保留行比留下无人回收的孤儿文件安全。
        let mut deletable = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let mut all_removed = true;
            for path_str in &candidate.file_paths {
                let p = PathBuf::from(path_str);
                if !p.exists() {
                    continue;
                }
                if let Err(e) = fs::remove_file(&p) {
                    log::warn!(
                        "Failed to remove cached file {} while pruning history: {}",
                        p.display(),
                        e
                    );
                    all_removed = false;
                }
            }
            if all_removed {
                deletable.push(candidate.session_id);
            }
        }
        if deletable.is_empty() {
            return Ok(0);
        }

        // 3. 删行段：重新取锁，先复核免疫状态再删行。
        //
        //    复核是必需的：第 2 段没持锁，用户可能正好在那段时间点了「装载」，
        //    免疫标记落在选取之后，选取段的判据已经算完了。
        //    但要清楚它**只挡得住行被删**——文件在第 2 段已经删掉，救不回来。
        //    复核与删行处在同一段连续持锁区间内，`mark_clipboard_injected` 要的是
        //    同一把锁，所以到这里不会再有新的交错。
        let mut conn = self.db_conn.lock().await;
        let safe = crate::storage::HistoryRepo::filter_recently_injected(&conn, &deletable)
            .map_err(|e| e.to_string())?;
        if safe.is_empty() {
            return Ok(0);
        }
        crate::storage::HistoryRepo::delete_sessions(&mut conn, &safe).map_err(|e| e.to_string())
    }

    /// 按保留策略清理缓存：TTL 淘汰 + 容量配额（LRU），返回删除的条目数。
    ///
    /// **刻意不按账号分区**，与 `prune_history` 相反：磁盘是物理共享资源，
    /// 给 N 个账号各配 10 GB 就是超卖 N 倍，最后谁都写不进去。全局 LRU 下
    /// 一个账号挤掉的是另一个账号的旧**缓存文件**而非历史行——行会保留并显示
    /// 「缓存已清理」，与今天单账号下旧文件被淘汰是同一种体验。
    ///
    /// 与 `prune_history` 相同的三段式，**磁盘 IO 段不持 `db_conn` 锁**：
    /// 改造前本函数从取锁起一路持有到函数尾，`fs::remove_file` 循环就在锁内；
    /// 容量上限开放到 1 TB 后，单轮可能删数千个文件，持锁停顿会把整条信令链路
    /// 一起堵住。`history_pruner` 的模块头注已把这条锁纪律写成总纲，
    /// 这里必须对齐，否则那条纪律形同虚设。
    ///
    /// 两段都尊重剪贴板免疫窗口；`policy` 的任一段为 0 表示关闭该段，
    /// **不是**「阈值为 0 立刻删光」——后者是能造成数据丢失的反向语义。
    pub async fn sweep(&self, policy: RetentionPolicy) -> Result<usize, String> {
        let mut victims: Vec<String> = Vec::new();
        // TTL 段即将释放的体积，容量段要先扣掉它再判断是否仍超限
        let mut ttl_freed: u64 = 0;

        // ---- 第 1 段：选取（持锁），取完即释放 ----
        {
            let conn = self.db_conn.lock().await;

            if policy.ttl_enabled() {
                let mut stmt = conn
                    .prepare(
                        "SELECT file_path, file_size FROM cache_entries
                         WHERE (strftime('%s', 'now') - strftime('%s', created_at)) > ?1
                           AND (clipboard_injected_at IS NULL
                                OR (strftime('%s', 'now') - strftime('%s', clipboard_injected_at)) > ?2)",
                    )
                    .map_err(|e| e.to_string())?;
                let expired: Vec<(String, u64)> = stmt
                    .query_map(rusqlite::params![policy.ttl_seconds(), CLIPBOARD_LOCK_SECS], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .collect();
                drop(stmt);
                for (path, size) in expired {
                    ttl_freed = ttl_freed.saturating_add(size);
                    victims.push(path);
                }
            }

            if policy.quota_enabled() {
                let total: u64 = conn
                    .query_row("SELECT COALESCE(SUM(file_size), 0) FROM cache_entries", [], |row| row.get(0))
                    .unwrap_or(0);

                // 先扣掉 TTL 段即将释放的体积，再判断是否仍然超限。
                //
                // 少了这一步就会**过度删除**：candidates 按 last_accessed_at 排序，
                // 这个顺序与「是否过期」毫无关系，于是一个未过期但很久没访问的活跃
                // 文件会排在已过期文件前面被选中——哪怕只删那些过期文件就已经降到
                // 低水位。默认配置（24h + 10240MB）两段都开，正是最常见的形态。
                let mut remaining = total.saturating_sub(ttl_freed);
                let low = policy.low_watermark_bytes();

                if remaining > low {
                    let mut stmt = conn
                        .prepare(
                            "SELECT file_path, file_size FROM cache_entries
                             WHERE (clipboard_injected_at IS NULL
                                    OR (strftime('%s', 'now') - strftime('%s', clipboard_injected_at)) > ?1)
                             ORDER BY last_accessed_at ASC",
                        )
                        .map_err(|e| e.to_string())?;
                    let candidates: Vec<(String, u64)> = stmt
                        .query_map(rusqlite::params![CLIPBOARD_LOCK_SECS], |row| Ok((row.get(0)?, row.get(1)?)))
                        .map_err(|e| e.to_string())?
                        .filter_map(Result::ok)
                        .collect();
                    drop(stmt);

                    // 成员判断用 HashSet：victims 会持续增长，对它做线性扫描是
                    // O(n²) 字符串比较，而这段就在持锁区间内——三段式的全部意义
                    // 就是不让长耗时操作卡在锁里，不能自己又把它放回去。
                    let mut chosen: HashSet<String> = victims.iter().cloned().collect();
                    for (path, size) in candidates {
                        if remaining <= low {
                            break;
                        }
                        // TTL 段已选中的体积在上面扣过了，这里跳过以免重复计入
                        if chosen.contains(&path) {
                            continue;
                        }
                        remaining = remaining.saturating_sub(size);
                        chosen.insert(path.clone());
                        victims.push(path);
                    }
                }
            }
        } // 锁在此释放

        if victims.is_empty() {
            return Ok(0);
        }

        // ---- 第 2 段：删文件（不持锁）----
        // 删不掉的保留其数据库行，留到下轮重试：只删行会让文件脱离 cache_entries
        // 索引，而所有清理逻辑都以表行为遍历源 ⇒ 成为永远扫不到的磁盘孤儿。
        let mut removed: Vec<String> = Vec::with_capacity(victims.len());
        for path_str in victims {
            let p = PathBuf::from(&path_str);
            if p.exists() {
                if let Err(e) = fs::remove_file(&p) {
                    log::warn!("Failed to remove cached file {} during sweep: {}", p.display(), e);
                    continue;
                }
            }
            removed.push(path_str);
        }
        if removed.is_empty() {
            return Ok(0);
        }

        // ---- 第 3 段：删行（重新取锁，先复核免疫状态，单事务）----
        //
        // 复核是必要的：第 2 段不持锁，用户可能正好在那段时间点了「装载」，
        // `mark_clipboard_injected` 需要同一把锁，因此标记会落在第 1 段选取之后。
        // 与 `prune_history` 的第 3 段对齐。
        //
        // **但它只挡得住行被删**：文件在第 2 段就已经删掉了，救不回来。
        // 它把「行和文件都没了」降级成「行在、文件没了」——后者是既有代码已处理的
        // 可自愈状态（另存为/打开位置都会提示缓存已清理），且用户刚碰过的条目不会
        // 凭空消失。要彻底关掉这个窗口就得持锁做磁盘 IO，那正是三段式要避免的。
        let mut conn = self.db_conn.lock().await;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut deleted = 0usize;
        {
            let mut lock_stmt = tx
                .prepare(
                    "SELECT 1 FROM cache_entries
                     WHERE file_path = ?1
                       AND clipboard_injected_at IS NOT NULL
                       AND (strftime('%s', 'now') - strftime('%s', clipboard_injected_at)) <= ?2
                     LIMIT 1",
                )
                .map_err(|e| e.to_string())?;
            // 同在持锁区间内，复用 prepared statement 而不是每条都重新 prepare
            let mut del_stmt = tx
                .prepare("DELETE FROM cache_entries WHERE file_path = ?1")
                .map_err(|e| e.to_string())?;

            for path_str in &removed {
                let just_injected = lock_stmt
                    .exists(rusqlite::params![path_str, CLIPBOARD_LOCK_SECS])
                    .map_err(|e| e.to_string())?;
                if just_injected {
                    log::info!(
                        "Cache entry {} was injected into clipboard during the sweep window, keeping its row",
                        path_str
                    );
                    continue;
                }
                del_stmt.execute([path_str]).map_err(|e| e.to_string())?;
                deleted += 1;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::HistoryRepo;

    /// 独占的临时目录，Drop 时递归清掉。不引入 tempfile 依赖——
    /// std + 已有的 uuid 足够，为一条测试加 crate 不划算。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("unidrop-prune-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn manager_with_memory_db() -> (CacheManager, Arc<Mutex<Connection>>) {
        let conn = Connection::open_in_memory().unwrap();
        crate::storage::db::create_schema(&conn).unwrap();
        let db = Arc::new(Mutex::new(conn));
        let mgr = CacheManager {
            cache_root: std::env::temp_dir(),
            db_conn: db.clone(),
        };
        (mgr, db)
    }

    fn sample_offer(session_id: &str) -> crate::protocol::TransferOfferPayload {
        crate::protocol::TransferOfferPayload {
            session_id: session_id.to_string(),
            data_type: "FILES".to_string(),
            total_size: 4,
            total_items: 1,
            preview_summary: "demo".to_string(),
            encrypted: false,
            encrypted_metadata: None,
            items: vec![crate::protocol::TransferItemPayload {
                item_index: 0,
                relative_path: "a.bin".to_string(),
                size: 4,
                is_dir: false,
                sha256: "abc".to_string(),
                total_chunks: 1,
            }],
        }
    }

    async fn seed_completed(db: &Arc<Mutex<Connection>>, session_id: &str, file_path: &Path) {
        let conn = db.lock().await;
        HistoryRepo::record_task(&conn, "acct", session_id, "peer", "RECEIVE", &sample_offer(session_id), "TRANSFERRING")
            .unwrap();
        HistoryRepo::update_task_status(&conn, session_id, "COMPLETED", None).unwrap();
        conn.execute(
            "INSERT INTO cache_entries (file_path, session_id, file_size) VALUES (?1, ?2, 4)",
            (file_path.to_str().unwrap(), session_id),
        )
        .unwrap();
    }

    async fn task_exists(db: &Arc<Mutex<Connection>>, session_id: &str) -> bool {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM transfer_tasks WHERE session_id = ?1",
            [session_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    }

    /// 修订计划 §3.2 第 2 段承诺的容错：某个文件删不掉时，**整个会话留到下轮重试**，
    /// 不能只删行——那会留下无人回收的磁盘孤儿（清理逻辑都以表行为遍历源）。
    /// 让登记路径指向一个**目录**，`remove_file` 必然失败，且不受运行用户权限影响。
    #[tokio::test]
    async fn prune_keeps_session_whose_file_cannot_be_removed() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();

        // s_bad 的“文件”其实是目录 → remove_file 失败
        let bad = tmp.path().join("undeletable-dir");
        fs::create_dir_all(&bad).unwrap();
        // s_good 是正常文件
        let good = tmp.path().join("good.bin");
        fs::write(&good, b"data").unwrap();

        seed_completed(&db, "s_bad", &bad).await;
        seed_completed(&db, "s_good", &good).await;
        // 保留窗口 0 条之外的都算候选，但 limit 必须 > 0（0 表示不限制）
        seed_completed(&db, "s_keep", &tmp.path().join("keep.bin")).await;

        let pruned = mgr.prune_history("acct", 1).await.unwrap();

        assert_eq!(pruned, 1, "只应删掉文件真正清理成功的那一个会话");
        assert!(task_exists(&db, "s_bad").await, "文件删除失败的会话必须保留行以便下轮重试");
        assert!(!task_exists(&db, "s_good").await, "文件已删除的会话应连行一起清掉");
        assert!(bad.exists(), "目录仍在，证明这条路径确实删除失败");
        assert!(!good.exists(), "正常文件应已从磁盘删除");
    }

    /// 端到端确认：已装载进剪贴板的会话不会被 `prune_history` 删掉。
    ///
    /// **注意这条测试守护的是第 1 段（选取）的免疫过滤，不是第 3 段的复核。**
    /// 标记在调用 `prune_history` 之前就打上了，选取段的 `NOT EXISTS` 当场就把它
    /// 排除了，执行路径根本到不了第 3 段——把第 3 段的复核整个删掉，这条测试
    /// 依然是绿的（已实测确认）。
    ///
    /// 第 3 段复核要的是「标记出现在选取之后、删行之前」那个无锁窗口，而
    /// `prune_history` 是一次原子调用，单测里没有可靠的注入点，用并发去撞会 flaky。
    /// 所以复核逻辑本身由 `HistoryRepo::filter_recently_injected` 的直接单测守护；
    /// 「第 3 段确实调用了它」这一点目前没有自动化守护，改动那里时请手工留意。
    #[tokio::test]
    async fn injected_session_survives_prune() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();

        let f1 = tmp.path().join("one.bin");
        fs::write(&f1, b"data").unwrap();
        seed_completed(&db, "s_old", &f1).await;
        seed_completed(&db, "s_new", &tmp.path().join("two.bin")).await;

        // 未装载时它确实是待删候选——先确认这一点，否则下面的断言可能是空过
        {
            let conn = db.lock().await;
            let candidates = HistoryRepo::select_prune_candidates(&conn, "acct", 1).unwrap();
            assert_eq!(candidates.len(), 1, "s_old 本应是待删候选");
            assert_eq!(candidates[0].session_id, "s_old");
        }

        mgr.mark_clipboard_injected("s_old").await.unwrap();
        let pruned = mgr.prune_history("acct", 1).await.unwrap();

        assert_eq!(pruned, 0, "已装载进剪贴板的会话不应被删");
        assert!(task_exists(&db, "s_old").await, "刚被用户装载的历史条目不能消失");
        assert!(f1.exists(), "免疫期内文件也不该被删，否则剪贴板里的引用会失效");
    }

    // ---------- sweep：TTL 与容量清理（需求 8） ----------

    /// 写入一个缓存条目，`age_secs` 指定它已存在多久（用于跨过 TTL 阈值）。
    async fn seed_cache_file(
        db: &Arc<Mutex<Connection>>,
        path: &Path,
        size: i64,
        age_secs: i64,
        injected: bool,
    ) {
        fs::write(path, vec![0u8; size.max(1) as usize]).unwrap();
        let conn = db.lock().await;
        conn.execute(
            "INSERT INTO cache_entries (file_path, session_id, file_size, created_at, last_accessed_at, clipboard_injected_at)
             VALUES (?1, 'sess', ?2, datetime('now', ?3), datetime('now', ?4), CASE WHEN ?5 THEN CURRENT_TIMESTAMP ELSE NULL END)",
            rusqlite::params![
                path.to_str().unwrap(),
                size,
                format!("-{} seconds", age_secs),
                format!("-{} seconds", age_secs),
                injected
            ],
        )
        .unwrap();
    }

    async fn cache_row_count(db: &Arc<Mutex<Connection>>) -> i64 {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM cache_entries", [], |r| r.get(0)).unwrap()
    }

    fn policy(ttl_hours: u32, max_mb: u64) -> RetentionPolicy {
        RetentionPolicy { ttl_hours, max_size_bytes: max_mb * 1024 * 1024 }
    }

    #[tokio::test]
    async fn sweep_removes_files_past_ttl() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();
        let old = tmp.path().join("old.bin");
        let fresh = tmp.path().join("fresh.bin");
        seed_cache_file(&db, &old, 10, 90_000, false).await;   // 25 小时前
        seed_cache_file(&db, &fresh, 10, 60, false).await;     // 1 分钟前

        assert_eq!(mgr.sweep(policy(24, 0)).await.unwrap(), 1);
        assert!(!old.exists(), "超期文件应被删除");
        assert!(fresh.exists(), "未超期文件必须保留");
        assert_eq!(cache_row_count(&db).await, 1);
    }

    /// ttl_hours == 0 表示「不按时间清理」，绝不能被当成「阈值 0 秒 ⇒ 全删」。
    /// 这条守的是一个会造成数据丢失的反向语义。
    #[tokio::test]
    async fn sweep_with_zero_ttl_deletes_nothing() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();
        let ancient = tmp.path().join("ancient.bin");
        seed_cache_file(&db, &ancient, 10, 9_000_000, false).await; // 100 天前

        assert_eq!(mgr.sweep(policy(0, 0)).await.unwrap(), 0);
        assert!(ancient.exists(), "TTL 关闭时再老的文件也不该删");
        assert_eq!(cache_row_count(&db).await, 1);
    }

    /// 剪贴板免疫窗口内的文件，即使早已超期也不删。
    #[tokio::test]
    async fn sweep_respects_clipboard_lock() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();
        let locked = tmp.path().join("locked.bin");
        seed_cache_file(&db, &locked, 10, 90_000, true).await;

        assert_eq!(mgr.sweep(policy(24, 0)).await.unwrap(), 0);
        assert!(locked.exists(), "剪贴板引用中的文件不得被 TTL 删掉");
    }

    #[tokio::test]
    async fn sweep_enforces_quota_down_to_low_watermark() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();
        // 上限 10MB → 低水位 8MB；放 5 个 3MB，共 15MB
        let mb = 1024 * 1024;
        for i in 0..5 {
            let p = tmp.path().join(format!("f{}.bin", i));
            // 越早注册的 last_accessed_at 越老，先被 LRU 淘汰
            seed_cache_file(&db, &p, 3 * mb, (5 - i) as i64 * 100, false).await;
        }

        let purged = mgr.sweep(policy(0, 10)).await.unwrap();
        assert!(purged >= 3, "15MB 降到 8MB 以下至少要删 3 个 3MB 文件，实际删了 {}", purged);

        let conn = db.lock().await;
        let total: i64 = conn
            .query_row("SELECT COALESCE(SUM(file_size),0) FROM cache_entries", [], |r| r.get(0))
            .unwrap();
        assert!(total as u64 <= policy(0, 10).low_watermark_bytes(), "应降到低水位以下");
    }

    /// TTL 与容量**同时启用**时不得过度删除。
    ///
    /// 这是 code 阶段审查抓到的唯一 major：容量段的 candidates 按 last_accessed_at
    /// 排序，与「是否过期」毫无关系；若 remaining 从未扣除 TTL 已选体积的总量起算，
    /// 一个未过期但很久没访问的活跃文件就会排在过期文件前面被选中——哪怕只删那些
    /// 过期文件就已经降到低水位。
    ///
    /// 此前两条 sweep 测试各把另一段关成 0，从未覆盖两段同开，而默认配置
    /// （24h + 10240MB）恰恰两段都开。
    #[tokio::test]
    async fn sweep_does_not_over_evict_when_ttl_already_frees_enough() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();
        let mb = 1024 * 1024;

        // active：未过期，但最后访问时间最老 → 在 LRU 候选中排第一
        let active = tmp.path().join("active.bin");
        seed_cache_file(&db, &active, 6 * mb, 60, false).await;
        {
            let conn = db.lock().await;
            conn.execute(
                "UPDATE cache_entries SET last_accessed_at = datetime('now', '-99999 seconds') WHERE file_path = ?1",
                [active.to_str().unwrap()],
            )
            .unwrap();
        }

        // expired：已过期（TTL 会选中），最后访问时间较新
        let expired = tmp.path().join("expired.bin");
        seed_cache_file(&db, &expired, 6 * mb, 90_000, false).await;

        // 上限 10MB → 低水位 8MB；总量 12MB 超限，但删掉 expired 后剩 6MB 已达标
        let purged = mgr.sweep(policy(24, 10)).await.unwrap();

        assert_eq!(purged, 1, "只该删过期的那一个");
        assert!(!expired.exists(), "过期文件应被删除");
        assert!(
            active.exists(),
            "未过期的活跃文件不得被误删——TTL 释放的体积已足够降到低水位"
        );
    }

    #[tokio::test]
    async fn sweep_with_zero_quota_skips_lru() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();
        let mb = 1024 * 1024;
        for i in 0..3 {
            seed_cache_file(&db, &tmp.path().join(format!("g{}.bin", i)), 5 * mb, 60, false).await;
        }

        assert_eq!(mgr.sweep(policy(0, 0)).await.unwrap(), 0, "两段都关闭时不该删任何东西");
        assert_eq!(cache_row_count(&db).await, 3);
    }

    /// 第 2 段删文件失败时保留该条目的行，留到下轮重试——只删行会让文件
    /// 脱离索引，而清理逻辑都以表行为遍历源，成为永远扫不到的孤儿。
    #[tokio::test]
    async fn sweep_keeps_row_when_file_cannot_be_removed() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();

        let bad = tmp.path().join("undeletable-dir");
        fs::create_dir_all(&bad).unwrap();
        {
            let conn = db.lock().await;
            conn.execute(
                "INSERT INTO cache_entries (file_path, session_id, file_size, created_at, last_accessed_at)
                 VALUES (?1, 'sess', 10, datetime('now', '-90000 seconds'), datetime('now', '-90000 seconds'))",
                [bad.to_str().unwrap()],
            )
            .unwrap();
        }

        assert_eq!(mgr.sweep(policy(24, 0)).await.unwrap(), 0, "删不掉的不计入");
        assert_eq!(cache_row_count(&db).await, 1, "行必须保留以便下轮重试");
        assert!(bad.exists());
    }

    #[tokio::test]
    async fn prune_limit_zero_is_a_no_op() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();
        seed_completed(&db, "s1", &tmp.path().join("a.bin")).await;
        assert_eq!(mgr.prune_history("acct", 0).await.unwrap(), 0);
        assert!(task_exists(&db, "s1").await);
    }
}
