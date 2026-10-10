//! 文件内容相关的查询：指定页的条目详情引用的内容 id。

use std::collections::HashSet;

use rusqlite::types::Value;
use crate::content::entry_contents_of;
use crate::store::select_entries;
use crate::utils::placeholders;
use crate::AppState;

/// Read full cached details only for this page; file trees stay Rust-side.
pub(crate) fn entry_file_ids(
    state: &AppState,
    history: &crate::store::HistoryData,
    entry_ids: &[String],
) -> Result<HashSet<String>, String> {
    if entry_ids.is_empty() { return Ok(HashSet::new()); }
    let path = state.active_history_path(history)?;
    let where_sql = format!("WHERE id IN ({})", placeholders(entry_ids.len()));
    let values = entry_ids.iter().cloned().map(Value::Text).collect::<Vec<_>>();
    state.with_database(&path, |connection| {
        let entries = select_entries(connection, &where_sql, "", &values)?;
        Ok(entries.iter().flat_map(entry_contents_of).map(|(id, _)| id).collect())
    })
}
