//! 持久上传队列（pending）：捕获与同步之间的唯一缓冲。
//!
//! - 文本与图片在捕获时就写出完整 extra（与 entries 行的 extra 完全相等），
//!   入队即可发布；
//! - files 只写捕获现场的简单 extra（`f: ""` 占位树），sha256 由
//!   [`resolve`]（本目录 `resolve.rs`）在同步轮到该行时才计算；
//! - 处理严格串行：每次只取最早的一行，发布成功后确认删除，再取下一行。

mod resolve;

pub(crate) use resolve::resolve_entry_files;

use serde::Serialize;
use tauri::State;

use crate::content::{ClipboardEntry, ClipboardEntryExtra};
use crate::store::{history_path_for_key, select_entry};
use crate::AppState;

use rusqlite::{params, params_from_iter, Connection};

/// Temporary, pre-publish entry identity: the seq of the capture's queue row.
/// The server assigns the real id on first publish and
/// `apply_published_entry` swaps it in, so this only has to stay stable until
/// then. The `p` prefix keeps it from colliding with the server's numeric ids.
pub fn temp_entry_id(seq: i64) -> String {
    format!("p{seq}")
}

/// Parses a temporary id back into its queue row seq.
pub fn temp_entry_seq(id: &str) -> Option<i64> {
    id.strip_prefix('p')?.parse::<i64>().ok().filter(|seq| *seq > 0)
}

/// Appends one capture payload to the queue; the returned seq names the local
/// entry until the server's id is adopted.
pub fn enqueue(
    connection: &Connection,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<i64, String> {
    connection
        .execute(
            "INSERT INTO pending_entries (kind, content, extra, created_at) VALUES (?, ?, ?, ?)",
            params![kind, content, extra, created_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(connection.last_insert_rowid())
}

/// Drops the queue rows belonging to the given entry ids; only temporary
/// (`p<seq>`) ids ever have one. Every path that removes or re-keys an entry
/// calls this — the publish swap, content dedup and deletions.
pub fn delete_rows_for(connection: &Connection, entry_ids: &[String]) -> Result<(), String> {
    let seqs = entry_ids.iter().filter_map(|id| temp_entry_seq(id)).collect::<Vec<_>>();
    if seqs.is_empty() {
        return Ok(());
    }
    let marks = crate::store::placeholders(seqs.len());
    connection
        .execute(
            &format!("DELETE FROM pending_entries WHERE seq IN ({marks})"),
            params_from_iter(seqs),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Returns whether a row was actually removed.
pub fn acknowledge(connection: &Connection, seq: i64) -> Result<bool, String> {
    let changed = connection
        .execute(
            "DELETE FROM pending_entries WHERE seq = ?",
            params![seq],
        )
        .map_err(|error| error.to_string())?;
    Ok(changed > 0)
}

/// Creates the queue table and sweeps stale rows. `open_history_database`
/// calls this once per open: rows whose temporary-id entry is gone (a missed
/// cleanup in a pre-transaction build, or a crash between writes) would
/// otherwise replay forever.
pub(crate) fn init_table(connection: &Connection) -> Result<(), String> {
    // Databases written by older builds queued `(seq, entry_id, queued_at)`
    // references instead of the payload; that data is not migrated — the
    // table is recreated.
    let outdated = !crate::store::table_columns(connection, "pending_entries")?
        .iter()
        .any(|name| name == "content");
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
                created_at TEXT NOT NULL
            );",
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "DELETE FROM pending_entries WHERE 'p' || seq NOT IN (SELECT id FROM entries)",
            [],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 同步客户端视图：串行 drain
// ---------------------------------------------------------------------------

/// One publishable queue row for the sync client. For text and image the
/// extra comes straight from the row — capture already wrote the full payload.
/// For files it comes from the entry after content resolution, since the row
/// only ever holds the capture-time placeholder tree.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingRowView {
    seq: i64,
    kind: String,
    content: String,
    extra: serde_json::Value,
}

/// One queue row as stored: the capture payload plus the seq that names its
/// local entry.
struct PendingRow {
    seq: i64,
    kind: String,
    content: String,
    extra: String,
}

fn list_rows(connection: &Connection) -> Result<Vec<PendingRow>, String> {
    let mut statement = connection
        .prepare("SELECT seq, kind, content, extra FROM pending_entries ORDER BY seq ASC")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(PendingRow {
                seq: row.get("seq")?,
                kind: row.get("kind")?,
                content: row.get("content")?,
                extra: row.get("extra")?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

/// The oldest publishable row, or `None` when the queue is empty. Rows whose
/// entry vanished while waiting — adopted, deduped or deleted — are
/// acknowledged on the way, so the caller never sees one: republishing such a
/// row would land a server entry that `apply_published_entry` then has to
/// delete again. A files row is resolved right here (`resolve_entry_files`),
/// and the resolved entry is the publish payload.
#[tauri::command]
pub(crate) fn next_pending_entry(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<PendingRowView>, String> {
    let rows = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&path, |connection| list_rows(connection))?
    };
    for row in rows {
        let entry_id = temp_entry_id(row.seq);
        if row.kind == "files" {
            // Resolving a large tree can take a while, so the history lock is
            // released for it (`resolve_entry_files` re-locks per batch).
            resolve_entry_files(&app, &entry_id)?;
            let Some(entry) = read_entry(&state, &entry_id)? else {
                // Deleted while the resolution ran: the row is stale now.
                drop_stale_row(&state, row.seq)?;
                continue;
            };
            let extra = ClipboardEntryExtra::of(&entry).json()?;
            return Ok(Some(PendingRowView {
                seq: row.seq,
                kind: entry.kind,
                content: entry.content,
                extra: publish_extra(&extra),
            }));
        }
        // Text and image rows carry the complete capture payload, but the
        // entry must still exist — a publish against a vanished entry would
        // bounce straight back as a server-side deletion.
        if read_entry(&state, &entry_id)?.is_none() {
            drop_stale_row(&state, row.seq)?;
            continue;
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

fn publish_extra(extra: &str) -> serde_json::Value {
    serde_json::from_str(extra).unwrap_or_else(|_| {
        serde_json::json!({ "html": null, "rtf": null, "fileInfo": null, "imageInfo": null })
    })
}

/// One entry row read under the history lock, which also pins the active
/// history the pooled connection is keyed by.
fn read_entry(state: &AppState, entry_id: &str) -> Result<Option<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&path, |connection| select_entry(connection, entry_id))
}

/// Removes a queue row whose entry is gone.
fn drop_stale_row(state: &AppState, seq: i64) -> Result<(), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state
        .with_database(&path, |connection| acknowledge(connection, seq))
        .map(|_| ())
}

/// The frontend confirms a published row so the next one can come up.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn acknowledge_pending_entry(state: State<'_, AppState>, seq: i64) -> Result<(), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state
        .with_database(&path, |connection| acknowledge(connection, seq))?;
    Ok(())
}
