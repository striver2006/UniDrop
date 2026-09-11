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
