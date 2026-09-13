//! 服务端传输限额的只读查询（需求 3）。
//!
//! 限额的事实源在服务端的环境变量里，经 AUTH_RESPONSE 下发。本模块只负责把
//! 已下发的那份读出来给界面看——**没有写回命令，这是刻意的**。
//!
//! 曾经的需求第 6 条要求「客户端可以设置服务端的一些参数」，论证后发现在当前
//! 架构下无法成立：整个部署共用一把 PSK，account_id 由客户端自报，按字面实现
//! 等于任何能连上的客户端都能改写全服务端策略。该条需求随后被删除。
//! 三个候选方案的完整论证存档在 `.reviews/server-transfer-limits/plan/` 下，
//! 日后若重新提上日程不必再推导一遍。
//!
//! 这里也**不预留任何「为将来准备」的空壳**——空壳本身就会变成下一个
//! MaxItemsPerOffer：看起来是事实源，实际没人读。

use tauri::State;

use crate::app_state::AppState;
use crate::protocol::ServerLimits;

/// 读取服务端下发的传输限额。
///
/// 返回 `None` 表示尚未拿到：要么还没连上，要么对端是不下发这个字段的老服务端。
/// 界面必须把 `None` 显示成「未下发」，**不要显示 0，也不要显示默认值**——
/// 显示 0 会被读成「配额为零」，显示默认值会让用户以为那就是实际生效的值。
#[tauri::command]
pub async fn cmd_get_server_limits(
    state: State<'_, AppState>,
) -> Result<Option<ServerLimits>, String> {
    let limits = state.server_limits.lock().await;
    Ok(limits.clone())
}
