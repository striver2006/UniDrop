//! 传输历史修剪的统一触发入口（需求 4）
//!
//! # 必须在释放 `db_conn` 锁之后调用
//!
//! 本函数内部会 `state.db_conn.lock().await`。`tokio::sync::Mutex` **不可重入**，
//! 在任何仍持有该锁的作用域里调用它 = 进程永久死锁，且没有任何报错，
//! 表现为整个应用静默卡死。
//!
//! 具体到既有代码，这些地方的锁 **guard 活到哪里**：
//!
//! - `transfer_engine::update_history_status`：guard 的作用域就是那个函数体。
//!   所以只能在 `update_history_status(...).await` **返回之后**调用，
//!   **绝不能**把调用写进它的函数体内。
//! - `lib.rs` 的 TRANSFER_FAILURE 分支：guard 在一个显式 `{ }` 块内，
//!   只能在那个块**之后**调用。
//! - `cmd_save_settings`：落库与内存更新各自在独立块内，块后调用安全。

use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::AppState;

/// 按当前设置修剪历史；确有删除时广播 `history-pruned` 事件。
///
/// 失败只记日志：修剪是后台维护动作，不该让一次传输或一次保存设置因此失败。
pub async fn prune_and_notify(app: &AppHandle) {
    let state = app.state::<AppState>();

    // 单独取一次 settings 锁并立刻释放，避免与下面的 db_conn 锁交叠持有
    let (limit, account_id) = {
        let settings = state.settings.lock().await;
        (settings.history_max_entries, settings.account_id.clone())
    };
    if limit == 0 {
        return; // 0 = 不限制
    }
    // 账号非法时不修剪：带着空账号去查只会命中零行（认领同样会跳过它），
    // 白跑一趟还不如不跑。
    if crate::commands::settings_cmd::validate_account_id(&account_id).is_err() {
        return;
    }

    match state.cache_manager.prune_history(&account_id, limit).await {
        Ok(0) => {}
        Ok(pruned) => {
            log::info!("Pruned {} transfer history entries (limit {})", pruned, limit);
            // 前端 HistoryPanel 只在挂载 / 收到终态事件 / 手动点刷新时重拉，
            // 不广播的话「保存设置后列表立刻变短」就实现不了。
            let _ = app.emit("history-pruned", pruned);
        }
        Err(e) => log::warn!("Failed to prune transfer history: {}", e),
    }
}
