//! 同步时的内容解析：files 队列行带着 `f: ""` 占位树进入队列，上传队列的
//! drain 取到该行时才在这里把内容 id（sha256）解析出来并写回该行。文本与
//! 图片在捕获时就已完成，不会走到这里。

use std::{collections::HashMap, path::Path};
use tauri::{AppHandle, Emitter, Manager};

use super::{list_rows, row_entry};
use crate::content::{
    describe_roots, hash_file, tree_parent_at_path, ClipboardEntry, ClipboardEntryExtra, TreeNode,
};
use crate::store::{cached_hash, history_path_for_key, remember_hash};
use crate::AppState;

/// How many freshly hashed paths are folded into the row before the UI is
/// told about the progress.
const HASH_PROGRESS_BATCH: usize = 32;

struct PendingHash {
    path: String,
    source: String,
    size: u64,
    modified_at: Option<u64>,
}

/// Resolves every still-unresolved content id of a `files` queue row, folding
/// the results back into the row itself. Runs when the upload queue's drain
/// reaches the row. Idempotent: already-resolved sources are skipped.
pub fn resolve_entry_files(app: &AppHandle, seq: i64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (history_key, mut entry) = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        let Some(row) = state.with_database(&path, |connection| {
            Ok(list_rows(connection)?
                .into_iter()
                .find(|row| row.seq == seq))
        })?
        else {
            return Ok(());
        };
        (history.active_history.clone(), row_entry(&row))
    };
    let pending = entry
        .sources
        .files
        .iter()
        .filter(|source| source.file_id.is_none())
        .map(|source| PendingHash {
            path: source.path.clone(),
            source: source.source.clone(),
            size: source.size,
            modified_at: source.modified_at,
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(());
    }

    // The hash cache shares the pooled database connection; each lookup only
    // holds it briefly, so hashing a large file never blocks a history write.
    let hash_database = history_path_for_key(&state.histories_dir, &history_key);
    let mut batch = Vec::new();
    for item in pending {
        let modified_at = item.modified_at.map(|value| value as i64).unwrap_or(-1);
        let file_id = state
            .with_database(&hash_database, |connection| {
                Ok(cached_hash(connection, &item.source, item.size, modified_at))
            })
            .ok()
            .flatten()
            .or_else(|| {
                // A file that vanished between copy and hash drops out of the tree.
                let hashed = hash_file(Path::new(&item.source)).ok()?;
                let _ = state.with_database(&hash_database, |connection| {
                    remember_hash(connection, &item.source, item.size, modified_at, &hashed);
                    Ok(())
                });
                Some(hashed)
            });
        batch.push((item.path, file_id));
        if batch.len() >= HASH_PROGRESS_BATCH {
            apply_hashes(app, seq, &mut entry, &batch)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        apply_hashes(app, seq, &mut entry, &batch)?;
    }
    Ok(())
}

/// Folds resolved content ids into the queue row: the tree leaves, the content
/// description (so keyword search stays true) and the sources whose files
/// vanished between copy and hash.
fn apply_hashes(
    app: &AppHandle,
    seq: i64,
    entry: &mut ClipboardEntry,
    resolved: &[(String, Option<String>)],
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let hashes = resolved
        .iter()
        .map(|(path, file_id)| (path.as_str(), file_id.as_deref()))
        .collect::<HashMap<_, _>>();
    if let Some(file_info) = entry.file_info.as_mut() {
        for (path, file_id) in resolved {
            let Some(parent) = tree_parent_at_path(file_info, path) else {
                continue;
            };
            let leaf = path.rsplit('/').next().unwrap_or_default();
            match file_id {
                Some(file_id) => {
                    if let Some(TreeNode::File { f, .. }) = parent.get_mut(leaf) {
                        *f = file_id.clone();
                    }
                }
                // A file that vanished between copy and hash drops out of the tree.
                None => {
                    parent.shift_remove(leaf);
                }
            }
        }
    }
    entry.sources.files.retain_mut(|source| match hashes.get(source.path.as_str()) {
        Some(Some(file_id)) => {
            source.file_id = Some((*file_id).to_string());
            true
        }
        Some(None) => false,
        None => true,
    });
    let content = match &entry.file_info {
        Some(file_info) => describe_roots(file_info),
        None => entry.content.clone(),
    };
    let extra = ClipboardEntryExtra::of(entry).json()?;
    let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
    {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&path, |connection| {
            connection
                .execute(
                    "UPDATE pending_entries SET content = ?, extra = ?, sources = ? WHERE seq = ?",
                    rusqlite::params![content, extra, sources, seq],
                )
                .map_err(|error| error.to_string())
        })?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(())
}
