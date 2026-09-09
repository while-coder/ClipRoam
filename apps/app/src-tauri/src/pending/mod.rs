//! 持久上传队列（pending）：捕获与同步之间的唯一缓冲，与 entries 表零关联。
//!
//! 四个操作：
//! - **Enqueue**：[`enqueue_pending_entry`]，捕获时入队；
//! - **Peek**：[`peek_pending_entry`]，取最早一行；files 行先把 sha256
//!   解析写回本行，失败行用 `skip_seqs` 暂时跳过；
//! - **Dequeue**：[`dequeue_pending_entry`]，同步成功后删除该行；
//! - **List**：[`list_pending_entries`]，待同步视图的展示数据。
//!
//! 发布成功后新条目由服务器 `clipboard.created` 回显入库（服务器 id）。

mod resolve;

pub(crate) use resolve::resolve_entry_files;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::content::{refresh_summary, ClipboardEntry, ClipboardEntryExtra, LocalSources};
use crate::entry::lightweight_entry;
use crate::store::history_path_for_key;
use crate::{active_cache_dir, AppState};

use rusqlite::{params, Connection};

// ---------------------------------------------------------------------------
// Enqueue
// ---------------------------------------------------------------------------

/// 入队一条捕获 payload，返回该行的 seq。
pub fn enqueue_pending_entry(
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

/// 建队列表。旧结构直接重建。打开历史库时调用。
pub(crate) fn init_table(connection: &Connection) -> Result<(), String> {
    let columns = crate::store::table_columns(connection, "pending_entries")?;
    let outdated = !columns.is_empty()
        && (!columns.iter().any(|name| name == "content")
            || !columns.iter().any(|name| name == "sources"));
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
    Ok(())
}

/// 待同步视图里队列行的显示 id：`p` + seq，不进 entries 表。
pub fn temp_entry_id(seq: i64) -> String {
    format!("p{seq}")
}

// ---------------------------------------------------------------------------
// 存储
// ---------------------------------------------------------------------------

/// 一行队列数据的存储形态。
#[derive(Clone)]
pub(crate) struct PendingRow {
    pub(crate) seq: i64,
    pub(crate) kind: String,
    pub(crate) content: String,
    pub(crate) extra: String,
    pub(crate) sources: String,
    pub(crate) created_at: String,
}

/// 全部队列行，最早在前。Peek 与 List 共用。
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

/// 队列行转条目视图：显示 id 为 `p{seq}`，payload 取自行本身。
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

/// 给同步客户端的一个可发布队列行。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingRowView {
    seq: i64,
    kind: String,
    content: String,
    extra: serde_json::Value,
}

/// 取最早的可发布行，队列为空时返回 `None`。`skip_seqs` 里的行原地跳过
/// （不删，等待重试）。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn peek_pending_entry(
    app: tauri::AppHandle,
    skip_seqs: Option<Vec<i64>>,
) -> Result<Option<PendingRowView>, String> {
    let skip = skip_seqs.unwrap_or_default();
    let state = app.state::<AppState>();
    // files 行解析前后各读一次队列，读时只短暂持锁。
    let read = |state: &tauri::State<'_, AppState>| -> Result<Vec<PendingRow>, String> {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&path, |connection| list_rows(connection))
    };
    for row in read(&state)? {
        if skip.contains(&row.seq) {
            continue;
        }
        if row.kind == "files" {
            // 解析大目录耗时，期间不持锁；结果写回本行，所以要重读。
            resolve_entry_files(&app, row.seq)?;
            let Some(resolved) = read(&state)?.into_iter().find(|resolved| resolved.seq == row.seq)
            else {
                continue;
            };
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

/// 行内 extra 转发布 payload；损坏的行发空 extra，不卡住后面的队列。
fn publish_extra(extra: &str) -> serde_json::Value {
    serde_json::from_str(extra).unwrap_or_else(|_| {
        serde_json::json!({ "html": null, "rtf": null, "fileInfo": null, "imageInfo": null })
    })
}

// ---------------------------------------------------------------------------
// Dequeue
// ---------------------------------------------------------------------------

/// 删除一行队列。payload 只在本行，删即全部删除。
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

/// 全部队列行，条目形态——待同步视图与侧边栏角标的数据。
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
