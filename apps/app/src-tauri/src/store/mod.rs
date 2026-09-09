//! Local persistence for the clipboard history.
//!
//! SQLite is the only store: every entry lives in the `entries` table, reads
//! go through SQL, and every mutation writes exactly the rows it touched —
//! there is no in-memory window and no full-table sweep. `files` tracks which
//! content ids the server pool holds; local-cache state is derived from the
//! blob directories on disk, which are the source of truth for it.

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
use chrono::DateTime;

use crate::content::{ClipboardEntry, ClipboardEntryExtra};

pub const LOCAL_HISTORY_KEY: &str = "local";

/// History-level state that is not per-entry: the active profile key, the
/// activation signatures, the device identity and the two content-availability
/// sets. Entry rows live in SQLite alone.
#[derive(Debug)]
pub struct HistoryData {
    pub active_history: String,
    pub last_clipboard: String,
    pub last_file_signature: String,
    pub last_image_signature: String,
    pub device_id: String,
    pub device_name: String,
    /// Content ids this machine has a blob for. Kept in memory so refreshing a
    /// summary never touches the disk.
    pub cached_files: HashSet<String>,
    /// Content ids the server pool already holds, as far as this device knows.
    pub uploaded_files: HashSet<String>,
}

impl Default for HistoryData {
    fn default() -> Self {
        Self {
            active_history: default_active_history(),
            last_clipboard: String::new(),
            last_file_signature: String::new(),
            last_image_signature: String::new(),
            device_id: Uuid::new_v4().to_string(),
            device_name: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "This device".to_string()),
            cached_files: HashSet::new(),
            uploaded_files: HashSet::new(),
        }
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

pub(crate) fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>, String> {
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
    // Pinning, source_app and the files bookkeeping columns were removed:
    // they are dropped from databases written by older builds instead of
    // being recreated.
    let entry_columns = table_columns(&connection, "entries")?;
    let files_columns = table_columns(&connection, "files")?;
    let mut schema = String::new();
    // Dropping an indexed column fails, so the index goes first.
    schema.push_str("DROP INDEX IF EXISTS entries_source_app_created_at;\n");
    // Timestamps are ordered by the numeric `created_ms` column (string RFC3339
    // orders wrongly across variable-length subsecond digits and `Z`/`+00:00`).
    schema.push_str("DROP INDEX IF EXISTS entries_created_at;\n");
    schema.push_str("DROP INDEX IF EXISTS entries_kind_created_at;\n");
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
            created_ms INTEGER NOT NULL DEFAULT 0,
            source_device_id TEXT NOT NULL,
            sources TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS entries_created_ms ON entries(created_ms DESC);
        CREATE INDEX IF NOT EXISTS entries_kind_created_ms ON entries(kind, created_ms DESC);
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
    // The durable upload queue owns its table: schema, stale-row sweep and
    // row CRUD all live in the `pending` module.
    crate::pending::init_table(&connection)?;
    // Databases written before `created_ms` existed get the column added and
    // backfilled here; fresh databases created it in the schema above. Columns
    // are re-read after the schema batch so a crash between the ALTER and the
    // backfill resumes cleanly instead of failing on a duplicate column. The
    // timestamps come from two clocks (local `Utc::now().to_rfc3339()` and the
    // server's publish response), so the parse runs in Rust instead of relying
    // on SQLite date functions to accept every RFC3339 spelling.
    let stamped = table_columns(&connection, "entries")?
        .iter()
        .any(|name| name == "created_ms");
    if !stamped {
        connection
            .execute("ALTER TABLE entries ADD COLUMN created_ms INTEGER", [])
            .map_err(|error| error.to_string())?;
        let unstamped = {
            let mut statement = connection
                .prepare("SELECT id, created_at FROM entries WHERE created_ms IS NULL")
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            rows
        };
        let mut stamp = connection
            .prepare("UPDATE entries SET created_ms = ? WHERE id = ?")
            .map_err(|error| error.to_string())?;
        for (id, created_at) in unstamped {
            stamp
                .execute(params![entry_created_ms(&created_at), id])
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(connection)
}

/// Millisecond timestamp of an RFC3339 `created_at`; 0 when unparseable, so a
/// malformed row sorts last instead of poisoning the index with NULLs.
pub fn entry_created_ms(created_at: &str) -> i64 {
    DateTime::parse_from_rfc3339(created_at)
        .map(|time| time.timestamp_millis())
        .unwrap_or(0)
}

const ENTRY_COLUMNS: &str = "id, kind, content, extra, created_at, source_device_id, sources";

/// Builds a `ClipboardEntry` from one `entries` row. `summary` is derived
/// state and starts empty — recompute with `refresh_summary` before the entry
/// leaves the backend.
pub fn entry_from_row(row: &rusqlite::Row) -> rusqlite::Result<ClipboardEntry> {
    let extra =
        serde_json::from_str::<ClipboardEntryExtra>(&row.get::<_, String>("extra")?).unwrap_or_default();
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
}

/// Reads entries. `where_sql` and `suffix_sql` are appended after the column
/// list; both are built only from literals and interpolated integers inside
/// this crate, never from user input (values travel as `?` parameters).
pub fn select_entries(
    connection: &Connection,
    where_sql: &str,
    suffix_sql: &str,
    values: &[rusqlite::types::Value],
) -> Result<Vec<ClipboardEntry>, String> {
    let sql = format!("SELECT {ENTRY_COLUMNS} FROM entries {where_sql}{suffix_sql}");
    let mut statement = connection.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params_from_iter(values), entry_from_row)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

/// Reads one entry by id; absent ids come back as `None`.
pub fn select_entry(
    connection: &Connection,
    entry_id: &str,
) -> Result<Option<ClipboardEntry>, String> {
    Ok(select_entries(
        connection,
        "WHERE id = ?",
        "",
        &[rusqlite::types::Value::Text(entry_id.to_string())],
    )?
    .into_iter()
    .next())
}

pub fn count_entries(
    connection: &Connection,
    where_sql: &str,
    values: &[rusqlite::types::Value],
) -> Result<usize, String> {
    let sql = format!("SELECT COUNT(*) FROM entries {where_sql}");
    connection
        .query_row(&sql, params_from_iter(values), |row| row.get::<_, i64>(0))
        .map(|count| count.max(0) as usize)
        .map_err(|error| error.to_string())
}

/// Newest-first ordering with the stable same-millisecond tie-break (rows
/// inserted later within one millisecond sort first, matching the capture
/// path's insert-at-front). The optional page limit is interpolated as an
/// integer only.
pub fn newest_first_sql(limit: Option<usize>, offset: usize) -> String {
    let mut sql = " ORDER BY created_ms DESC, rowid DESC".to_string();
    if let Some(limit) = limit {
        sql.push_str(&format!(" LIMIT {limit} OFFSET {offset}"));
    }
    sql
}

pub fn select_all_entry_ids(connection: &Connection) -> Result<Vec<String>, String> {
    let sql = format!("SELECT id FROM entries{}", newest_first_sql(None, 0));
    let mut statement = connection.prepare(&sql).map_err(|error| error.to_string())?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(ids)
}

pub fn load_history(path: &Path, key: &str) -> HistoryData {
    let mut history = HistoryData {
        active_history: key.to_string(),
        ..HistoryData::default()
    };
    let Ok(connection) = open_history_database(path) else {
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
                    _ => {}
                }
            }
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
    history
}

pub fn cache_dir_for_path(path: &Path) -> PathBuf {
    path.parent()
        .expect("history file always has a parent directory")
        .join("files")
}

/// Writes one entry row, replacing any row with the same id. The full extra
/// payload (rich text, trees, thumbnails) rides the row write, so hashing
/// results and remote updates need no separate pass.
pub fn upsert_entry_row(connection: &Connection, entry: &ClipboardEntry) -> Result<(), String> {
    let extra = ClipboardEntryExtra::of(entry).json()?;
    let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT OR REPLACE INTO entries (id, kind, content, extra, created_at, created_ms, source_device_id, sources) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                entry.id,
                entry.kind,
                entry.content,
                extra,
                entry.created_at,
                entry_created_ms(&entry.created_at),
                entry.source_device_id,
                sources,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Removes entry rows by id; absent ids are ignored.
pub fn delete_entries_by_ids(connection: &Connection, entry_ids: &[String]) -> Result<(), String> {
    if entry_ids.is_empty() {
        return Ok(());
    }
    let marks = placeholders(entry_ids.len());
    connection
        .execute(
            &format!("DELETE FROM entries WHERE id IN ({marks})"),
            params_from_iter(entry_ids),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Comma-separated `?` marks for an IN clause.
pub fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count).collect::<Vec<_>>().join(", ")
}

/// Persists the history-level metadata: the activation signatures and the
/// device identity.
pub fn save_metadata(connection: &Connection, history: &HistoryData) -> Result<(), String> {
    let metadata = vec![
        ("last_clipboard", history.last_clipboard.clone()),
        ("last_file_signature", history.last_file_signature.clone()),
        ("last_image_signature", history.last_image_signature.clone()),
        ("device_id", history.device_id.clone()),
        ("device_name", history.device_name.clone()),
    ];
    for (key, value) in metadata {
        connection
            .execute(
                "INSERT INTO metadata (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}


pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
