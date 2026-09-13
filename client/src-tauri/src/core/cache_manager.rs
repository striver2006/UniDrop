use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use rusqlite::Connection;
use tokio::sync::Mutex;

pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 3600); // 24 hours
pub const CLIPBOARD_LOCK_DURATION: Duration = Duration::from_secs(2 * 3600); // 2 hours
pub const MAX_CACHE_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GB
pub const SAFE_LOW_WATERMARK_BYTES: u64 = 8 * 1024 * 1024 * 1024; // 8 GB

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
    /// **先删文件后删行**，与 `sweep_expired_and_lru` 同序。反过来先删行的话，
    /// 一旦 `remove_file` 失败，文件就脱离了 `cache_entries` 索引，而全部清理逻辑
    /// 都以表行为遍历源 ⇒ 永远扫不到它，成为不可回收的磁盘孤儿。
    /// 反向留下的「行在、文件没了」则可自愈：下轮修剪会重试，且另存为/打开位置
    /// 本就处理了文件缺失。
    pub async fn prune_history(&self, limit: u32) -> Result<usize, String> {
        // 1. 选取段：取完即释放锁
        let candidates = {
            let conn = self.db_conn.lock().await;
            crate::storage::HistoryRepo::select_prune_candidates(&conn, limit)
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

    /// Scans cache, purging expired files (>24h) and enforcing LRU quota (<10GB) while respecting 2h clipboard lock (P1-9, P3-3).
    pub async fn sweep_expired_and_lru(&self) -> Result<usize, String> {
        let conn = self.db_conn.lock().await;

        // 1. Query files eligible for TTL eviction (>24h and no active clipboard lock)
        let mut stmt = conn.prepare(
            "SELECT file_path FROM cache_entries
             WHERE (strftime('%s', 'now') - strftime('%s', created_at)) > 86400
               AND (clipboard_injected_at IS NULL OR (strftime('%s', 'now') - strftime('%s', clipboard_injected_at)) > 7200)"
        ).map_err(|e| e.to_string())?;

        let expired_files: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);

        let mut purged_count = 0;
        for path_str in &expired_files {
            let p = PathBuf::from(path_str);
            if p.exists() {
                let _ = fs::remove_file(&p);
            }
            conn.execute("DELETE FROM cache_entries WHERE file_path = ?1", [path_str]).ok();
            purged_count += 1;
        }

        // 2. Enforce LRU quota: if total cache size > 10GB, evict oldest accessed files down to 8GB
        let mut total_size_stmt = conn.prepare("SELECT COALESCE(SUM(file_size), 0) FROM cache_entries").map_err(|e| e.to_string())?;
        let mut current_total: u64 = total_size_stmt.query_row([], |row| row.get(0)).unwrap_or(0);
        drop(total_size_stmt);

        if current_total > MAX_CACHE_SIZE_BYTES {
            let mut lru_stmt = conn.prepare(
                "SELECT file_path, file_size FROM cache_entries
                 WHERE (clipboard_injected_at IS NULL OR (strftime('%s', 'now') - strftime('%s', clipboard_injected_at)) > 7200)
                 ORDER BY last_accessed_at ASC"
            ).map_err(|e| e.to_string())?;

            let lru_entries: Vec<(String, u64)> = lru_stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .collect();
            drop(lru_stmt);

            for (path_str, size) in lru_entries {
                if current_total <= SAFE_LOW_WATERMARK_BYTES {
                    break;
                }
                let p = PathBuf::from(&path_str);
                if p.exists() {
                    let _ = fs::remove_file(&p);
                }
                conn.execute("DELETE FROM cache_entries WHERE file_path = ?1", [&path_str]).ok();
                current_total = current_total.saturating_sub(size);
                purged_count += 1;
            }
        }

        Ok(purged_count)
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
        HistoryRepo::record_task(&conn, session_id, "peer", "RECEIVE", &sample_offer(session_id), "TRANSFERRING")
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

        let pruned = mgr.prune_history(1).await.unwrap();

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
            let candidates = HistoryRepo::select_prune_candidates(&conn, 1).unwrap();
            assert_eq!(candidates.len(), 1, "s_old 本应是待删候选");
            assert_eq!(candidates[0].session_id, "s_old");
        }

        mgr.mark_clipboard_injected("s_old").await.unwrap();
        let pruned = mgr.prune_history(1).await.unwrap();

        assert_eq!(pruned, 0, "已装载进剪贴板的会话不应被删");
        assert!(task_exists(&db, "s_old").await, "刚被用户装载的历史条目不能消失");
        assert!(f1.exists(), "免疫期内文件也不该被删，否则剪贴板里的引用会失效");
    }

    #[tokio::test]
    async fn prune_limit_zero_is_a_no_op() {
        let tmp = TempDir::new();
        let (mgr, db) = manager_with_memory_db();
        seed_completed(&db, "s1", &tmp.path().join("a.bin")).await;
        assert_eq!(mgr.prune_history(0).await.unwrap(), 0);
        assert!(task_exists(&db, "s1").await);
    }
}
