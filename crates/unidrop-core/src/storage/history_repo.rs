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
        account_id: &str,
        session_id: &str,
        remote_device_id: &str,
        direction: &str,
        offer: &TransferOfferPayload,
        status: &str,
    ) -> Result<(), rusqlite::Error> {
        conn.execute(
            "INSERT OR REPLACE INTO transfer_tasks
                (session_id, account_id, remote_device_id, direction, data_type, preview_summary,
                 total_size, total_items, status, error_message, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, CURRENT_TIMESTAMP, NULL)",
            (
                session_id,
                account_id,
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

    /// 核验某个会话是否归属指定账号。
    ///
    /// 供三个直接操作文件的 IPC 命令（装载剪贴板 / 另存为 / 在文件管理器中显示）
    /// 在动文件之前调用。这不是冗余的纵深防御：`list_history` 的过滤只保证
    /// 「看不见」，而界面残留、事件丢失、用户在刷新落地前抢先点击，任何一种
    /// 都会让「按钮只作用于可见行」这个前提失效——失效的后果是把另一个账号的
    /// 文件写进剪贴板。前端状态的正确性不该是文件访问控制的唯一依据。
    ///
    /// 归属未知（account_id 为 NULL，即尚未认领的老库行）时返回 false：
    /// 宁可让用户重启一次应用完成认领，也不要放行一条来历不明的记录。
    pub fn session_belongs_to(conn: &Connection, session_id: &str, account_id: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM transfer_tasks WHERE session_id = ?1 AND account_id = ?2",
            rusqlite::params![session_id, account_id],
            |_| Ok(()),
        )
        .is_ok()
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
    /// 列出**当前账号**的历史。
    ///
    /// 账号作为显式首参而不从全局读：显式参数让「忘了过滤」成为编译错误，
    /// 读全局只会让它成为运行期的静默串台。与服务端 registry 改复合键同理。
    pub fn list_history(conn: &Connection, account_id: &str, limit: u32) -> Result<Vec<TransferHistoryEntry>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT t.session_id, t.direction, t.data_type, t.preview_summary, t.total_size,
                    t.total_items, t.status, t.error_message, t.created_at, t.completed_at,
                    (SELECT COUNT(*) FROM cache_entries c WHERE c.session_id = t.session_id) AS cached_count
             FROM transfer_tasks t
             WHERE t.account_id = ?1
             ORDER BY t.created_at DESC, t.rowid DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(rusqlite::params![account_id, limit], |row| {
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
    /// 选出**当前账号**可修剪的会话。
    ///
    /// 账号条件要加在**两处**，漏掉第二处的后果反直觉，所以单独说明：
    ///
    /// 1. 外层 `WHERE` —— 圈定本账号的可删候选；
    /// 2. `NOT IN` 保留窗口子查询 —— 「最新 N 条」的窗口本身。
    ///
    /// 只加第一处的话，保留窗口仍是全局的：另一个账号的新行占满窗口名额后，
    /// 本账号的修剪会把**自己**最新 N 条之内的行判成超限删掉。注意失败形态
    /// 不是「删到别人的行」，而是「删掉自己该留的行」——它违反的是本文件
    /// 既有测试守护的「候选集与可见前 N 条不得相交」不变量，且只在两个账号
    /// 的行在时间上交错时才显现（一方全新或全旧时两种写法结果相同）。
    ///
    /// 中间那段 `NOT EXISTS` 剪贴板免疫子查询**不需要**账号条件：它已由
    /// `c.session_id = t.session_id` 与外层关联，外层收窄即自动收窄。
    pub fn select_prune_candidates(
        conn: &Connection,
        account_id: &str,
        limit: u32,
    ) -> Result<Vec<PruneCandidate>, rusqlite::Error> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare(
            "SELECT t.session_id
             FROM transfer_tasks t
             WHERE t.account_id = ?3
               AND t.status IN ('COMPLETED', 'FAILED', 'CANCELLED')
               AND NOT EXISTS (
                   SELECT 1 FROM cache_entries c
                   WHERE c.session_id = t.session_id
                     AND c.clipboard_injected_at IS NOT NULL
                     AND (strftime('%s', 'now') - strftime('%s', c.clipboard_injected_at)) <= ?2
               )
               AND t.session_id NOT IN (
                   SELECT session_id FROM transfer_tasks
                   WHERE account_id = ?3
                   ORDER BY created_at DESC, rowid DESC
                   LIMIT ?1
               )",
        )?;

        let session_ids: Vec<String> = stmt
            .query_map(rusqlite::params![limit, CLIPBOARD_LOCK_SECS, account_id], |row| row.get(0))?
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
            e2ee_version: None,
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

        HistoryRepo::record_task(&conn, "acct", "s1", "peer-1", "RECEIVE", &sample_offer("s1", "FILES"), "TRANSFERRING").unwrap();
        HistoryRepo::record_task(&conn, "acct", "s2", "peer-2", "SEND", &sample_offer("s2", "TEXT"), "TRANSFERRING").unwrap();

        HistoryRepo::update_task_status(&conn, "s1", "COMPLETED", None).unwrap();
        HistoryRepo::update_task_status(&conn, "s2", "FAILED", Some("data connect failed")).unwrap();

        let entries = HistoryRepo::list_history(&conn, "acct", 10).unwrap();
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

    // ---------- 按账号分区 ----------

    /// 按指定账号与创建时间种一条已完成的行。
    /// `age_secs` 越大越旧，用来构造两个账号在时间上**交错**的分布。
    fn seed_for(conn: &Connection, account: &str, sid: &str, age_secs: i64) {
        HistoryRepo::record_task(conn, account, sid, "peer", "RECEIVE", &sample_offer(sid, "FILES"), "TRANSFERRING")
            .unwrap();
        HistoryRepo::update_task_status(conn, sid, "COMPLETED", None).unwrap();
        conn.execute(
            "UPDATE transfer_tasks SET created_at = datetime('now', ?2) WHERE session_id = ?1",
            rusqlite::params![sid, format!("-{} seconds", age_secs)],
        )
        .unwrap();
    }

    /// 删掉 list_history 的账号过滤，这条立刻红 —— 那正是「切账号后
    /// 历史面板列出上一个账号的文件名」的形态。
    #[test]
    fn list_history_only_returns_current_account() {
        let conn = test_conn();
        seed_for(&conn, "alice", "a1", 10);
        seed_for(&conn, "alice", "a2", 20);
        seed_for(&conn, "bob", "b1", 15);

        let alice = HistoryRepo::list_history(&conn, "alice", 100).unwrap();
        assert_eq!(alice.len(), 2, "alice 只应看到自己的两条");
        assert!(alice.iter().all(|e| e.session_id.starts_with('a')));

        let bob = HistoryRepo::list_history(&conn, "bob", 100).unwrap();
        assert_eq!(bob.len(), 1, "bob 只应看到自己的一条");
        assert_eq!(bob[0].session_id, "b1");
    }

    /// 泄露的具体内容是文件名与预览摘要，单断条数不够。
    #[test]
    fn list_history_does_not_leak_other_account_previews() {
        let conn = test_conn();
        seed_for(&conn, "alice", "secret-session", 10);

        let bob = HistoryRepo::list_history(&conn, "bob", 100).unwrap();
        assert!(bob.is_empty(), "bob 不得看到 alice 的任何一条记录");
    }

    /// **本组最关键的一条。**
    ///
    /// 守的是 select_prune_candidates 的**第二处**过滤（NOT IN 保留窗口子查询）。
    /// 只在外层加账号条件的话，保留窗口仍是全局的：bob 的新行占满窗口名额后，
    /// alice 的修剪会把 alice **自己**最新 N 条之内的行判成超限删掉。
    ///
    /// 注意失败形态不是「删到别人的行」，而是「删掉自己该留的行」——
    /// 所以下面那条 prune_never_touches_other_account_rows 抓不住它。
    /// 也必须刻意让两个账号的行在时间上**交错**：bob 的行全新或全旧时，
    /// 两种写法结果相同，用例就没有鉴别力了。
    #[test]
    fn prune_window_is_per_account_when_rows_interleave() {
        let conn = test_conn();
        // 时间交错：bob 的两条夹在 alice 的三条中间
        seed_for(&conn, "alice", "a_new", 10);
        seed_for(&conn, "bob", "b_1", 20);
        seed_for(&conn, "alice", "a_mid", 30);
        seed_for(&conn, "bob", "b_2", 40);
        seed_for(&conn, "alice", "a_old", 50);

        // alice 保留最新 2 条 => 只有 a_old 该被修剪
        let candidates = HistoryRepo::select_prune_candidates(&conn, "alice", 2).unwrap();
        let ids = session_ids(&candidates);

        assert_eq!(
            ids,
            vec!["a_old".to_string()],
            "alice 保留窗口内的 a_new / a_mid 一条都不能进候选；\
             实得 {:?} —— 多半是 NOT IN 子查询漏了 account_id 过滤，\
             bob 的新行占掉了 alice 的窗口名额",
            ids
        );
    }

    /// 修剪不得越界删到别人的行。与上一条守的是不同的失败形态。
    #[test]
    fn prune_never_touches_other_account_rows() {
        let conn = test_conn();
        seed_for(&conn, "alice", "a1", 10);
        seed_for(&conn, "bob", "b1", 20);
        seed_for(&conn, "bob", "b2", 30);
        seed_for(&conn, "bob", "b3", 40);

        // alice 一条都不保留，也只能删到自己那条
        let candidates = HistoryRepo::select_prune_candidates(&conn, "alice", 0).unwrap();
        assert!(candidates.is_empty(), "limit=0 表示不限制，不应有候选");

        let candidates = HistoryRepo::select_prune_candidates(&conn, "alice", 1).unwrap();
        assert!(
            session_ids(&candidates).iter().all(|id| id.starts_with('a')),
            "alice 的修剪不得把 bob 的行选进候选：{:?}",
            session_ids(&candidates)
        );
    }

    /// 老库的 NULL 行归给当前账号。
    #[test]
    fn claim_assigns_legacy_rows_to_current_account() {
        let conn = test_conn();
        HistoryRepo::record_task(&conn, "alice", "legacy", "peer", "RECEIVE", &sample_offer("legacy", "FILES"), "TRANSFERRING").unwrap();
        // 人为制造老库形态
        conn.execute("UPDATE transfer_tasks SET account_id = NULL", []).unwrap();
        assert!(HistoryRepo::list_history(&conn, "alice", 10).unwrap().is_empty());

        let claimed = crate::storage::db::claim_unowned_history(&conn, "alice").unwrap();
        assert_eq!(claimed, 1);
        assert_eq!(HistoryRepo::list_history(&conn, "alice", 10).unwrap().len(), 1);
    }

    /// 认领必须幂等，且**不得改走已有归属的行**。
    /// 否则每次切账号都会把上一个账号的历史搬过来——认领的全部安全性
    /// 都建立在「只触 NULL 行」上。
    #[test]
    fn claim_is_idempotent_and_does_not_resteal() {
        let conn = test_conn();
        seed_for(&conn, "alice", "a1", 10);

        let claimed = crate::storage::db::claim_unowned_history(&conn, "bob").unwrap();
        assert_eq!(claimed, 0, "已有归属的行不得被 bob 认领走");
        assert_eq!(HistoryRepo::list_history(&conn, "alice", 10).unwrap().len(), 1);
        assert!(HistoryRepo::list_history(&conn, "bob", 10).unwrap().is_empty());
    }

    /// 切账号是「看不见」而不是「被删掉」，切回来必须原样在。
    #[test]
    fn switching_account_hides_but_keeps_history() {
        let conn = test_conn();
        seed_for(&conn, "alice", "a1", 10);

        assert!(HistoryRepo::list_history(&conn, "bob", 10).unwrap().is_empty());
        let back = HistoryRepo::list_history(&conn, "alice", 10).unwrap();
        assert_eq!(back.len(), 1, "切回 alice 后历史必须原样回来");
        assert_eq!(back[0].session_id, "a1");
    }

    /// 归属闸门：三个直接读文件的 IPC 命令据此判断。
    #[test]
    fn session_ownership_gate_rejects_other_account_and_unclaimed() {
        let conn = test_conn();
        seed_for(&conn, "alice", "a1", 10);

        assert!(HistoryRepo::session_belongs_to(&conn, "a1", "alice"));
        assert!(!HistoryRepo::session_belongs_to(&conn, "a1", "bob"), "跨账号必须拒绝");
        assert!(!HistoryRepo::session_belongs_to(&conn, "missing", "alice"));

        // 未认领的老库行归属未知，宁可拒绝
        conn.execute("UPDATE transfer_tasks SET account_id = NULL", []).unwrap();
        assert!(
            !HistoryRepo::session_belongs_to(&conn, "a1", "alice"),
            "归属未知的行不得放行——宁可让用户重启一次完成认领"
        );
    }

    /// `reset_stale_in_flight` **刻意保持全局**，这条把那个决策钉成会红的断言。
    ///
    /// 按账号过滤的话，上个账号崩溃留下的死行永远不会被复位，
    /// 会永久占住它的保留额度并在它的历史里显示成「传输中」。
    #[test]
    fn reset_stale_in_flight_covers_all_accounts() {
        let conn = test_conn();
        HistoryRepo::record_task(&conn, "alice", "a_dead", "peer", "RECEIVE", &sample_offer("a_dead", "FILES"), "TRANSFERRING").unwrap();
        HistoryRepo::record_task(&conn, "bob", "b_dead", "peer", "RECEIVE", &sample_offer("b_dead", "FILES"), "TRANSFERRING").unwrap();

        let reset = HistoryRepo::reset_stale_in_flight(&conn).unwrap();
        assert_eq!(reset, 2, "两个账号的死行都必须被复位，不论当前账号是谁");

        for (acct, sid) in [("alice", "a_dead"), ("bob", "b_dead")] {
            let entries = HistoryRepo::list_history(&conn, acct, 10).unwrap();
            assert_eq!(entries[0].session_id, sid);
            assert_eq!(entries[0].status, "FAILED", "{} 的死行应被复位为 FAILED", acct);
        }
    }

    // ---------- 修剪（需求 4） ----------

    /// 按插入顺序建 n 条已完成的任务：s1 最旧，sN 最新（同秒内靠 rowid 分先后）。
    fn seed_completed(conn: &Connection, n: usize) {
        for i in 1..=n {
            let sid = format!("s{}", i);
            HistoryRepo::record_task(conn, "acct", &sid, "peer", "RECEIVE", &sample_offer(&sid, "FILES"), "TRANSFERRING")
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

        let candidates = HistoryRepo::select_prune_candidates(&conn, "acct", 3).unwrap();
        assert_eq!(session_ids(&candidates), vec!["s1", "s2"], "应只删最旧的两条");

        let ids: Vec<String> = candidates.into_iter().map(|c| c.session_id).collect();
        assert_eq!(HistoryRepo::delete_sessions(&mut conn, &ids).unwrap(), 2);

        let left = HistoryRepo::list_history(&conn, "acct", 10).unwrap();
        assert_eq!(left.len(), 3);
        assert_eq!(left[0].session_id, "s5", "最新的必须还在");
    }

    #[test]
    fn prune_limit_zero_means_unlimited() {
        let conn = test_conn();
        seed_completed(&conn, 5);
        assert!(
            HistoryRepo::select_prune_candidates(&conn, "acct", 0).unwrap().is_empty(),
            "0 表示不限制，一条都不该选中"
        );
    }

    /// 约束 A：进行中的任务即使排在保留窗口之外也不能删——删掉会让后续的
    /// update_task_status 静默影响 0 行，故障无从追查。
    #[test]
    fn prune_never_touches_in_flight_tasks() {
        let conn = test_conn();
        HistoryRepo::record_task(&conn, "acct", "old-running", "peer", "RECEIVE", &sample_offer("old-running", "FILES"), "TRANSFERRING")
            .unwrap();
        seed_completed(&conn, 4);

        let candidates = HistoryRepo::select_prune_candidates(&conn, "acct", 2).unwrap();
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

        let ids = session_ids(&HistoryRepo::select_prune_candidates(&conn, "acct", 3).unwrap());
        assert_eq!(ids, vec!["s2"], "s1 处于免疫窗口内，本轮应跳过");
    }

    /// 外键默认关闭（init_database 从未开启 PRAGMA foreign_keys），级联不会发生。
    /// 这条测试守护「四张表逐一显式删除」——漏掉任何一张都会留下孤儿行。
    #[test]
    fn delete_sessions_clears_all_four_tables() {
        let mut conn = test_conn();
        HistoryRepo::record_task(&conn, "acct", "s1", "peer", "RECEIVE", &sample_offer("s1", "FILES"), "TRANSFERRING").unwrap();
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

        let candidates = HistoryRepo::select_prune_candidates(&conn, "acct", 2).unwrap();
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

        let visible: Vec<String> = HistoryRepo::list_history(&conn, "acct", 4)
            .unwrap()
            .into_iter()
            .map(|e| e.session_id)
            .collect();
        let candidates = session_ids(&HistoryRepo::select_prune_candidates(&conn, "acct", 4).unwrap());

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
        HistoryRepo::record_task(&conn, "acct", "running", "peer", "RECEIVE", &sample_offer("running", "FILES"), "TRANSFERRING")
            .unwrap();
        HistoryRepo::record_task(&conn, "acct", "pending", "peer", "SEND", &sample_offer("pending", "TEXT"), "PENDING").unwrap();
        HistoryRepo::record_task(&conn, "acct", "done", "peer", "SEND", &sample_offer("done", "TEXT"), "TRANSFERRING").unwrap();
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
        HistoryRepo::record_task(&conn, "acct", "dead", "peer", "RECEIVE", &sample_offer("dead", "FILES"), "TRANSFERRING")
            .unwrap();
        seed_completed(&conn, 3);

        assert!(
            HistoryRepo::select_prune_candidates(&conn, "acct", 2).unwrap().iter().all(|c| c.session_id != "dead"),
            "复位前，死行受约束 A 保护"
        );

        HistoryRepo::reset_stale_in_flight(&conn).unwrap();
        let ids: Vec<String> = HistoryRepo::select_prune_candidates(&conn, "acct", 2)
            .unwrap()
            .into_iter()
            .map(|c| c.session_id)
            .collect();
        assert!(ids.contains(&"dead".to_string()), "复位后死行应可被回收");

        HistoryRepo::delete_sessions(&mut conn, &ids).unwrap();
        assert_eq!(HistoryRepo::list_history(&conn, "acct", 10).unwrap().len(), 2);
    }
}
