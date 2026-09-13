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

/// 一个待修剪的会话，连同它在缓存中登记的文件路径。
///
/// 路径在这里只是**取出来**，删除动作由 `CacheManager` 执行：本模块是纯 SQL 层，
/// 全文不做文件 IO，测试才能全部跑在 `Connection::open_in_memory()` 上。
#[derive(Debug, Clone)]
pub struct PruneCandidate {
    pub session_id: String,
    pub file_paths: Vec<String>,
}

/// 剪贴板免疫窗口（秒）。从 `core::retention` 引入，全仓仅此一处定义——
/// 改造前同一个 7200 散落在死常量、两处 SQL 字面量和本模块共四处。
use crate::core::retention::CLIPBOARD_LOCK_SECS;

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

    /// 把残留的非终态行复位为 FAILED，返回复位条数。
    ///
    /// 历史行在任务**开始**时就以 TRANSFERRING 落库（`lib.rs` 收、`clipboard_cmd.rs` 发），
    /// 进程被杀或崩溃时它永远停在那里。而修剪按约束 A 每轮都跳过非终态行，
    /// 这类死行会永久占住「最新 N 条」的额度——正是需求 4 要收敛的对象。
    ///
    /// 复位是安全的：本仓库没有续传路径（`BitmapRepo` / `chunk_bitmaps` 除建表语句外
    /// 无任何生产引用），不存在会被这一下丢掉的在途状态。**只在启动时调用**，
    /// 那时也不可能有真正在传的任务。
    pub fn reset_stale_in_flight(conn: &Connection) -> Result<usize, rusqlite::Error> {
        let affected = conn.execute(
            "UPDATE transfer_tasks
             SET status = 'FAILED',
                 error_message = COALESCE(error_message, '进程异常退出'),
                 completed_at = CURRENT_TIMESTAMP
             WHERE status IN ('PENDING', 'TRANSFERRING')",
            [],
        )?;
        Ok(affected)
    }

    /// 选出超出保留上限、且可以安全删除的会话（修剪三段式的第 1 段）。
    ///
    /// `limit == 0` 表示不限制，直接返回空。
    ///
    /// 保留集是**全部行**按 `created_at DESC, rowid DESC` 排序后的前 `limit` 名——
    /// 排序键与 `list_history` 逐字一致，否则「列表里看得到的第 N 条」与
    /// 「被删的第 N 条」会错位。进行中的任务也参与排序占位（用户说的「最新 N 条」
    /// 包含它们），但**不会**被选为删除候选：
    ///
    /// - 约束 A：只有终态（COMPLETED / FAILED / CANCELLED）才可删。删掉进行中的行会让
    ///   `update_task_status` 的 UPDATE 影响 0 行且**不报错**，故障极难追查。
    /// - 约束 B：会话中只要还有一个文件处在 2 小时剪贴板免疫窗口内就整体跳过——
    ///   用户刚把它装载进系统剪贴板，随时可能粘贴。条数超限是攒出来的慢问题，
    ///   剪贴板失效是下一秒就撞上的快问题，本轮跳过，下一轮再收。
    pub fn select_prune_candidates(
        conn: &Connection,
        limit: u32,
    ) -> Result<Vec<PruneCandidate>, rusqlite::Error> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare(
            "SELECT t.session_id
             FROM transfer_tasks t
             WHERE t.status IN ('COMPLETED', 'FAILED', 'CANCELLED')
               AND NOT EXISTS (
                   SELECT 1 FROM cache_entries c
                   WHERE c.session_id = t.session_id
                     AND c.clipboard_injected_at IS NOT NULL
                     AND (strftime('%s', 'now') - strftime('%s', c.clipboard_injected_at)) <= ?2
               )
               AND t.session_id NOT IN (
                   SELECT session_id FROM transfer_tasks
                   ORDER BY created_at DESC, rowid DESC
                   LIMIT ?1
               )",
        )?;

        let session_ids: Vec<String> = stmt
            .query_map(rusqlite::params![limit, CLIPBOARD_LOCK_SECS], |row| row.get(0))?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);

        // prepare 提到循环外复用：候选多时每轮重新 prepare 同一条 SQL 是纯浪费
        let mut files_stmt = conn.prepare("SELECT file_path FROM cache_entries WHERE session_id = ?1")?;
        let mut candidates = Vec::with_capacity(session_ids.len());
        for session_id in session_ids {
            let file_paths: Vec<String> = files_stmt
                .query_map([&session_id], |row| row.get(0))?
                .filter_map(Result::ok)
                .collect();
            candidates.push(PruneCandidate {
                session_id,
                file_paths,
            });
        }
        drop(files_stmt);
        Ok(candidates)
    }

    /// 从候选里剔除处于剪贴板免疫窗口内的会话，返回仍可安全删行的那些。
    ///
    /// 存在的理由是一个 TOCTOU 窗口：修剪三段式为了不持锁做磁盘 IO，在选取与删行
    /// 之间放开了 `db_conn` 锁。用户若恰好在这个窗口里点「装载」，
    /// `cmd_inject_session` 会在选取**之后**打上 `clipboard_injected_at`，
    /// 而选取段的免疫判据早已算完 ⇒ 这条刚被用户装载的记录照样会被删掉。
    ///
    /// **本复核只挡得住「行被删」。** 文件在第 2 段就已经删掉了，救不回来——
    /// 它把「行和文件都没了」降级成「行在、文件没了」，后者是既有代码已处理的
    /// 可自愈状态（另存为/打开位置都会提示缓存已清理），且用户刚用过的历史条目
    /// 不会凭空消失。要彻底关掉这个窗口就得持锁做 IO，那正是三段式要避免的。
    ///
    /// 判据与 `select_prune_candidates` 逐字一致，共用 `CLIPBOARD_LOCK_SECS`。
    pub fn filter_recently_injected(
        conn: &Connection,
        session_ids: &[String],
    ) -> Result<Vec<String>, rusqlite::Error> {
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare(
            "SELECT 1 FROM cache_entries
             WHERE session_id = ?1
               AND clipboard_injected_at IS NOT NULL
               AND (strftime('%s', 'now') - strftime('%s', clipboard_injected_at)) <= ?2
             LIMIT 1",
        )?;

        let mut safe = Vec::with_capacity(session_ids.len());
        for session_id in session_ids {
            let locked = stmt
                .exists(rusqlite::params![session_id, CLIPBOARD_LOCK_SECS])?;
            if locked {
                log::info!(
                    "Session {} was injected into clipboard during the prune window, keeping its row",
                    session_id
                );
            } else {
                safe.push(session_id.clone());
            }
        }
        drop(stmt);
        Ok(safe)
    }

    /// 在单个事务内删除这些会话的全部数据库痕迹（修剪三段式的第 3 段）。
    ///
    /// 四张表必须**逐一显式删除**，不能指望级联：`transfer_items` 虽然声明了
    /// `ON DELETE CASCADE`，但 `init_database` 从未开启 `PRAGMA foreign_keys`，
    /// SQLite 默认关闭外键 ⇒ 级联不会发生；`chunk_bitmaps` 与 `cache_entries`
    /// 连外键声明都没有。
    ///
    /// 表间顺序先子后父：中途失败留下的是「父在、子少」的可恢复状态，
    /// 而不是「父没了、子成孤儿」的不可追溯状态。
    pub fn delete_sessions(
        conn: &mut Connection,
        session_ids: &[String],
    ) -> Result<usize, rusqlite::Error> {
        if session_ids.is_empty() {
            return Ok(0);
        }

        let tx = conn.transaction()?;
        let mut deleted = 0usize;
        for session_id in session_ids {
            tx.execute("DELETE FROM cache_entries WHERE session_id = ?1", [session_id])?;
            tx.execute("DELETE FROM chunk_bitmaps WHERE session_id = ?1", [session_id])?;
            tx.execute("DELETE FROM transfer_items WHERE session_id = ?1", [session_id])?;
            deleted += tx.execute("DELETE FROM transfer_tasks WHERE session_id = ?1", [session_id])?;
        }
        tx.commit()?;
        Ok(deleted)
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

    // ---------- 修剪（需求 4） ----------

    /// 按插入顺序建 n 条已完成的任务：s1 最旧，sN 最新（同秒内靠 rowid 分先后）。
    fn seed_completed(conn: &Connection, n: usize) {
        for i in 1..=n {
            let sid = format!("s{}", i);
            HistoryRepo::record_task(conn, &sid, "peer", "RECEIVE", &sample_offer(&sid, "FILES"), "TRANSFERRING")
                .unwrap();
            HistoryRepo::update_task_status(conn, &sid, "COMPLETED", None).unwrap();
        }
    }

    fn add_cache_entry(conn: &Connection, session_id: &str, path: &str, injected_now: bool) {
        let injected = if injected_now { "CURRENT_TIMESTAMP" } else { "NULL" };
        conn.execute(
            &format!(
                "INSERT INTO cache_entries (file_path, session_id, file_size, clipboard_injected_at)
                 VALUES (?1, ?2, 10, {})",
                injected
            ),
            (path, session_id),
        )
        .unwrap();
    }

    fn session_ids(candidates: &[PruneCandidate]) -> Vec<String> {
        let mut v: Vec<String> = candidates.iter().map(|c| c.session_id.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn prune_keeps_newest_and_drops_oldest() {
        let mut conn = test_conn();
        seed_completed(&conn, 5);

        let candidates = HistoryRepo::select_prune_candidates(&conn, 3).unwrap();
        assert_eq!(session_ids(&candidates), vec!["s1", "s2"], "应只删最旧的两条");

        let ids: Vec<String> = candidates.into_iter().map(|c| c.session_id).collect();
        assert_eq!(HistoryRepo::delete_sessions(&mut conn, &ids).unwrap(), 2);

        let left = HistoryRepo::list_history(&conn, 10).unwrap();
        assert_eq!(left.len(), 3);
        assert_eq!(left[0].session_id, "s5", "最新的必须还在");
    }

    #[test]
    fn prune_limit_zero_means_unlimited() {
        let conn = test_conn();
        seed_completed(&conn, 5);
        assert!(
            HistoryRepo::select_prune_candidates(&conn, 0).unwrap().is_empty(),
            "0 表示不限制，一条都不该选中"
        );
    }

    /// 约束 A：进行中的任务即使排在保留窗口之外也不能删——删掉会让后续的
    /// update_task_status 静默影响 0 行，故障无从追查。
    #[test]
    fn prune_never_touches_in_flight_tasks() {
        let conn = test_conn();
        HistoryRepo::record_task(&conn, "old-running", "peer", "RECEIVE", &sample_offer("old-running", "FILES"), "TRANSFERRING")
            .unwrap();
        seed_completed(&conn, 4);

        let candidates = HistoryRepo::select_prune_candidates(&conn, 2).unwrap();
        let ids = session_ids(&candidates);
        assert!(!ids.contains(&"old-running".to_string()), "进行中的任务不得被选为删除候选");
        assert_eq!(ids, vec!["s1", "s2"]);
    }

    /// 约束 B：文件还在 2 小时剪贴板免疫窗口内的会话本轮跳过，
    /// 否则用户刚装载进剪贴板、下一秒 Ctrl+V 就粘不出来。
    #[test]
    fn prune_respects_clipboard_immunity_lock() {
        let conn = test_conn();
        seed_completed(&conn, 5);
        add_cache_entry(&conn, "s1", "/tmp/locked.bin", true);
        add_cache_entry(&conn, "s2", "/tmp/free.bin", false);

        let ids = session_ids(&HistoryRepo::select_prune_candidates(&conn, 3).unwrap());
        assert_eq!(ids, vec!["s2"], "s1 处于免疫窗口内，本轮应跳过");
    }

    /// 外键默认关闭（init_database 从未开启 PRAGMA foreign_keys），级联不会发生。
    /// 这条测试守护「四张表逐一显式删除」——漏掉任何一张都会留下孤儿行。
    #[test]
    fn delete_sessions_clears_all_four_tables() {
        let mut conn = test_conn();
        HistoryRepo::record_task(&conn, "s1", "peer", "RECEIVE", &sample_offer("s1", "FILES"), "TRANSFERRING").unwrap();
        HistoryRepo::update_task_status(&conn, "s1", "COMPLETED", None).unwrap();
        add_cache_entry(&conn, "s1", "/tmp/a.bin", false);
        conn.execute(
            "INSERT INTO chunk_bitmaps (session_id, item_index, chunk_index, checksum) VALUES ('s1', 0, 0, 1)",
            [],
        )
        .unwrap();

        HistoryRepo::delete_sessions(&mut conn, &["s1".to_string()]).unwrap();

        for table in ["transfer_tasks", "transfer_items", "chunk_bitmaps", "cache_entries"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {} WHERE session_id = 's1'", table), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{} 仍残留 s1 的行", table);
        }
    }

    #[test]
    fn prune_candidates_carry_their_cached_file_paths() {
        let conn = test_conn();
        seed_completed(&conn, 3);
        add_cache_entry(&conn, "s1", "/tmp/one.bin", false);
        add_cache_entry(&conn, "s1", "/tmp/two.bin", false);

        let candidates = HistoryRepo::select_prune_candidates(&conn, 2).unwrap();
        assert_eq!(candidates.len(), 1);
        let mut paths = candidates[0].file_paths.clone();
        paths.sort();
        assert_eq!(paths, vec!["/tmp/one.bin", "/tmp/two.bin"]);
    }

    /// 候选集与 list_history 看得到的前 N 条必须无交集。两处排序键一旦不一致，
    /// 「界面上看到的第 N 条」和「被删的第 N 条」就会错位。
    #[test]
    fn prune_candidates_never_overlap_visible_head() {
        let conn = test_conn();
        seed_completed(&conn, 6);

        let visible: Vec<String> = HistoryRepo::list_history(&conn, 4)
            .unwrap()
            .into_iter()
            .map(|e| e.session_id)
            .collect();
        let candidates = session_ids(&HistoryRepo::select_prune_candidates(&conn, 4).unwrap());

        for id in &candidates {
            assert!(!visible.contains(id), "{} 同时出现在保留窗口与删除候选中", id);
        }
        assert_eq!(candidates.len(), 2);
    }

    /// 关的是选取与删行之间那个无锁窗口：窗口期内被装载的会话必须从待删集合里剔除。
    #[test]
    fn filter_recently_injected_drops_freshly_loaded_sessions() {
        let conn = test_conn();
        seed_completed(&conn, 3);
        add_cache_entry(&conn, "s1", "/tmp/injected.bin", true);
        add_cache_entry(&conn, "s2", "/tmp/plain.bin", false);
        // s3 干脆没有缓存行，也应视为可删

        let ids = vec!["s1".to_string(), "s2".to_string(), "s3".to_string()];
        let safe = HistoryRepo::filter_recently_injected(&conn, &ids).unwrap();

        assert_eq!(safe, vec!["s2".to_string(), "s3".to_string()], "只有 s1 处于免疫窗口内");
    }

    #[test]
    fn filter_recently_injected_on_empty_input_is_empty() {
        let conn = test_conn();
        assert!(HistoryRepo::filter_recently_injected(&conn, &[]).unwrap().is_empty());
    }

    // ---------- 启动期复位（需求 4 的前置） ----------

    #[test]
    fn reset_stale_in_flight_converts_only_unfinished_rows() {
        let conn = test_conn();
        HistoryRepo::record_task(&conn, "running", "peer", "RECEIVE", &sample_offer("running", "FILES"), "TRANSFERRING")
            .unwrap();
        HistoryRepo::record_task(&conn, "pending", "peer", "SEND", &sample_offer("pending", "TEXT"), "PENDING").unwrap();
        HistoryRepo::record_task(&conn, "done", "peer", "SEND", &sample_offer("done", "TEXT"), "TRANSFERRING").unwrap();
        HistoryRepo::update_task_status(&conn, "done", "COMPLETED", None).unwrap();

        assert_eq!(HistoryRepo::reset_stale_in_flight(&conn).unwrap(), 2);

        let status_of = |sid: &str| -> String {
            conn.query_row("SELECT status FROM transfer_tasks WHERE session_id = ?1", [sid], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(status_of("running"), "FAILED");
        assert_eq!(status_of("pending"), "FAILED");
        assert_eq!(status_of("done"), "COMPLETED", "既有终态不得被改写");
    }

    /// 复位之后那些死行才能被修剪回收——这正是加复位的理由。
    #[test]
    fn reset_then_prune_reclaims_dead_rows() {
        let mut conn = test_conn();
        HistoryRepo::record_task(&conn, "dead", "peer", "RECEIVE", &sample_offer("dead", "FILES"), "TRANSFERRING")
            .unwrap();
        seed_completed(&conn, 3);

        assert!(
            HistoryRepo::select_prune_candidates(&conn, 2).unwrap().iter().all(|c| c.session_id != "dead"),
            "复位前，死行受约束 A 保护"
        );

        HistoryRepo::reset_stale_in_flight(&conn).unwrap();
        let ids: Vec<String> = HistoryRepo::select_prune_candidates(&conn, 2)
            .unwrap()
            .into_iter()
            .map(|c| c.session_id)
            .collect();
        assert!(ids.contains(&"dead".to_string()), "复位后死行应可被回收");

        HistoryRepo::delete_sessions(&mut conn, &ids).unwrap();
        assert_eq!(HistoryRepo::list_history(&conn, 10).unwrap().len(), 2);
    }
}
