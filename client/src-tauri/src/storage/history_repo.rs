use rusqlite::Connection;
use serde::Serialize;

use crate::protocol::TransferOfferPayload;

/// Persistent transfer history backed by the transfer_tasks/transfer_items tables.
pub struct HistoryRepo;

#[derive(Debug, Clone, Serialize)]
pub struct TransferHistoryEntry {
    pub session_id: String,
    pub direction: String,
    pub data_type: String,
    pub preview_summary: Option<String>,
    pub total_size: i64,
    pub total_items: i64,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: Option<String>,
    pub completed_at: Option<String>,
    /// Number of files still present in the local cache for this session.
    pub cached_count: i64,
}

#[derive(Debug, Clone)]
pub struct TaskBrief {
    pub direction: String,
    pub data_type: String,
    pub preview_summary: Option<String>,
    pub total_size: i64,
}

impl HistoryRepo {
    /// Inserts (or replaces) a task row together with its items. Used when a
    /// transfer is first observed (outgoing send / incoming offer).
    pub fn record_task(
        conn: &Connection,
        session_id: &str,
        remote_device_id: &str,
        direction: &str,
        offer: &TransferOfferPayload,
        status: &str,
    ) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT OR REPLACE INTO transfer_tasks
                (session_id, remote_device_id, direction, data_type, preview_summary,
                 total_size, total_items, status, error_message, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, CURRENT_TIMESTAMP, NULL)",
            (
                session_id,
                remote_device_id,
                direction,
                offer.data_type.clone(),
                offer.preview_summary.as_str(),
                offer.total_size,
                offer.total_items as i64,
                status,
            ),
        )?;

        for item in &offer.items {
            conn.execute(
                "INSERT OR REPLACE INTO transfer_items
                    (session_id, item_index, relative_path, size, sha256, total_chunks, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'WAITING')",
                (
                    session_id,
                    item.item_index as i64,
                    item.relative_path.as_str(),
                    item.size,
                    item.sha256.as_str(),
                    item.total_chunks as i64,
                ),
            )?;
        }
        Ok(())
    }

    /// Updates task status; stamps completed_at on terminal states.
    pub fn update_task_status(
        conn: &Connection,
        session_id: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        conn.execute(
            "UPDATE transfer_tasks
             SET status = ?2,
                 error_message = ?3,
                 completed_at = CASE WHEN ?2 IN ('COMPLETED', 'FAILED', 'CANCELLED') THEN CURRENT_TIMESTAMP ELSE completed_at END
             WHERE session_id = ?1",
            (session_id, status, error_message),
        )?;
        Ok(())
    }

    /// Minimal task info for building UI events (e.g. peer TRANSFER_FAILURE).
    pub fn get_task_brief(conn: &Connection, session_id: &str) -> Option<TaskBrief> {
        conn.query_row(
            "SELECT direction, data_type, preview_summary, total_size FROM transfer_tasks WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok(TaskBrief {
                    direction: row.get(0)?,
                    data_type: row.get(1)?,
                    preview_summary: row.get(2)?,
                    total_size: row.get(3)?,
                })
            },
        )
        .ok()
    }

    /// Lists the most recent tasks, oldest first is reversed — newest first.
    pub fn list_history(conn: &Connection, limit: u32) -> Result<Vec<TransferHistoryEntry>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT t.session_id, t.direction, t.data_type, t.preview_summary, t.total_size,
                    t.total_items, t.status, t.error_message, t.created_at, t.completed_at,
                    (SELECT COUNT(*) FROM cache_entries c WHERE c.session_id = t.session_id) AS cached_count
             FROM transfer_tasks t
             ORDER BY t.created_at DESC, t.rowid DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map([limit], |row| {
            Ok(TransferHistoryEntry {
                session_id: row.get(0)?,
                direction: row.get(1)?,
                data_type: row.get(2)?,
                preview_summary: row.get(3)?,
                total_size: row.get(4)?,
                total_items: row.get(5)?,
                status: row.get(6)?,
                error_message: row.get(7)?,
                created_at: row.get(8)?,
                completed_at: row.get(9)?,
                cached_count: row.get(10)?,
            })
        })?;

        let mut entries = Vec::new();
        for r in rows {
            entries.push(r?);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TransferItemPayload;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::storage::db::create_schema(&conn).unwrap();
        conn
    }

    fn sample_offer(session_id: &str, data_type: &str) -> TransferOfferPayload {
        TransferOfferPayload {
            session_id: session_id.to_string(),
            data_type: data_type.to_string(),
            total_size: 12,
            total_items: 1,
            preview_summary: "demo".to_string(),
            encrypted: false,
            encrypted_metadata: None,
            items: vec![TransferItemPayload {
                item_index: 0,
                relative_path: "a.txt".to_string(),
                size: 12,
                is_dir: false,
                sha256: "abc".to_string(),
                total_chunks: 1,
            }],
        }
    }

    #[test]
    fn test_history_crud() {
        let conn = test_conn();

        HistoryRepo::record_task(&conn, "s1", "peer-1", "RECEIVE", &sample_offer("s1", "FILES"), "TRANSFERRING").unwrap();
        HistoryRepo::record_task(&conn, "s2", "peer-2", "SEND", &sample_offer("s2", "TEXT"), "TRANSFERRING").unwrap();

        HistoryRepo::update_task_status(&conn, "s1", "COMPLETED", None).unwrap();
        HistoryRepo::update_task_status(&conn, "s2", "FAILED", Some("data connect failed")).unwrap();

        let entries = HistoryRepo::list_history(&conn, 10).unwrap();
        assert_eq!(entries.len(), 2);
        // newest first: s2 recorded after s1 within the same second, so rely on rowid tiebreak
        assert_eq!(entries[0].session_id, "s2");
        assert_eq!(entries[0].status, "FAILED");
        assert_eq!(entries[0].error_message.as_deref(), Some("data connect failed"));
        assert_eq!(entries[1].session_id, "s1");
        assert_eq!(entries[1].status, "COMPLETED");
        assert!(entries[1].completed_at.is_some());

        let brief = HistoryRepo::get_task_brief(&conn, "s1").unwrap();
        assert_eq!(brief.direction, "RECEIVE");
        assert_eq!(brief.data_type, "FILES");
        assert_eq!(brief.preview_summary.as_deref(), Some("demo"));

        assert!(HistoryRepo::get_task_brief(&conn, "missing").is_none());
    }
}
