//! 持久上传队列（pending）：捕获与同步之间的唯一缓冲，与 entries 表零关联。
//!
//! 队列只有四个操作：
//! - **Enqueue**：[`enqueue`]，捕获时追加一行（`sources` 一并入队，供
//!   `files` 行稍后解析内容 id）；
//! - **Peek**：[`next_pending_entry`]，同步 drain 取最早一行，`files` 行先
//!   经 [`resolve_entry_files`] 把 sha256 写回本行；失败的行通过 `skip_seqs`
//!   暂时跳过（不删，重连后重试）；
//! - **Dequeue**：[`dequeue_pending_entry`]，同步成功后清理该行；用户在待
//!   同步页手动删除也走这里（payload 只存在于本行，删即全部删除）；
//! - **List**：[`list_pending_entries`]，待同步视图的纯展示读取。
//!
//! 发布成功后服务器把新条目经 `clipboard.created` 回显入库（服务器 id），
//! 本地不再换绑任何 id。

mod resolve;

pub(crate) use resolve::resolve_entry_files;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::content::{refresh_summary, ClipboardEntry, ClipboardEntryExtra, LocalSources};
use crate::entry::lightweight_entry;
use crate::store::history_path_for_key;
use crate::{active_cache_dir, AppState};

use rusqlite::{params, Connection};

/// Display identity of a queue row in the pending-sync view: `p` + seq. It
/// never enters the `entries` table — the server assigns the real id, which
/// arrives through the publish echo.
pub fn temp_entry_id(seq: i64) -> String {
    format!("p{seq}")
}

// ---------------------------------------------------------------------------
// Enqueue
// ---------------------------------------------------------------------------

/// Appends one capture payload to the queue; the returned seq names the row
/// until it is dequeued.
pub fn enqueue(
    connection: &Connection,
    kind: &str,
    content: &str,
    extra: &str,
    sources: &str,
    created_at: &str,
) -> Result<i64, String> {
    connection
        .execute(
            "INSERT INTO pending_entries (kind, content, extra, sources, created_at) VALUES (?, ?, ?, ?, ?)",
            params![kind, content, extra, sources, created_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(connection.last_insert_rowid())
}

/// Creates the queue table, backfills the `sources` column, and folds any
/// leftover temporary rows from the old capture design back into the queue.
/// `open_history_database` calls this once per open.
pub(crate) fn init_table(connection: &Connection) -> Result<(), String> {
    // Databases written by older builds queued `(seq, entry_id, queued_at)`
    // references instead of the payload; that data is not migrated — the
    // table is recreated.
    let columns = crate::store::table_columns(connection, "pending_entries")?;
    let outdated = !columns.iter().any(|name| name == "content");
    if outdated {
        connection
            .execute("DROP TABLE IF EXISTS pending_entries", [])
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS pending_entries (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                content TEXT NOT NULL,
                extra TEXT NOT NULL DEFAULT '{}',
                sources TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            );",
        )
        .map_err(|error| error.to_string())?;
    let columns = crate::store::table_columns(connection, "pending_entries")?;
    if !columns.iter().any(|name| name == "sources") {
        connection
            .execute(
                "ALTER TABLE pending_entries ADD COLUMN sources TEXT NOT NULL DEFAULT '{}'",
                [],
            )
            .map_err(|error| error.to_string())?;
    }
    migrate_legacy_temp_rows(connection)
}

/// Old builds kept unpublished captures as temporary `p<seq>` rows in
/// `entries` and re-keyed them on publish. That coupling is gone; any rows a
/// previous version left behind become queue rows so their payloads survive.
fn migrate_legacy_temp_rows(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare("SELECT kind, content, extra, sources, created_at FROM entries WHERE id LIKE 'p%'")
        .map_err(|error| error.to_string())?;
    let legacy = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>("kind")?,
                row.get::<_, String>("content")?,
                row.get::<_, String>("extra")?,
                row.get::<_, String>("sources")?,
                row.get::<_, String>("created_at")?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(statement);
    for (kind, content, extra, sources, created_at) in legacy {
        enqueue(connection, &kind, &content, &extra, &sources, &created_at)?;
    }
    connection
        .execute("DELETE FROM entries WHERE id LIKE 'p%'", [])
        .map_err(|error| error.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 存储：行读取与行→条目视图
// ---------------------------------------------------------------------------

/// One queue row as stored.
#[derive(Clone)]
pub(crate) struct PendingRow {
    pub(crate) seq: i64,
    pub(crate) kind: String,
    pub(crate) content: String,
    pub(crate) extra: String,
    pub(crate) sources: String,
    pub(crate) created_at: String,
}

/// Every queue row, oldest first. Shared by Peek and List.
pub(crate) fn list_rows(connection: &Connection) -> Result<Vec<PendingRow>, String> {
    let mut statement = connection
        .prepare("SELECT seq, kind, content, extra, sources, created_at FROM pending_entries ORDER BY seq ASC")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(PendingRow {
                seq: row.get("seq")?,
                kind: row.get("kind")?,
                content: row.get("content")?,
                extra: row.get("extra")?,
                sources: row.get("sources")?,
                created_at: row.get("created_at")?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

/// The queue row as an entry-shaped view: the display id is `p{seq}` and the
/// payload comes straight from the row.
pub(crate) fn row_entry(row: &PendingRow) -> ClipboardEntry {
    let extra: ClipboardEntryExtra = serde_json::from_str(&row.extra).unwrap_or_default();
    let sources: LocalSources = serde_json::from_str(&row.sources).unwrap_or_default();
    ClipboardEntry {
        id: temp_entry_id(row.seq),
        kind: row.kind.clone(),
        content: row.content.clone(),
        html: extra.html,
        rtf: extra.rtf,
        file_info: extra.file_info,
        image_info: extra.image_info,
        source_device_id: String::new(),
        created_at: row.created_at.clone(),
        sources,
        summary: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Peek
// ---------------------------------------------------------------------------

/// One publishable queue row for the sync client. Capture already wrote the
/// complete payload, and files resolution folds its result back into the row,
/// so the publish request needs nothing else.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingRowView {
    seq: i64,
    kind: String,
    content: String,
    extra: serde_json::Value,
}

/// The oldest publishable row, or `None` when the queue is empty. Rows whose
/// seq is in `skip_seqs` are left in place for a later retry — a row that
/// keeps failing must not block the rest of the queue, and it must not be
/// dropped either: the queue row is the payload's only home.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn next_pending_entry(
    app: tauri::AppHandle,
    skip_seqs: Option<Vec<i64>>,
) -> Result<Option<PendingRowView>, String> {
    let skip = skip_seqs.unwrap_or_default();
    let rows = read_rows(&app.state::<AppState>())?;
    for row in rows {
        if skip.contains(&row.seq) {
            continue;
        }
        if row.kind == "files" {
            // Resolving a large tree can take a while, so the history lock is
            // released for it (`resolve_entry_files` re-locks per batch). The
            // resolved payload is written back to the row, so it is re-read.
            resolve_entry_files(&app, row.seq)?;
            let resolved = read_rows(&app.state::<AppState>())?
                .into_iter()
                .find(|resolved| resolved.seq == row.seq);
            let Some(resolved) = resolved else { continue };
            return Ok(Some(PendingRowView {
                seq: resolved.seq,
                kind: resolved.kind,
                content: resolved.content,
                extra: publish_extra(&resolved.extra),
            }));
        }
        return Ok(Some(PendingRowView {
            seq: row.seq,
            kind: row.kind,
            content: row.content,
            extra: publish_extra(&row.extra),
        }));
    }
    Ok(None)
}

fn read_rows(state: &tauri::State<'_, AppState>) -> Result<Vec<PendingRow>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&path, |connection| list_rows(connection))
}

fn publish_extra(extra: &str) -> serde_json::Value {
    serde_json::from_str(extra).unwrap_or_else(|_| {
        serde_json::json!({ "html": null, "rtf": null, "fileInfo": null, "imageInfo": null })
    })
}

// ---------------------------------------------------------------------------
// Dequeue
// ---------------------------------------------------------------------------

/// Removes one queue row — the sync client after a successful publish, or the
/// user from the pending-sync view. The payload lives nowhere else, so this
/// is the whole delete.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn dequeue_pending_entry(app: AppHandle, state: State<'_, AppState>, seq: i64) -> Result<(), String> {
    {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&path, |connection| {
            connection
                .execute("DELETE FROM pending_entries WHERE seq = ?", params![seq])
                .map_err(|error| error.to_string())
        })?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// Every queue row as an entry-shaped view — the durable pending list behind
/// the sidebar badge and the pending-sync view.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn list_pending_entries(state: State<'_, AppState>) -> Result<Vec<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    let rows = state.with_database(&path, |connection| list_rows(connection))?;
    let mut entries: Vec<ClipboardEntry> = rows.iter().map(row_entry).collect();
    for entry in &mut entries {
        refresh_summary(entry, &history.cached_files, &cache_dir);
    }
    Ok(entries.iter().map(lightweight_entry).collect())
}
