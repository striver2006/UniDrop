use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

pub fn init_database(db_path: Option<PathBuf>) -> Result<Connection, rusqlite::Error> {
    let path = db_path.unwrap_or_else(|| {
        let root = crate::core::CacheManager::resolve_default_cache_dir();
        fs::create_dir_all(&root).ok();
        root.join("unidrop.db")
    });

    let conn = Connection::open(&path)?;

    // Enable WAL mode for concurrent performance
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

    create_schema(&conn)?;

    // Lightweight migration for databases created before preview_summary existed.
    // Errors (duplicate column) are intentionally ignored.
    let _ = conn.execute_batch("ALTER TABLE transfer_tasks ADD COLUMN preview_summary TEXT;");

    Ok(conn)
}

/// Creates all tables if missing. Also usable on in-memory connections in tests.
pub fn create_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        r#"
        -- 1. Transfer Tasks
        CREATE TABLE IF NOT EXISTS transfer_tasks (
            session_id TEXT PRIMARY KEY,
            remote_device_id TEXT NOT NULL,
            direction TEXT CHECK(direction IN ('SEND', 'RECEIVE')) NOT NULL,
            data_type TEXT NOT NULL,
            total_size INTEGER NOT NULL,
            total_items INTEGER NOT NULL,
            status TEXT CHECK(status IN ('PENDING', 'TRANSFERRING', 'COMPLETED', 'FAILED', 'CANCELLED')) NOT NULL,
            error_message TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            completed_at DATETIME,
            preview_summary TEXT
        );

        -- 2. Transfer Items
        CREATE TABLE IF NOT EXISTS transfer_items (
            session_id TEXT NOT NULL,
            item_index INTEGER NOT NULL,
            relative_path TEXT NOT NULL,
            size INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            total_chunks INTEGER NOT NULL,
            status TEXT CHECK(status IN ('WAITING', 'DOWNLOADING', 'VERIFIED', 'CORRUPTED')) NOT NULL,
            PRIMARY KEY (session_id, item_index),
            FOREIGN KEY (session_id) REFERENCES transfer_tasks(session_id) ON DELETE CASCADE
        );

        -- 3. Chunk Bitmaps (for Resumable Transfer)
        CREATE TABLE IF NOT EXISTS chunk_bitmaps (
            session_id TEXT NOT NULL,
            item_index INTEGER NOT NULL,
            chunk_index INTEGER NOT NULL,
            checksum INTEGER NOT NULL,
            received_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (session_id, item_index, chunk_index)
        );

        -- 4. Paired Trusted Devices
        CREATE TABLE IF NOT EXISTS paired_devices (
            device_id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL,
            alias TEXT NOT NULL,
            os_type TEXT NOT NULL,
            ed25519_pubkey BLOB NOT NULL,
            x25519_pubkey BLOB NOT NULL,
            paired_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            is_trusted INTEGER DEFAULT 1
        );

        -- 5. Cache Lifecycle with 2h Clipboard Immunity Lock
        CREATE TABLE IF NOT EXISTS cache_entries (
            file_path TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            clipboard_injected_at DATETIME,
            last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        -- 6. Local Config & Device ID Persistence (P1-4, P1-5)
        CREATE TABLE IF NOT EXISTS local_config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )?;

    Ok(())
}

/// Retrieves the existing device_id from SQLite or generates and persists a new one (P1-4).
pub fn get_or_create_device_id(conn: &Connection) -> Result<String, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT value FROM local_config WHERE key = 'device_id'")?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        return Ok(id);
    }
    drop(rows);
    drop(stmt);

    let new_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO local_config (key, value) VALUES ('device_id', ?1)",
        [&new_id],
    )?;
    Ok(new_id)
}

/// Retrieves saved settings JSON string if present (P1-5).
pub fn get_persisted_settings(conn: &Connection) -> Option<String> {
    let mut stmt = conn.prepare("SELECT value FROM local_config WHERE key = 'app_settings'").ok()?;
    stmt.query_row([], |row| row.get(0)).ok()
}

/// Persists settings JSON string into SQLite (P1-5).
pub fn save_persisted_settings(conn: &Connection, json_val: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO local_config (key, value, updated_at) VALUES ('app_settings', ?1, CURRENT_TIMESTAMP)",
        [json_val],
    )?;
    Ok(())
}
