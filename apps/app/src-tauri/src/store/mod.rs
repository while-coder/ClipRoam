//! Local persistence for the clipboard history.
//!
//! Entry metadata, trees and local sources share one row. General history saves
//! patch only presentation fields, while capture, hashing and remote upsert
//! explicitly replace the full entry data. `files` tracks which content ids the
//! server pool holds; local-cache state is derived from the blob directories on
//! disk, which are the source of truth for it.

mod cache;
mod queue;

pub use cache::{
    cached_hash, cached_source_for, collect_local_garbage, mark_files_uploaded, remember_hash,
    scan_cached_blobs,
};
pub use queue::{
    acknowledge_pending_entry, enqueue_pending_entry, ensure_pending_entry, list_pending_rows,
    temp_entry_id, temp_entry_seq, update_pending_entry,
};

use rusqlite::{params, params_from_iter, Connection};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use crate::content::{refresh_summary, ClipboardEntry, ClipboardEntryExtra};

pub const MAX_HISTORY_ENTRIES: usize = 200;
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
        let extra = ClipboardEntryExtra::of(entry).json()?;
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

pub fn write_entry_data(connection: &Connection, entry: &ClipboardEntry) -> Result<(), String> {
    let extra = ClipboardEntryExtra::of(entry).json()?;
    let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE entries SET extra = ?, sources = ? WHERE id = ?",
            params![extra, sources, entry.id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
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

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{LocalSources, TreeNode};
    use indexmap::IndexMap;

    #[test]
    fn saving_history_keeps_contents_until_they_are_rewritten_explicitly() {
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
            sources: LocalSources::default(),
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
            sources: LocalSources::default(),
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
    fn legacy_columns_are_migrated_on_open() {
        let directory = std::env::temp_dir().join(format!("cliproam-migrate-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        fs::create_dir_all(&directory).expect("create temporary directory");
        {
            let connection = Connection::open(&path).expect("open database");
            connection
                .execute_batch(
                    "CREATE TABLE entries (
                        id TEXT PRIMARY KEY,
                        kind TEXT NOT NULL,
                        content TEXT NOT NULL,
                        extra TEXT NOT NULL DEFAULT '{}',
                        created_at TEXT NOT NULL,
                        source_device_id TEXT NOT NULL,
                        source_app TEXT NOT NULL DEFAULT '',
                        sources TEXT NOT NULL DEFAULT '{}',
                        pinned INTEGER NOT NULL DEFAULT 0
                    );
                    CREATE INDEX entries_source_app_created_at ON entries(source_app, created_at DESC);
                    CREATE TABLE files (
                        file_id TEXT PRIMARY KEY,
                        size INTEGER NOT NULL,
                        created_at TEXT NOT NULL,
                        available INTEGER NOT NULL DEFAULT 0,
                        cached INTEGER NOT NULL DEFAULT 0
                    );
                    INSERT INTO files (file_id, size, created_at, available, cached)
                        VALUES ('a', 1, 'x', 1, 1), ('b', 2, 'x', 0, 1);",
                )
                .expect("create legacy tables");
        }
        open_history_database(&path).expect("migrate database");
        {
            let connection = Connection::open(&path).expect("reopen database");
            let mut statement = connection
                .prepare("SELECT name FROM pragma_table_info('entries') UNION ALL SELECT name FROM pragma_table_info('files')")
                .expect("read columns");
            let names = statement
                .query_map([], |row| row.get::<_, String>(0))
                .expect("query columns")
                .collect::<Result<Vec<_>, _>>()
                .expect("read columns");
            assert!(!names
                .iter()
                .any(|name| name == "pinned" || name == "source_app" || name == "available" || name == "cached" || name == "size"));
            // The uploaded mark survives; the local-cache-only row is swept.
            let stored: i64 = connection
                .query_row("SELECT stored FROM files WHERE file_id = 'a'", [], |row| row.get(0))
                .expect("read stored flag");
            assert_eq!(stored, 1);
            assert!(connection
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
                .map(|count| count == 1)
                .unwrap_or(false));
        }
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
