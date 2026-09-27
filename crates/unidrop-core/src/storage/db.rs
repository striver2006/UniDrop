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

    // 删除 paired_devices。
    //
    // 这张表自建库起就没有任何读写：全仓库对它的引用只有一条建表语句，
    // 既没有 repo 模块也没有任何 SQL，配对与 E2EE 都还停在设计稿上。
    // 它与 config.go 顶部注释里被删掉的那两个「定义了、赋值了、从来没人读」
    // 的字段是同一类东西。
    //
    // 更糟的是它**主动制造了账号分区的假象**：一个 `account_id TEXT NOT NULL`
    // 的列会让读 schema 的人以为配对表已经按账号分区了，本轮的需求正是被它
    // 误导出来的（而这已经是它第二次造成误判，上一次见 2026-09-11 的审查留痕）。
    //
    // DROP 在这里可证明无损，因为从来不存在写入方。**不要**把这个模式抄到
    // 任何有数据的表上——那需要真正的迁移，而不是一句 DROP。
    let _ = conn.execute_batch("DROP TABLE IF EXISTS paired_devices;");

    // 历史按账号分区。老库的行没有归属，列留 NULL——
    // 此刻还读不到 settings（init_database 跑在它之前），不知道当前账号是谁。
    // 回填由 claim_unowned_history 在读到 settings 之后完成。
    let _ = conn.execute_batch("ALTER TABLE transfer_tasks ADD COLUMN account_id TEXT;");

    Ok(conn)
}

/// Creates all tables if missing. Also usable on in-memory connections in tests.
pub fn create_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        r#"
        -- 1. Transfer Tasks
        CREATE TABLE IF NOT EXISTS transfer_tasks (
            session_id TEXT PRIMARY KEY,
            -- 归属账号。只有本表带这一列：transfer_items / chunk_bitmaps /
            -- cache_entries 都以 session_id 为外键，归属由父表唯一决定，
            -- 各存一份只会产生四份可能互相矛盾的记录。
            account_id TEXT,
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

        -- 4. （原 paired_devices 表已删除，见 init_database 的迁移说明）

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

/// 把没有归属的历史行认领给 `account_id`，返回认领的行数。
///
/// 只触 NULL 行，因此天然幂等，也因此**绝不能用一个非法账号调用它**：
/// 行一旦被认领就不再是 NULL、永远不会被重新认领，而空串之类的非法账号
/// 又永远无法通过 `validate_account_id` 成为当前账号——那些历史就此在应用内
/// 永久不可见，只能手工改库恢复。调用方必须先校验（见 lib.rs 的调用点）。
///
/// 跳过一次是安全的：NULL 行原样留着，推迟到某个合法账号启动时再认领。
/// 这个「宁可推迟也不要错认领」的不对称性，正是校验放在调用方而不是
/// 在这里兜底的理由。
pub fn claim_unowned_history(conn: &Connection, account_id: &str) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "UPDATE transfer_tasks SET account_id = ?1 WHERE account_id IS NULL",
        [account_id],
    )
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

/// 读一个 `local_config` 标记，不存在（或读失败）返回 `None`。
///
/// 与上面几个函数不同，键名由调用方给：`device_id` / `app_settings` 是两个
/// 各有语义的单例，写死键名是合适的；而这里服务的是一族「后端自己写、
/// 前端不参与」的开关，每加一个就复制一对专用函数没有意义。
///
/// **这类标记刻意不放进 `AppSettings`。** `cmd_save_settings` 收的是前端提交的
/// **整份** settings，后端若往里写字段，用户随便保存一次设置就会把它冲回默认值
/// （前端手里那份是打开面板时拉的旧副本）——经典的 read-modify-write 覆盖。
/// `AppSettings` 的不变量是「前端是唯一写入方」，后端状态不能混进去。
pub fn get_local_flag(conn: &Connection, key: &str) -> Option<String> {
    let mut stmt = conn
        .prepare("SELECT value FROM local_config WHERE key = ?1")
        .ok()?;
    stmt.query_row([key], |row| row.get(0)).ok()
}

/// 写一个 `local_config` 标记。
pub fn set_local_flag(conn: &Connection, key: &str, value: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO local_config (key, value, updated_at) VALUES (?1, ?2, CURRENT_TIMESTAMP)",
        [key, value],
    )?;
    Ok(())
}
