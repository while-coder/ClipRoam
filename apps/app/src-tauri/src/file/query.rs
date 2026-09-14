//! 文件内容相关的查询命令：历史引用的内容 id。

use std::collections::HashSet;

use super::cache::history_file_ids as cache_history_file_ids;
use crate::store::history_path_for_key;
use crate::AppState;

/// Every content id the durable history references, derived from the entries'
/// extras Rust-side so the whole history never crosses the IPC boundary. The
/// derived set is cached on the history and invalidated by any `entries`
/// write, so repeated calls in a refresh burst re-parse nothing.
pub(crate) fn derived_history_file_ids(
    state: &AppState,
    history: &mut crate::store::HistoryData,
) -> Result<HashSet<String>, String> {
    if history.file_ids.is_none() {
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        let ids = state.with_database(&path, |connection| Ok(cache_history_file_ids(connection)))?;
        history.file_ids = Some(ids);
    }
    Ok(history
        .file_ids
        .as_ref()
        .expect("the set was derived above when absent")
        .clone())
}
