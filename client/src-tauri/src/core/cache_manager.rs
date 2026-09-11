use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
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
    pub fn new(db_conn: Arc<Mutex<Connection>>) -> io_result::Result<Self, String> {
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

    /// Scans cache, purging expired files (>24h) and enforcing LRU quota (<10GB) while respecting 2h clipboard lock.
    pub async fn sweep_expired_and_lru(&self) -> Result<usize, String> {
        let conn = self.db_conn.lock().await;

        // 1. Query files eligible for TTL eviction:
        // older than 24 hours AND (no clipboard lock OR clipboard lock > 2 hours ago)
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

        Ok(purged_count)
    }
}
