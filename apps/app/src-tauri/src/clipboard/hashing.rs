//! Background hashing pipeline: files entries enter the history unresolved
//! (`f: ""`) and get their content ids folded in here.

use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc,
    thread,
};
use tauri::{AppHandle, Emitter, Manager};

use crate::content::{hash_file, tree_parent_at_path, TreeNode};
use crate::store::{
    cached_hash, history_path_for_key, open_history_database, refresh_entry_summary,
    remember_hash, temp_entry_seq, update_pending_entry, HistoryData,
};
use crate::{active_cache_dir, save_active_history, AppState};

use super::capture::entry_extra;

/// How many freshly hashed paths are folded into the entry before the UI is
/// told about the progress.
const HASH_PROGRESS_BATCH: usize = 32;

struct PendingHash {
    path: String,
    source: String,
    size: u64,
    modified_at: Option<u64>,
}

pub(crate) fn queue_hashing(state: &AppState, entry_id: &str) {
    if let Ok(sender) = state.hash_queue.lock() {
        let _ = sender.send(entry_id.to_string());
    }
}

pub(crate) fn pending_entry_ids(history: &HistoryData) -> Vec<String> {
    history
        .active_entries()
        .iter()
        .filter(|entry| entry.sources.files.iter().any(|source| source.file_id.is_none()))
        .map(|entry| entry.id.clone())
        .collect()
}

/// Hashing runs on one background thread: an entry becomes visible and pasteable
/// straight away, and only reaches the server once every content is identified.
pub(crate) fn start_hash_worker(app: AppHandle, receiver: mpsc::Receiver<String>) {
    thread::spawn(move || {
        for entry_id in receiver {
            if let Err(error) = hash_entry_files(&app, &entry_id) {
                eprintln!("ClipRoam: 计算 {entry_id} 的内容标识失败：{error}");
            }
        }
    });
}

fn hash_entry_files(app: &AppHandle, entry_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (history_key, pending) = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let Some(entry) = history.find(entry_id) else {
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
        return app
            .emit("cliproam://entry-ready", entry_id)
            .map_err(|error| error.to_string());
    }

    // A second connection keeps the hash cache off the UI thread's connection.
    let connection = open_history_database(&history_path_for_key(&state.histories_dir, &history_key))?;
    let mut batch = Vec::new();
    for item in pending {
        let modified_at = item.modified_at.map(|value| value as i64).unwrap_or(-1);
        let file_id = cached_hash(&connection, &item.source, item.size, modified_at).or_else(|| {
            // A file that vanished between copy and hash drops out of the tree.
            let hashed = hash_file(Path::new(&item.source)).ok()?;
            remember_hash(&connection, &item.source, item.size, modified_at, &hashed);
            Some(hashed)
        });
        batch.push((item.path, file_id));
        if batch.len() >= HASH_PROGRESS_BATCH {
            if apply_hashes(app, entry_id, &batch, false)?.is_none() {
                return Ok(());
            };
            batch.clear();
        }
    }
    if apply_hashes(app, entry_id, &batch, true)?.is_none() {
        return Ok(());
    };
    app.emit("cliproam://entry-ready", entry_id)
        .map_err(|error| error.to_string())
}

/// Folds resolved content ids into the entry. Only the final call persists, so
/// progress updates stay in memory.
fn apply_hashes(
    app: &AppHandle,
    entry_id: &str,
    resolved: &[(String, Option<String>)],
    persist: bool,
) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let hashes = resolved
        .iter()
        .map(|(path, file_id)| (path.as_str(), file_id.as_deref()))
        .collect::<HashMap<_, _>>();
    let final_entry_id = {
        let Some(entry) = history.find_mut(entry_id) else {
            return Ok(None);
        };
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
        entry.id.clone()
    };
    refresh_entry_summary(&mut history, &final_entry_id, &cache_dir);
    if persist {
        save_active_history(&state, &history)?;
        // The queue row carries the published payload, so the resolved tree
        // must land there too — an unpublished files entry is only synced
        // once its content ids are known.
        if let Some(seq) = temp_entry_seq(&final_entry_id) {
            if let Some(entry) = history.find(&final_entry_id) {
                let payload = entry_extra(entry)?;
                if let Err(error) = update_pending_entry(
                    &history_path_for_key(&state.histories_dir, &history.active_history),
                    seq,
                    &entry.kind,
                    &entry.content,
                    &payload,
                    &entry.created_at,
                ) {
                    eprintln!("ClipRoam: 回写待上传条目失败：{error}");
                }
            }
        }
    }
    drop(history);
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(Some(final_entry_id))
}
