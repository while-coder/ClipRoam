//! `files` 表：服务器池可用性的本地持久层。
//!
//! 结构与服务器 FileStore 的同名表一致：`stored = 1` 表示服务器池持有该
//! 内容的字节，`size` 是内容大小。本机 blob 是否在磁盘不进这张表——磁盘
//! 即真相（见 `cache.rs` 的 blob 目录扫描）。表里只有确认过的池状态：id
//! 不在其中 = 从未向服务器查询过（`find_unknown_file_ids` 的「未知」），唯一
//! 写入是前端对未知 id 的 `/files/query` 回填（`upsert_server_files`）——
//! 上传完成与 `file.available` 推送只触发刷新，由该回填持久化。

use std::collections::HashSet;

use rusqlite::{params, Connection};
use serde::Deserialize;
use tauri::State;

use crate::store::{history_path_for_key, HistoryData};
use crate::{AppState};

/// One `/files/query` answer, straight off the wire (`FileStatus` in the
/// protocol).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileStatusInput {
    pub(crate) file_id: String,
    #[serde(default)]
    pub(crate) size: u64,
    #[serde(default)]
    pub(crate) stored: bool,
}

pub(crate) fn stored_file_ids(connection: &Connection) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare("SELECT file_id FROM files WHERE stored = 1")
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(ids)
}

fn all_file_ids(connection: &Connection) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare("SELECT file_id FROM files")
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(ids)
}

/// The update is monotonic on purpose: a late query racing a finished upload
/// must not walk a confirmed `stored = 1` back to 0, and the in-memory set
/// this table replaced only ever grew too.
fn upsert_status_rows(connection: &Connection, statuses: &[FileStatusInput]) -> Result<(), String> {
    if statuses.is_empty() {
        return Ok(());
    }
    let mut statement = connection
        .prepare(
            "INSERT INTO files (file_id, size, stored, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(file_id) DO UPDATE SET
                 stored = MAX(files.stored, excluded.stored), size = excluded.size",
        )
        .map_err(|error| error.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    for status in statuses {
        statement
            .execute(params![status.file_id, status.size as i64, status.stored, now])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The cached view of [`stored_file_ids`], loaded from the table on first
/// use. All callers hold the history lock, so `&mut HistoryData` is free to
/// fill in.
pub(crate) fn history_stored_ids(
    state: &AppState,
    history: &mut HistoryData,
) -> Result<HashSet<String>, String> {
    if history.stored_file_ids.is_none() {
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        let ids = state.with_database(&path, |connection| stored_file_ids(connection))?;
        history.stored_file_ids = Some(ids);
    }
    Ok(history
        .stored_file_ids
        .as_ref()
        .expect("the set was loaded above when absent")
        .clone())
}

/// Of the ids the durable history references, those with no confirmed pool
/// answer yet — the frontend's batch for the next `/files/query`. By default
/// that means "no row at all"; `recheck_unstored` also re-asks ids whose last
/// answer was `stored = 0`, so a reconnect heals a `file.available` push that
/// was missed while offline. Also refreshes the stored-id cache while the
/// connection is open; the reads are one pass.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn find_unknown_file_ids(
    state: State<'_, AppState>,
    recheck_unstored: Option<bool>,
) -> Result<Vec<String>, String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let referenced = super::query::derived_history_file_ids(&state, &mut history)?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    let (known, stored) = state.with_database(&path, |connection| {
        Ok((all_file_ids(connection)?, stored_file_ids(connection)?))
    })?;
    history.stored_file_ids = Some(stored.clone());
    let answered = if recheck_unstored.unwrap_or(false) { stored } else { known };
    Ok(referenced.difference(&answered).cloned().collect())
}

/// Persists a `/files/query` batch. Ids the server reports as not stored are
/// only ever ids we had no row for, so extending the cache with the stored
/// ones keeps it truthful.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn upsert_server_files(
    state: State<'_, AppState>,
    statuses: Vec<FileStatusInput>,
) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&path, |connection| upsert_status_rows(connection, &statuses))?;
    if let Some(stored) = &mut history.stored_file_ids {
        for status in &statuses {
            if status.stored {
                stored.insert(status.file_id.clone());
            }
        }
    }
    Ok(())
}
