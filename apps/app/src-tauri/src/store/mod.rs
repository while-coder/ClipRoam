//! Local persistence for the clipboard history.
//!
//! Entry metadata, trees and local sources share one row. A flush writes only
//! the rows the mutation actually touched; a mark-sweep keeps the `entries`
//! and `pending_entries` tables aligned with the in-memory working set by
//! deleting rows the history no longer holds. `files` tracks which content ids
//! the server pool holds; local-cache state is derived from the blob
//! directories on disk, which are the source of truth for it.

mod cache;

pub use cache::{
    cached_hash, cached_source_for, collect_local_garbage, mark_files_uploaded, remember_hash,
    scan_cached_blobs,
};

use rusqlite::{params, params_from_iter, Connection};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use crate::content::{refresh_summary, ClipboardEntry, ClipboardEntryExtra};

pub const LOCAL_HISTORY_KEY: &str = "local";

#[derive(Debug)]
pub struct HistoryData {
    pub histories: HashMap<String, Vec<ClipboardEntry>>,
    pub active_history: String,
    pub last_clipboard: String,
    pub last_file_signature: String,
    pub last_image_signature: String,
    pub device_id: String,
    pub device_name: String,
    /// Locally initiated deletions are kept until the server echoes the
    /// deletion, otherwise an offline delete would be restored on reconnect.
    pub pending_deletions: HashSet<String>,
    /// Content ids this machine has a blob for. Kept in memory so refreshing a
    /// summary never touches the disk.
    pub cached_files: HashSet<String>,
    /// Content ids the server pool already holds, as far as this device knows.
    pub uploaded_files: HashSet<String>,
}

impl Default for HistoryData {
    fn default() -> Self {
        Self {
            histories: HashMap::new(),
            active_history: default_active_history(),
            last_clipboard: String::new(),
            last_file_signature: String::new(),
            last_image_signature: String::new(),
            device_id: Uuid::new_v4().to_string(),
            device_name: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "This device".to_string()),
            pending_deletions: HashSet::new(),
            cached_files: HashSet::new(),
            uploaded_files: HashSet::new(),
        }
    }
}

impl HistoryData {
    pub fn active_entries(&self) -> &[ClipboardEntry] {
        self.histories
            .get(&self.active_history)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn active_entries_mut(&mut self) -> &mut Vec<ClipboardEntry> {
        self.histories.entry(self.active_history.clone()).or_default()
    }

    pub fn find(&self, entry_id: &str) -> Option<&ClipboardEntry> {
        self.active_entries()
            .iter()
            .find(|entry| entry.id == entry_id)
    }

    pub fn find_mut(&mut self, entry_id: &str) -> Option<&mut ClipboardEntry> {
        self.active_entries_mut()
            .iter_mut()
            .find(|entry| entry.id == entry_id)
    }
}

pub fn default_active_history() -> String {
    LOCAL_HISTORY_KEY.to_string()
}

pub fn history_path_for_key(histories_dir: &Path, key: &str) -> PathBuf {
    histories_dir
        .join(format!(
            "{}-{:016x}",
            safe_history_directory_name(key),
            crate::content::fnv1a(key.bytes())
        ))
        .join("history.sqlite")
}

pub fn cache_dir_for(histories_dir: &Path, key: &str) -> PathBuf {
    cache_dir_for_path(&history_path_for_key(histories_dir, key))
}

fn safe_history_directory_name(key: &str) -> String {
    let name = key
        .strip_prefix("account:")
        .unwrap_or(key)
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if name.is_empty() {
        "local".to_string()
    } else {
        name
    }
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .map_err(|error| error.to_string())?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(columns)
}

/// One SQLite connection per history database, reused across writes. Opening
/// a connection re-runs the whole schema migration, so call sites take the
/// pooled connection instead of reopening on every statement.
#[derive(Default)]
pub struct DatabasePool {
    connections: HashMap<PathBuf, Connection>,
}

impl DatabasePool {
    pub fn connection(&mut self, path: &Path) -> Result<&mut Connection, String> {
        if !self.connections.contains_key(path) {
            let connection = open_history_database(path)?;
            self.connections.insert(path.to_path_buf(), connection);
        }
        Ok(self
            .connections
            .get_mut(path)
            .expect("connection was inserted above"))
    }
}

pub fn open_history_database(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
        .map_err(|error| error.to_string())?;
    // The durable upload queue carries the full capture payload, so the sync
    // client can publish straight from it without re-reading `entries`.
    // Databases written by older builds queued `(seq, entry_id, queued_at)`
    // references instead; that data is not migrated — the table is recreated.
    let recreate_pending_entries = !table_columns(&connection, "pending_entries")?
        .iter()
        .any(|name| name == "content");
    // Pinning, source_app and the files bookkeeping columns were removed:
    // they are dropped from databases written by older builds instead of
    // being recreated.
    let entry_columns = table_columns(&connection, "entries")?;
    let files_columns = table_columns(&connection, "files")?;
    let mut schema = String::new();
    if recreate_pending_entries {
        schema.push_str("DROP TABLE IF EXISTS pending_entries;\n");
    }
    // Dropping an indexed column fails, so the index goes first.
    schema.push_str("DROP INDEX IF EXISTS entries_source_app_created_at;\n");
    for column in ["pinned", "source_app"] {
        if entry_columns.iter().any(|name| name == column) {
            schema.push_str(&format!("ALTER TABLE entries DROP COLUMN {column};\n"));
        }
    }
    if files_columns.iter().any(|name| name == "available") {
        schema.push_str("ALTER TABLE files RENAME COLUMN available TO stored;\n");
    }
    for column in ["cached", "size"] {
        if files_columns.iter().any(|name| name == column) {
            schema.push_str(&format!("ALTER TABLE files DROP COLUMN {column};\n"));
        }
    }
    // Rows that only carried the removed local-cache flag mean nothing now;
    // every remaining row marks content the server pool holds. A fresh
    // database has no files table until the schema below creates it.
    if !files_columns.is_empty() {
        schema.push_str("DELETE FROM files WHERE stored = 0;\n");
    }
    schema.push_str(
        "
        CREATE TABLE IF NOT EXISTS pending_entries (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            extra TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS entries (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            extra TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            source_device_id TEXT NOT NULL,
            sources TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS entries_created_at ON entries(created_at DESC);
        CREATE INDEX IF NOT EXISTS entries_kind_created_at ON entries(kind, created_at DESC);
        CREATE TABLE IF NOT EXISTS files (
            file_id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            stored INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS hash_cache (
            source TEXT NOT NULL,
            size INTEGER NOT NULL,
            modified_at INTEGER NOT NULL,
            hash TEXT NOT NULL,
            PRIMARY KEY (source, size, modified_at)
        );
        ",
    );
    connection
        .execute_batch(&schema)
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

pub fn load_history(path: &Path, key: &str) -> HistoryData {
    let mut history = HistoryData {
        active_history: key.to_string(),
        ..HistoryData::default()
    };
    let Ok(connection) = open_history_database(path) else {
        history.histories.insert(key.to_string(), Vec::new());
        return history;
    };

    if let Ok(mut statement) = connection.prepare("SELECT key, value FROM metadata") {
        if let Ok(rows) = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) {
            for (name, value) in rows.flatten() {
                match name.as_str() {
                    "last_clipboard" => history.last_clipboard = value,
                    "last_file_signature" => history.last_file_signature = value,
                    "last_image_signature" => history.last_image_signature = value,
                    "device_id" => history.device_id = value,
                    "device_name" => history.device_name = value,
                    "pending_deletions" => {
                        history.pending_deletions = serde_json::from_str(&value).unwrap_or_default()
                    }
                    _ => {}
                }
            }
        }
    }

    let mut entries = Vec::new();
    if let Ok(mut statement) = connection.prepare(
        "SELECT id, kind, content, extra, source_device_id, created_at, sources FROM entries ORDER BY created_at DESC",
    ) {
        if let Ok(rows) = statement.query_map([], |row| {
            let extra = serde_json::from_str::<ClipboardEntryExtra>(&row.get::<_, String>("extra")?).unwrap_or_default();
            Ok(ClipboardEntry {
                id: row.get("id")?,
                kind: row.get("kind")?,
                content: row.get("content")?,
                html: extra.html,
                rtf: extra.rtf,
                file_info: extra.file_info,
                image_info: extra.image_info,
                source_device_id: row.get("source_device_id")?,
                created_at: row.get("created_at")?,
                summary: Default::default(),
                sources: serde_json::from_str(&row.get::<_, String>("sources")?).unwrap_or_default(),
            })
        }) {
            entries.extend(rows.flatten());
        }
    }

    // Every row marks content the server pool holds; the migration has
    // already swept rows that only carried the old local-cache flag.
    let mut uploaded_files = HashSet::new();
    if let Ok(mut statement) = connection.prepare("SELECT file_id FROM files") {
        if let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) {
            for file_id in rows.flatten() {
                uploaded_files.insert(file_id);
            }
        }
    }

    let cache_dir = cache_dir_for_path(path);
    history.uploaded_files = uploaded_files;
    history.cached_files = scan_cached_blobs(&cache_dir);
    history.histories.insert(key.to_string(), entries);
    refresh_summaries(&mut history, &cache_dir);
    history
}

pub fn cache_dir_for_path(path: &Path) -> PathBuf {
    path.parent()
        .expect("history file always has a parent directory")
        .join("files")
}

pub fn refresh_summaries(history: &mut HistoryData, cache_dir: &Path) {
    refresh_history_summaries(history, cache_dir, None);
}

pub fn refresh_entry_summary(history: &mut HistoryData, entry_id: &str, cache_dir: &Path) {
    refresh_history_summaries(history, cache_dir, Some(entry_id));
}

fn refresh_history_summaries(history: &mut HistoryData, cache_dir: &Path, only: Option<&str>) {
    let HistoryData {
        histories,
        active_history,
        cached_files,
        uploaded_files,
        ..
    } = history;
    let Some(entries) = histories.get_mut(active_history) else {
        return;
    };
    for entry in entries.iter_mut() {
        if only.is_some_and(|entry_id| entry_id != entry.id) {
            continue;
        }
        refresh_summary(entry, cached_files, uploaded_files, cache_dir);
    }
}

/// Flushes the in-memory history into its SQLite projection: the sweeps that
/// keep the `entries` and `pending_entries` tables aligned with the working
/// set, history-level metadata, and the entry rows the mutation actually
/// touched. Rows outside `upserts` are only ever deleted by the sweep, never
/// rewritten, so a steady-state change stays O(touched rows) instead of
/// O(history).
pub fn flush_history(
    connection: &mut Connection,
    history: &HistoryData,
    upserts: &[&ClipboardEntry],
) -> Result<(), String> {
    let transaction = connection.transaction().map_err(|error| error.to_string())?;
    let entry_ids = history
        .active_entries()
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    if entry_ids.is_empty() {
        transaction
            .execute("DELETE FROM entries", [])
            .map_err(|error| error.to_string())?;
    } else {
        let placeholders = std::iter::repeat_n("?", entry_ids.len()).collect::<Vec<_>>().join(", ");
        transaction
            .execute(
                &format!("DELETE FROM entries WHERE id NOT IN ({placeholders})"),
                params_from_iter(entry_ids),
            )
            .map_err(|error| error.to_string())?;
    }
    for entry in upserts {
        // The full extra payload (rich text, trees, thumbnails) rides the row
        // write, so hashing results and remote updates need no separate pass.
        let extra = ClipboardEntryExtra::of(entry).json()?;
        let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO entries (id, kind, content, extra, created_at, source_device_id, sources) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    entry.id,
                    entry.kind,
                    entry.content,
                    extra,
                    entry.created_at,
                    entry.source_device_id,
                    sources,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    // Every path that removes or re-keys an entry goes through this flush: the
    // publish swap, content dedup and deletions all drop the queue row that no
    // longer has a matching temporary-id entry. This runs after the rows above
    // are written, so freshly captured entries keep theirs.
    transaction
        .execute(
            "DELETE FROM pending_entries WHERE 'p' || seq NOT IN (SELECT id FROM entries)",
            [],
        )
        .map_err(|error| error.to_string())?;
    let mut metadata = vec![
        ("last_clipboard", history.last_clipboard.clone()),
        ("last_file_signature", history.last_file_signature.clone()),
        ("last_image_signature", history.last_image_signature.clone()),
        ("device_id", history.device_id.clone()),
        ("device_name", history.device_name.clone()),
    ];
    metadata.push((
        "pending_deletions",
        serde_json::to_string(&history.pending_deletions).map_err(|error| error.to_string())?,
    ));
    for (key, value) in metadata {
        transaction
            .execute(
                "INSERT INTO metadata (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

pub fn retain_single_history(history: &mut HistoryData, key: &str) {
    history.histories.retain(|name, _| name == key);
    history.active_history = key.to_string();
}

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// 持久上传队列：完整捕获载荷按序追加，发布/清理或显式确认时移除，
// 离线捕获在下次连接时按序重放。
// ---------------------------------------------------------------------------

/// Temporary, pre-publish entry identity. The server assigns the real id on
/// first publish and `apply_published_entry` swaps it in, so this only has to
/// stay stable until then. The `p` prefix keeps it from colliding with the
/// server's numeric ids.
pub fn temp_entry_id(seq: i64) -> String {
    format!("p{seq}")
}

/// Parses a temporary id back into its queue row seq.
pub fn temp_entry_seq(id: &str) -> Option<i64> {
    id.strip_prefix('p')?.parse::<i64>().ok().filter(|seq| *seq > 0)
}

/// One durable upload-queue row: the full capture payload.
#[derive(Debug)]
pub struct PendingQueueRow {
    pub seq: i64,
    pub kind: String,
    pub content: String,
    pub extra: String,
    pub created_at: String,
}

/// Durable upload queue. Rows are appended in capture order with the complete
/// entry payload, and removed by the `flush_history` sweep (publish swap,
/// dedup, deletion) or an explicit acknowledge, so an offline capture
/// replays in order on the next connection.
pub fn enqueue_pending_entry(
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

/// Recreates the queue row an existing temporary-id entry should have, for the
/// rare case where the entry survived but its row did not. A row already
/// occupying the seq is kept.
pub fn ensure_pending_entry(
    connection: &Connection,
    seq: i64,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT OR IGNORE INTO pending_entries (seq, kind, content, extra, created_at) VALUES (?, ?, ?, ?, ?)",
            params![seq, kind, content, extra, created_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Folds updated payload (resolved content ids after hashing) back into the
/// queue row, recreating it if the sweep removed it in the meantime.
pub fn update_pending_entry(
    connection: &Connection,
    seq: i64,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO pending_entries (seq, kind, content, extra, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(seq) DO UPDATE SET kind = excluded.kind, content = excluded.content, extra = excluded.extra",
            params![seq, kind, content, extra, created_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn list_pending_rows(connection: &Connection) -> Result<Vec<PendingQueueRow>, String> {
    let mut statement = connection
        .prepare("SELECT seq, kind, content, extra, created_at FROM pending_entries ORDER BY seq ASC")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(PendingQueueRow {
                seq: row.get("seq")?,
                kind: row.get("kind")?,
                content: row.get("content")?,
                extra: row.get("extra")?,
                created_at: row.get("created_at")?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

/// Returns whether a row was actually removed.
pub fn acknowledge_pending_entry(connection: &Connection, seq: i64) -> Result<bool, String> {
    let changed = connection
        .execute(
            "DELETE FROM pending_entries WHERE seq = ?",
            params![seq],
        )
        .map_err(|error| error.to_string())?;
    Ok(changed > 0)
}
