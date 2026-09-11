use rusqlite::Connection;

pub struct BitmapRepo;

impl BitmapRepo {
    /// Records a received chunk in the database.
    pub fn mark_chunk_received(conn: &Connection, session_id: &str, item_index: u32, chunk_index: u32, checksum: u32) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT OR REPLACE INTO chunk_bitmaps (session_id, item_index, chunk_index, checksum, received_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)",
            (session_id, item_index, chunk_index, checksum),
        )?;
        Ok(())
    }

    /// Queries all received chunk indices for a given item in a session.
    pub fn get_received_chunks(conn: &Connection, session_id: &str, item_index: u32) -> Result<Vec<u32>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT chunk_index FROM chunk_bitmaps WHERE session_id = ?1 AND item_index = ?2 ORDER BY chunk_index ASC"
        )?;

        let rows = stmt.query_map((session_id, item_index), |row| row.get(0))?;
        let mut chunks = Vec::new();
        for r in rows {
            chunks.push(r?);
        }
        Ok(chunks)
    }

    /// Deletes all recorded chunks for a session when transfer finishes or is cancelled.
    pub fn clear_session_bitmaps(conn: &Connection, session_id: &str) -> Result<(), rusqlite::Error> {
        conn.execute("DELETE FROM chunk_bitmaps WHERE session_id = ?1", [session_id])?;
        Ok(())
    }
}
