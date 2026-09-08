//! Local persistence for the clipboard history.
//!
//! Entry metadata, trees and local sources share one row. General history saves
//! patch only presentation fields, while capture, hashing and remote upsert
//! explicitly replace the full entry data. Content availability and local-cache
//! state live once in `files`, keyed by content hash.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

use crate::content::{
    cached_file_path, refresh_summary, tree_contents, ClipboardEntry, ClipboardEntryExtra,
};

pub const MAX_HISTORY_ENTRIES: usize = 200;
pub const LOCAL_HISTORY_KEY: &str = "local";
pub const HASH_CACHE_LIMIT: i64 = 20_000;
pub const DOWNLOAD_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

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
        .join(format!("{}-{:016x}", safe_history_directory_name(key), stable_key_hash(key)))
        .join("history.sqlite")
}

pub fn cache_dir_for(histories_dir: &Path, key: &str) -> PathBuf {
    history_path_for_key(histories_dir, key)
        .parent()
        .expect("history file always has a parent directory")
        .join("files")
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

fn stable_key_hash(key: &str) -> u64 {
    key.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
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
    let recreate_pending_entries = {
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info('pending_entries')")
            .map_err(|error| error.to_string())?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        !names.iter().any(|name| name == "content")
    };
    // Pinning was removed: the column is dropped from databases written by
    // older builds instead of being recreated.
    let has_pinned = {
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info('entries')")
            .map_err(|error| error.to_string())?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        names.iter().any(|name| name == "pinned")
    };
    let mut schema = String::new();
    if recreate_pending_entries {
        schema.push_str("DROP TABLE IF EXISTS pending_entries;\n");
    }
    if has_pinned {
        schema.push_str("ALTER TABLE entries DROP COLUMN pinned;\n");
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
            source_app TEXT NOT NULL DEFAULT '',
            sources TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS entries_created_at ON entries(created_at DESC);
        CREATE INDEX IF NOT EXISTS entries_kind_created_at ON entries(kind, created_at DESC);
        CREATE INDEX IF NOT EXISTS entries_source_app_created_at ON entries(source_app, created_at DESC);
        CREATE TABLE IF NOT EXISTS files (
            file_id TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            available INTEGER NOT NULL DEFAULT 0,
            cached INTEGER NOT NULL DEFAULT 0
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

    let mut uploaded_files = HashSet::new();
    let mut cached_files = HashSet::new();
    if let Ok(mut statement) = connection.prepare("SELECT file_id, available, cached FROM files") {
        if let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>("file_id")?,
                row.get::<_, i64>("available")? != 0,
                row.get::<_, i64>("cached")? != 0,
            ))
        }) {
            for (file_id, available, cached) in rows.flatten() {
                if available {
                    uploaded_files.insert(file_id.clone());
                }
                if cached {
                    cached_files.insert(file_id);
                }
            }
        }
    }

    let cache_dir = cache_dir_for_path(path);
    history.uploaded_files = uploaded_files;
    history.cached_files = cached_files
        .into_iter()
        .filter(|file_id| cached_file_path(&cache_dir, file_id).is_some())
        .collect();
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
        refresh_summary(entry, cached_files, uploaded_files, cache_dir);
    }
}

pub fn refresh_entry_summary(history: &mut HistoryData, entry_id: &str, cache_dir: &Path) {
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
    if let Some(entry) = entries
        .iter_mut()
        .find(|entry| entry.id == entry_id)
    {
        refresh_summary(entry, cached_files, uploaded_files, cache_dir);
    }
}

/// Writes small entry fields plus metadata. Existing trees, local sources and
/// content rows are changed only by `write_entry_data`, so trimming
/// cannot overwrite work completed by the background hash worker.
pub fn save_history(path: &Path, history: &HistoryData) -> Result<(), String> {
    let mut connection = open_history_database(path)?;
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
        let placeholders = std::iter::repeat("?").take(entry_ids.len()).collect::<Vec<_>>().join(", ");
        transaction
            .execute(
                &format!("DELETE FROM entries WHERE id NOT IN ({placeholders})"),
                params_from_iter(entry_ids),
            )
            .map_err(|error| error.to_string())?;
    }
    for entry in history.active_entries() {
        let extra = serde_json::to_string(&ClipboardEntryExtra {
            html: entry.html.clone(),
            rtf: entry.rtf.clone(),
            file_info: entry.file_info.clone(),
            image_info: entry.image_info.clone(),
        })
        .map_err(|error| error.to_string())?;
        let presentation = serde_json::to_string(&serde_json::json!({
            "html": entry.html,
            "rtf": entry.rtf,
        }))
        .map_err(|error| error.to_string())?;
        let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO entries (id, kind, content, extra, created_at, source_device_id, sources) VALUES (?, ?, ?, ?, ?, ?, ?)",
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
        transaction
            .execute(
                "UPDATE entries SET kind = ?, content = ?, extra = json_patch(extra, ?), created_at = ?, source_device_id = ? WHERE id = ?",
                params![
                    entry.kind,
                    entry.content,
                    presentation,
                    entry.created_at,
                    entry.source_device_id,
                    entry.id,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    // Every path that removes or re-keys an entry goes through this save: the
    // publish swap, trim eviction, content dedup and deletions all drop the
    // queue row that no longer has a matching temporary-id entry. This runs
    // after the rows above are written, so freshly captured entries keep
    // theirs.
    transaction
        .execute(
            "DELETE FROM pending_entries WHERE 'p' || seq NOT IN (SELECT id FROM entries)",
            [],
        )
        .map_err(|error| error.to_string())?;
    for (key, value) in [
        ("last_clipboard", history.last_clipboard.as_str()),
        ("last_file_signature", history.last_file_signature.as_str()),
        ("last_image_signature", history.last_image_signature.as_str()),
        ("device_id", history.device_id.as_str()),
        ("device_name", history.device_name.as_str()),
    ] {
        transaction
            .execute(
                "INSERT INTO metadata (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
    }
    for (key, values) in [("pending_deletions", &history.pending_deletions)] {
        let value = serde_json::to_string(values).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO metadata (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

pub fn write_entry_data(connection: &Connection, entry: &ClipboardEntry) -> Result<(), String> {
    let extra = serde_json::to_string(&ClipboardEntryExtra {
        html: entry.html.clone(),
        rtf: entry.rtf.clone(),
        file_info: entry.file_info.clone(),
        image_info: entry.image_info.clone(),
    })
    .map_err(|error| error.to_string())?;
    let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE entries SET extra = ?, sources = ? WHERE id = ?",
            params![extra, sources, entry.id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Event-driven record that the server now holds a content, replacing the old
/// per-entry batch write driven by `entry.files`. Availability is entry-shaped
/// state no more; it lives in this table and the in-memory set.
pub fn mark_files_uploaded(connection: &Connection, file_ids: &[String]) {
    for file_id in file_ids {
        let _ = connection
            .execute(
                "INSERT INTO files (file_id, size, created_at, available, cached) VALUES (?, 0, ?, 1, 0) ON CONFLICT(file_id) DO UPDATE SET available = 1",
                params![file_id.as_str(), now_rfc3339()],
            );
    }
}

/// Records a local content file this machine now holds. The content pool is independent of
/// entries, so this never rewrites history rows.
pub fn register_cached_file(
    database_path: &Path,
    file_id: &str,
    size: u64,
) -> Result<(), String> {
    let connection = open_history_database(database_path)?;
    connection
        .execute(
            "INSERT INTO files (file_id, size, created_at, available, cached) VALUES (?, ?, ?, 0, 1) ON CONFLICT(file_id) DO UPDATE SET size = excluded.size, cached = 1",
            params![file_id, size, now_rfc3339()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

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
/// entry payload, and removed by the `save_history` sweep (publish swap,
/// eviction, deletion) or an explicit acknowledge, so an offline capture
/// replays in order on the next connection.
pub fn enqueue_pending_entry(
    database_path: &Path,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<i64, String> {
    let connection = open_history_database(database_path)?;
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
    database_path: &Path,
    seq: i64,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<(), String> {
    let connection = open_history_database(database_path)?;
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
    database_path: &Path,
    seq: i64,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<(), String> {
    let connection = open_history_database(database_path)?;
    connection
        .execute(
            "INSERT INTO pending_entries (seq, kind, content, extra, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(seq) DO UPDATE SET kind = excluded.kind, content = excluded.content, extra = excluded.extra",
            params![seq, kind, content, extra, created_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn list_pending_rows(database_path: &Path) -> Result<Vec<PendingQueueRow>, String> {
    let connection = open_history_database(database_path)?;
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
pub fn acknowledge_pending_entry(database_path: &Path, seq: i64) -> Result<bool, String> {
    let connection = open_history_database(database_path)?;
    let changed = connection
        .execute(
            "DELETE FROM pending_entries WHERE seq = ?",
            params![seq],
        )
        .map_err(|error| error.to_string())?;
    Ok(changed > 0)
}

pub fn cached_hash(connection: &Connection, source: &str, size: u64, modified_at: i64) -> Option<String> {
    connection
        .query_row(
            "SELECT hash FROM hash_cache WHERE source = ? AND size = ? AND modified_at = ?",
            params![source, size, modified_at],
            |row| row.get::<_, String>("hash"),
        )
        .optional()
        .ok()
        .flatten()
}

pub fn remember_hash(connection: &Connection, source: &str, size: u64, modified_at: i64, hash: &str) {
    let _ = connection.execute(
        "INSERT INTO hash_cache (source, size, modified_at, hash) VALUES (?, ?, ?, ?) ON CONFLICT(source, size, modified_at) DO UPDATE SET hash = excluded.hash",
        params![source, size, modified_at, hash],
    );
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM hash_cache", [], |row| row.get(0))
        .unwrap_or_default();
    if count > HASH_CACHE_LIMIT {
        let _ = connection.execute(
            "DELETE FROM hash_cache WHERE rowid IN (SELECT rowid FROM hash_cache ORDER BY rowid ASC LIMIT ?)",
            params![count / 2],
        );
    }
}

/// Mark-sweep over local uploads, downloaded content, and views for entries
/// that are gone. Incomplete downloads expire after one day.
pub fn collect_local_garbage(histories_dir: &Path, history: &mut HistoryData) -> Result<usize, String> {
    let cache_dir = cache_dir_for(histories_dir, &history.active_history);
    let share_dir = cache_dir.join("share");
    let mut referenced = HashSet::new();
    let mut entry_ids = HashSet::new();
    let mut referenced_share_requests = HashSet::new();
    for entry in history.active_entries() {
        entry_ids.insert(entry.id.clone());
        for root in &entry.sources.roots {
            let path = PathBuf::from(root);
            if let Ok(relative) = path.strip_prefix(&share_dir) {
                if let Some(component) = relative.components().next() {
                    referenced_share_requests.insert(component.as_os_str().to_owned());
                }
            }
        }
        if let Some(image_info) = &entry.image_info {
            referenced.insert(image_info.file_id.clone());
        }
        for (file_id, _) in entry.file_info.as_ref().map(tree_contents).unwrap_or_default() {
            referenced.insert(file_id);
        }
    }

    let mut removed = Vec::new();
    for directory in [cache_dir.join("upload").join("images"), cache_dir.join("download")] {
        let Ok(files) = fs::read_dir(directory) else {
            continue;
        };
        for file in files.filter_map(Result::ok) {
            let file_id = file.file_name().to_string_lossy().into_owned();
            if referenced.contains(&file_id) && history.cached_files.contains(&file_id) {
                continue;
            }
            let expired = file
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|time| now_millis().saturating_sub(time.as_millis() as u64) > DOWNLOAD_TTL_MS)
                .unwrap_or(true);
            if !referenced.contains(&file_id) || expired {
                if fs::remove_file(file.path()).is_ok() {
                    removed.push(file_id);
                }
            }
        }
    }
    if let Ok(views) = fs::read_dir(cache_dir.join("views")) {
        for view in views.filter_map(Result::ok) {
            if !entry_ids.contains(&view.file_name().to_string_lossy().into_owned()) {
                let _ = fs::remove_dir_all(view.path());
            }
        }
    }
    let mut removed_share_requests = 0usize;
    if let Ok(requests) = fs::read_dir(&share_dir) {
        for request in requests.filter_map(Result::ok) {
            if !referenced_share_requests.contains(&request.file_name())
                && fs::remove_dir_all(request.path()).is_ok()
            {
                removed_share_requests += 1;
            }
        }
    }
    {
        let mut connection = open_history_database(&history_path_for_key(histories_dir, &history.active_history))?;
        let transaction = connection.transaction().map_err(|error| error.to_string())?;
        for chunk in removed.chunks(500) {
            let placeholders = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(", ");
            transaction
                .execute(
                    &format!("UPDATE files SET cached = 0 WHERE file_id IN ({placeholders})"),
                    params_from_iter(chunk.iter()),
                )
                .map_err(|error| error.to_string())?;
        }
        if referenced.is_empty() {
            transaction
                .execute("DELETE FROM files WHERE cached = 0", [])
                .map_err(|error| error.to_string())?;
        } else {
            let referenced = referenced.iter().collect::<Vec<_>>();
            let placeholders = std::iter::repeat("?")
                .take(referenced.len())
                .collect::<Vec<_>>()
                .join(", ");
            transaction
                .execute(
                    &format!("DELETE FROM files WHERE cached = 0 AND file_id NOT IN ({placeholders})"),
                    params_from_iter(referenced),
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        for file_id in &removed {
            history.cached_files.remove(file_id);
        }
    }
    Ok(removed.len() + removed_share_requests)
}

pub fn trim_history(entries: &mut Vec<ClipboardEntry>) {
    entries.truncate(MAX_HISTORY_ENTRIES);
}

pub fn retain_single_history(history: &mut HistoryData, key: &str) {
    let entries = history.histories.remove(key).unwrap_or_default();
    history.histories.clear();
    history.histories.insert(key.to_string(), entries);
    history.active_history = key.to_string();
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_history_keeps_contents_until_they_are_rewritten_explicitly() {
        use crate::content::TreeNode;
        use indexmap::IndexMap;

        let directory = std::env::temp_dir().join(format!("cliproam-contents-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let tree = |file_id: &str| {
            let mut inner = IndexMap::new();
            inner.insert(
                "a.txt".to_string(),
                TreeNode::File { f: file_id.to_string(), s: 12 },
            );
            let mut root = IndexMap::new();
            root.insert("bundle".to_string(), TreeNode::Dir(inner));
            root
        };
        let mut history = HistoryData {
            active_history: LOCAL_HISTORY_KEY.to_string(),
            ..HistoryData::default()
        };
        history.active_entries_mut().push(ClipboardEntry {
            id: "entry".to_string(),
            kind: "files".to_string(),
            content: "bundle".to_string(),
            html: Some("<b>bundle</b>".to_string()),
            rtf: Some("{\\rtf1 bundle}".to_string()),
            // Hashing has not run yet, so the content id is still empty.
            file_info: Some(tree("")),
            image_info: None,
            source_device_id: "device".to_string(),
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            summary: Default::default(),
            sources: crate::content::LocalSources::default(),
        });
        save_history(&path, &history).expect("store history");

        // Pasting or trimming goes through `save_history`, which must
        // never clobber a tree the hash worker filled in the meantime.
        let hashed = "a".repeat(64);
        {
            let connection = open_history_database(&path).expect("reopen database");
            let mut updated = history.active_entries()[0].clone();
            updated.file_info = Some(tree(&hashed));
            write_entry_data(&connection, &updated).expect("persist hashed tree");
        }
        save_history(&path, &history).expect("store history again");

        let reloaded = load_history(&path, LOCAL_HISTORY_KEY);
        let entry = &reloaded.active_entries()[0];
        fs::remove_dir_all(&directory).expect("remove temporary database");
        assert_eq!(entry.html.as_deref(), Some("<b>bundle</b>"));
        assert_eq!(entry.rtf.as_deref(), Some("{\\rtf1 bundle}"));
        let crate::content::TreeNode::Dir(bundle) = &entry.file_info.as_ref().expect("file info")["bundle"]
        else {
            panic!("bundle should be a directory");
        };
        let crate::content::TreeNode::File { f, .. } = &bundle["a.txt"] else {
            panic!("a.txt should be a file");
        };
        assert_eq!(f, &hashed);
    }

    #[test]
    fn hash_cache_round_trips_and_evicts() {
        let directory = std::env::temp_dir().join(format!("cliproam-hash-test-{}", Uuid::new_v4()));
        let connection = open_history_database(&directory.join("history.sqlite")).expect("create database");
        remember_hash(&connection, "C:/a.txt", 12, 99, "abc");
        assert_eq!(cached_hash(&connection, "C:/a.txt", 12, 99).as_deref(), Some("abc"));
        assert_eq!(cached_hash(&connection, "C:/a.txt", 13, 99), None);
        drop(connection);
        fs::remove_dir_all(&directory).expect("remove temporary database");
    }

    #[test]
    fn pending_entries_keep_capture_order_and_payload() {
        let directory = std::env::temp_dir().join(format!("cliproam-pending-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let mut seqs = Vec::new();
        for (kind, content) in [("text", "c"), ("text", "b"), ("image", "a")] {
            seqs.push(
                enqueue_pending_entry(&path, kind, content, "{}", "2026-01-01T00:00:00.000Z")
                    .expect("enqueue entry"),
            );
        }
        let rows = list_pending_rows(&path).expect("list pending");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].seq, seqs[0]);
        assert_eq!(rows[2].seq, seqs[2]);
        assert_eq!(rows[0].content, "c");
        assert_eq!(rows[2].kind, "image");
        assert_eq!(temp_entry_id(seqs[1]), format!("p{}", seqs[1]));
        assert_eq!(temp_entry_seq(&format!("p{}", seqs[1])), Some(seqs[1]));
        assert_eq!(temp_entry_seq(&seqs[1].to_string()), None);
        fs::remove_dir_all(&directory).expect("remove temporary database");
    }

    #[test]
    fn acknowledging_removes_only_the_confirmed_row() {
        let directory = std::env::temp_dir().join(format!("cliproam-pending-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let mut seqs = Vec::new();
        for content in ["a", "b", "c"] {
            seqs.push(
                enqueue_pending_entry(&path, "text", content, "{}", "2026-01-01T00:00:00.000Z")
                    .expect("enqueue entry"),
            );
        }
        assert!(acknowledge_pending_entry(&path, seqs[1]).expect("acknowledge entry"));
        assert!(!acknowledge_pending_entry(&path, seqs[1] + 1_000).expect("acknowledge entry"));
        let remaining = list_pending_rows(&path).expect("list pending");
        assert_eq!(remaining.iter().map(|row| row.seq).collect::<Vec<_>>(), [seqs[0], seqs[2]]);
        fs::remove_dir_all(&directory).expect("remove temporary database");
    }

    #[test]
    fn save_history_keeps_queue_rows_for_surviving_entries_and_sweeps_the_rest() {
        let directory = std::env::temp_dir().join(format!("cliproam-pending-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let kept = enqueue_pending_entry(&path, "text", "kept", "{}", "2026-01-01T00:00:00.000Z")
            .expect("enqueue entry");
        let dropped = enqueue_pending_entry(&path, "text", "dropped", "{}", "2026-01-01T00:00:00.000Z")
            .expect("enqueue entry");

        // `kept` still has its temporary-id entry; `dropped` was evicted, so
        // the sweep must remove its row while leaving the other untouched.
        let mut history = HistoryData {
            active_history: LOCAL_HISTORY_KEY.to_string(),
            ..HistoryData::default()
        };
        let entry = ClipboardEntry {
            id: temp_entry_id(kept),
            kind: "text".to_string(),
            content: "kept".to_string(),
            html: None,
            rtf: None,
            file_info: None,
            image_info: None,
            source_device_id: "device".to_string(),
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            summary: Default::default(),
            sources: crate::content::LocalSources::default(),
        };
        history.active_entries_mut().push(entry);
        save_history(&path, &history).expect("store history");
        let rows = list_pending_rows(&path).expect("list pending");
        assert_eq!(rows.iter().map(|row| row.seq).collect::<Vec<_>>(), [kept]);
        assert!(rows[0].seq != dropped);

        // Publishing swaps the entry id, which sweeps the queue row too.
        history.active_entries_mut()[0].id = "42".to_string();
        save_history(&path, &history).expect("store history again");
        assert!(list_pending_rows(&path).expect("list pending").is_empty());
        fs::remove_dir_all(&directory).expect("remove temporary database");
    }

    #[test]
    fn ensure_and_update_pending_entry_round_trip() {
        let directory = std::env::temp_dir().join(format!("cliproam-pending-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let seq = enqueue_pending_entry(&path, "files", "bundle", r#"{"fileInfo":{}}"#, "2026-01-01T00:00:00.000Z")
            .expect("enqueue entry");
        // Recreating an existing row keeps the original.
        ensure_pending_entry(&path, seq, "files", "bundle", r#"{"fileInfo":{}}"#, "2026-01-01T00:00:00.000Z")
            .expect("ensure entry");
        assert_eq!(list_pending_rows(&path).expect("list pending").len(), 1);
        // Hashing folds the resolved content ids back into the row.
        update_pending_entry(&path, seq, "files", "bundle", r#"{"fileInfo":{"a.txt":{"f":"aabb","s":1}}}"#, "2026-01-01T00:00:00.000Z")
            .expect("update entry");
        let rows = list_pending_rows(&path).expect("list pending");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].extra.contains("aabb"));
        // A row the sweep already removed is recreated by the update.
        acknowledge_pending_entry(&path, seq).expect("acknowledge entry");
        update_pending_entry(&path, seq, "files", "bundle", r#"{}"#, "2026-01-01T00:00:00.000Z")
            .expect("update entry again");
        assert_eq!(list_pending_rows(&path).expect("list pending").len(), 1);
        fs::remove_dir_all(&directory).expect("remove temporary database");
    }

    #[test]
    fn legacy_pending_entries_table_is_recreated() {
        let directory = std::env::temp_dir().join(format!("cliproam-pending-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        fs::create_dir_all(&directory).expect("create temporary directory");
        {
            let connection = Connection::open(&path).expect("open database");
            connection
                .execute_batch(
                    "CREATE TABLE pending_entries (
                        seq INTEGER PRIMARY KEY AUTOINCREMENT,
                        entry_id TEXT NOT NULL UNIQUE,
                        queued_at TEXT NOT NULL
                    );
                    INSERT INTO pending_entries (entry_id, queued_at) VALUES ('dead', '2026-01-01T00:00:00.000Z');",
                )
                .expect("create legacy table");
        }
        open_history_database(&path).expect("migrate database");
        assert!(list_pending_rows(&path).expect("list pending").is_empty());
        // The new layout accepts captures and survives a reopen.
        enqueue_pending_entry(&path, "text", "a", "{}", "2026-01-01T00:00:00.000Z").expect("enqueue entry");
        drop(open_history_database(&path).expect("reopen database"));
        assert_eq!(list_pending_rows(&path).expect("list pending").len(), 1);
        fs::remove_dir_all(&directory).expect("remove temporary database");
    }
}
