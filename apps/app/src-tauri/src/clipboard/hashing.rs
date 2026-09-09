//! 同步时的内容解析：files 条目带着 `f: ""` 占位树进入历史，上传队列的
//! drain 取到该行时才在这里把内容 id（sha256）解析出来。文本与图片在捕获
//! 时就已完成，不会走到这里。

use std::{collections::HashMap, path::Path};
use tauri::{AppHandle, Emitter, Manager};

use crate::content::{describe_roots, hash_file, tree_parent_at_path, ClipboardEntryExtra, TreeNode};
use crate::store::{cached_hash, history_path_for_key, remember_hash, select_entry};
use crate::AppState;

/// How many freshly hashed paths are folded into the entry before the UI is
/// told about the progress.
const HASH_PROGRESS_BATCH: usize = 32;

struct PendingHash {
    path: String,
    source: String,
    size: u64,
    modified_at: Option<u64>,
}

/// Resolves every still-unresolved content id of a `files` entry, folding the
/// results into its SQLite row. Runs when the upload queue's drain reaches the
/// entry, or when a manual upload picks it. A vanished entry — deleted while
/// the queue waited — ends the run quietly; the caller cleans up the queue
/// row. Idempotent: already-resolved sources are skipped.
pub(crate) fn resolve_entry_files(app: &AppHandle, entry_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (history_key, pending) = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        let Some(entry) = state.with_database(&path, |connection| select_entry(connection, entry_id))? else {
            return Ok(());
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
        (history.active_history.clone(), pending)
    };
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
            if apply_hashes(app, entry_id, &batch)?.is_none() {
                return Ok(());
            }
            batch.clear();
        }
    }
    if !batch.is_empty() && apply_hashes(app, entry_id, &batch)?.is_none() {
        return Ok(());
    }
    Ok(())
}

/// Folds resolved content ids into the entry row. Only the SQLite row is
/// written — the queue row keeps the capture-time payload, and the publish
/// flow reads the resolved entry back from here.
fn apply_hashes(
    app: &AppHandle,
    entry_id: &str,
    resolved: &[(String, Option<String>)],
) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    let Some(mut entry) = state
        .with_database(&path, |connection| select_entry(connection, entry_id))?
    else {
        // The row is gone — the entry was deleted while hashing ran. Drop
        // silently; the caller stops the run.
        return Ok(None);
    };
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
    // The tree changed, so the stored content description is refreshed too and
    // the keyword search stays true.
    let content = match &entry.file_info {
        Some(file_info) => describe_roots(file_info),
        None => entry.content.clone(),
    };
    let extra = ClipboardEntryExtra::of(&entry).json()?;
    let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
    let final_entry_id = entry.id.clone();
    let changed = state.with_database(&path, |connection| {
        connection
            .execute(
                "UPDATE entries SET content = ?, extra = ?, sources = ? WHERE id = ?",
                rusqlite::params![content, extra, sources, final_entry_id],
            )
            .map_err(|error| error.to_string())
    })?;
    if changed == 0 {
        return Ok(None);
    }
    drop(history);
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(Some(final_entry_id))
}
