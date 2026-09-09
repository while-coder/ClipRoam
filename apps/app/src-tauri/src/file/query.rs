//! 文件内容相关的查询命令：历史引用的内容 id，以及候选上传条目。

use tauri::State;

use super::cache::history_file_ids as cache_history_file_ids;
use crate::content::{refresh_summary, ClipboardEntry};
use crate::entry::lightweight_entry;
use crate::store::{history_path_for_key, select_entries};
use crate::{active_cache_dir, AppState};

/// Upper bound for `list_upload_candidates`, so a huge library cannot turn a
/// settings toggle into an unbounded publish run.
const UPLOAD_CANDIDATE_LIMIT: usize = 500;

/// Every content id the durable history references, derived from the entries'
/// extras Rust-side so the whole history never crosses the IPC boundary. The
/// frontend asks the pool (`/files/query`) which of these it holds to render
/// upload status; nothing is persisted locally. The derived set is cached on
/// the history and invalidated by any `entries` write, so repeated calls in a
/// refresh burst re-parse nothing.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn history_file_ids(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    if history.file_ids.is_none() {
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        let ids = state.with_database(&path, |connection| Ok(cache_history_file_ids(connection)))?;
        history.file_ids = Some(ids);
    }
    Ok(history
        .file_ids
        .as_ref()
        .expect("the set was derived above when absent")
        .iter()
        .cloned()
        .collect())
}

/// Files/image entries fully hashed whose uploadable payload fits the
/// automatic-upload limit; drives the settings page's "upload now" run.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn list_upload_candidates(
    state: State<'_, AppState>,
    limit_bytes: u64,
) -> Result<Vec<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    // Files entries still hashing carry an unresolved `"fileId":null` source;
    // images have no hashing step.
    let mut entries = state.with_database(&path, |connection| {
        select_entries(
            connection,
            "WHERE kind IN ('files', 'image') AND (kind = 'image' OR sources NOT LIKE '%\"fileId\":null%')",
            "",
            &[],
        )
    })?;
    let mut candidates = Vec::new();
    for entry in &mut entries {
        refresh_summary(entry, &history.cached_files, &cache_dir);
        if entry.summary.uploadable_size.is_some_and(|size| size < limit_bytes) {
            candidates.push(lightweight_entry(entry));
            if candidates.len() >= UPLOAD_CANDIDATE_LIMIT {
                break;
            }
        }
    }
    Ok(candidates)
}
