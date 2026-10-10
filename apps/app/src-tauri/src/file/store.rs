//! `files` 表：服务器池可用性的本地持久层。
//!
//! 结构与服务器 FileStore 的同名表一致：`stored = 1` 表示服务器池持有该
//! 内容的字节，`size` 是内容大小。本机 blob 是否在磁盘不进这张表——磁盘
//! 即真相（见 `cache.rs` 的 blob 目录扫描）。表里只有确认过的池状态：id
//! 不在其中 = 从未向服务器查询过（`find_unknown_file_ids` 的「未知」），唯一
//! 写入是前端对未知 id 的 `/files/query` 回填（`upsert_server_files`）——
//! 上传完成与 `file.available` 推送只触发刷新，由该回填持久化。

use std::collections::HashSet;

use rusqlite::{params, params_from_iter, types::Value, Connection};
use serde::Deserialize;
use tauri::State;

use crate::content::entry_contents_of;
use crate::store::select_entries;
use crate::{utils::placeholders, AppState};

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

/// SQLite's host-parameter ceiling is far above this chunk size, so the
/// membership probe never trips it even for a whole-history reference list.
const QUERY_CHUNK: usize = 500;

/// Which of the given ids have a row, and whether it is stored. Same shape as
/// `find_stale_entry_ids`: the candidate list goes into SQLite via IN, so
/// only referenced rows are read back — the table is never dumped whole.
fn file_rows(connection: &Connection, file_ids: &[String]) -> Result<Vec<(String, bool)>, String> {
    let mut rows = Vec::new();
    for chunk in file_ids.chunks(QUERY_CHUNK) {
        let sql = format!(
            "SELECT file_id, stored FROM files WHERE file_id IN ({})",
            placeholders(chunk.len())
        );
        let mut statement = connection.prepare(&sql).map_err(|error| error.to_string())?;
        let values = chunk.iter().map(|id| Value::Text(id.clone())).collect::<Vec<_>>();
        rows.extend(
            statement
                .query_map(params_from_iter(values), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(rows)
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

/// From this page's cached entry details, query files that are unknown or
/// still marked unstored. Confirmed stored files need no repeat HTTP request.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn find_unknown_file_ids(
    state: State<'_, AppState>,
    entry_ids: Vec<String>,
    session_id: String,
) -> Result<Vec<String>, String> {
    let account = state.account(&session_id)?;
    let referenced = entry_file_ids(&account, &entry_ids)?;
    let referenced: Vec<String> = referenced.into_iter().collect();
    let rows = account.with_database(|connection| file_rows(connection, &referenced))?;
    let mut stored = HashSet::new();
    for (file_id, is_stored) in rows {
        if is_stored {
            stored.insert(file_id);
        }
    }
    Ok(referenced.into_iter().filter(|id| !stored.contains(id)).collect())
}

/// Persists a `/files/query` batch. Ids the server reports as not stored are
/// only ever ids we had no row for, so extending the cache with the stored
/// ones keeps it truthful.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn upsert_server_files(
    state: State<'_, AppState>,
    statuses: Vec<FileStatusInput>,
    session_id: String,
) -> Result<(), String> {
    let account = state.account(&session_id)?;
    account.with_database(|connection| upsert_status_rows(connection, &statuses))
}

/// Read full cached details only for this page; file trees stay Rust-side.
fn entry_file_ids(
    account: &crate::account::AccountContext,
    entry_ids: &[String],
) -> Result<HashSet<String>, String> {
    if entry_ids.is_empty() { return Ok(HashSet::new()); }
    let where_sql = format!("WHERE id IN ({})", placeholders(entry_ids.len()));
    let values = entry_ids.iter().cloned().map(Value::Text).collect::<Vec<_>>();
    account.with_database(|connection| {
        let entries = select_entries(connection, &where_sql, "", &values)?;
        Ok(entries.iter().flat_map(entry_contents_of).map(|(id, _)| id).collect())
    })
}
