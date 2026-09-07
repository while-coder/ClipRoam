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
    cached_file_path, refresh_summary, ClipboardEntry, ClipboardEntryExtra, ClipboardFile,
    ClipboardTree,
};

pub const SCHEMA_VERSION: i64 = 6;
pub const MAX_UNPINNED_ENTRIES: usize = 200;
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
    /// Small entry updates (currently pinning) need the same durable replay:
    /// a manifest only contains ids, so it cannot discover an update to an
    /// entry that already exists remotely.
    pub pending_entry_updates: HashSet<String>,
    /// Content ids this machine has a blob for. Kept in memory so refreshing a
    /// summary never touches the disk.
    pub cached_files: HashSet<String>,
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
            pending_entry_updates: HashSet::new(),
            cached_files: HashSet::new(),
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

/// Content addressing changed the storage layout outright, so a database from
/// an older schema is reset rather than migrated.
pub fn open_history_database(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
        .map_err(|error| error.to_string())?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version != SCHEMA_VERSION {
        connection
            .execute_batch(
                "
                DROP TABLE IF EXISTS entries_fts;
                DROP TABLE IF EXISTS entry_tags;
                DROP TABLE IF EXISTS tags;
                DROP TABLE IF EXISTS entry_contents;
                DROP TABLE IF EXISTS files;
                DROP TABLE IF EXISTS hash_cache;
                DROP TABLE IF EXISTS upload_sessions;
                DROP TABLE IF EXISTS entries;
                DROP TABLE IF EXISTS metadata;
                ",
            )
            .map_err(|error| error.to_string())?;
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent.join("files"));
        }
    }
    connection
        .execute_batch(
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
                pinned INTEGER NOT NULL,
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
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
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
                    "pending_entry_updates" => {
                        history.pending_entry_updates = serde_json::from_str(&value).unwrap_or_default()
                    }
                    _ => {}
                }
            }
        }
    }

    let mut entries = Vec::new();
    if let Ok(mut statement) = connection.prepare(
        "SELECT id, kind, content, extra, source_device_id, created_at, pinned, sources FROM entries ORDER BY pinned DESC, created_at DESC",
    ) {
        if let Ok(rows) = statement.query_map([], |row| {
            let extra = serde_json::from_str::<ClipboardEntryExtra>(&row.get::<_, String>("extra")?).unwrap_or_default();
            Ok(ClipboardEntry {
                id: row.get("id")?,
                kind: row.get("kind")?,
                content: row.get("content")?,
                html: extra.html,
                rtf: extra.rtf,
                thumbnail: extra.thumbnail,
                tree: extra.tree,
                files: Vec::new(),
                source_device_id: row.get("source_device_id")?,
                created_at: row.get("created_at")?,
                pinned: row.get::<_, i64>("pinned")? != 0,
                summary: Default::default(),
                sources: serde_json::from_str(&row.get::<_, String>("sources")?).unwrap_or_default(),
            })
        }) {
            entries.extend(rows.flatten());
        }
    }

    let mut known_files = HashMap::new();
    let mut cached_files = HashSet::new();
    if let Ok(mut statement) = connection.prepare("SELECT file_id, size, available, cached FROM files") {
        if let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>("file_id")?,
                row.get::<_, u64>("size")?,
                row.get::<_, i64>("available")? != 0,
                row.get::<_, i64>("cached")? != 0,
            ))
        }) {
            for (file_id, size, available, cached) in rows.flatten() {
                known_files.insert(file_id.clone(), ClipboardFile { file_id: file_id.clone(), size, available });
                if cached {
                    cached_files.insert(file_id);
                }
            }
        }
    }
    for entry in &mut entries {
        entry.files = files_for_tree(entry.tree.as_ref(), &known_files);
    }

    let cache_dir = cache_dir_for_path(path);
    history.cached_files = cached_files
        .into_iter()
        .filter(|file_id| cached_file_path(&cache_dir, file_id).is_some())
        .collect();
    history.histories.insert(key.to_string(), entries);
    refresh_summaries(&mut history, &cache_dir);
    history
}

fn files_for_tree(
    tree: Option<&ClipboardTree>,
    known_files: &HashMap<String, ClipboardFile>,
) -> Vec<ClipboardFile> {
    let Some(tree) = tree else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    tree.files
        .iter()
        .filter_map(|node| {
            if node.f.is_empty() || !seen.insert(node.f.clone()) {
                return None;
            }
            Some(known_files.get(&node.f).cloned().unwrap_or_else(|| ClipboardFile {
                file_id: node.f.clone(),
                size: node.s.unwrap_or_default(),
                available: false,
            }))
        })
        .collect()
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
        ..
    } = history;
    let Some(entries) = histories.get_mut(active_history) else {
        return;
    };
    for entry in entries.iter_mut() {
        refresh_summary(entry, cached_files, cache_dir);
    }
}

pub fn refresh_entry_summary(history: &mut HistoryData, entry_id: &str, cache_dir: &Path) {
    let HistoryData {
        histories,
        active_history,
        cached_files,
        ..
    } = history;
    let Some(entries) = histories.get_mut(active_history) else {
        return;
    };
    if let Some(entry) = entries
        .iter_mut()
        .find(|entry| entry.id == entry_id)
    {
        refresh_summary(entry, cached_files, cache_dir);
    }
}

/// Writes small entry fields plus metadata. Existing trees, local sources and
/// content rows are changed only by `write_entry_data`, so pinning or trimming
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
            thumbnail: entry.thumbnail.clone(),
            tree: entry.tree.clone(),
        })
        .map_err(|error| error.to_string())?;
        let presentation = serde_json::to_string(&serde_json::json!({
            "html": entry.html,
            "rtf": entry.rtf,
            "thumbnail": entry.thumbnail,
        }))
        .map_err(|error| error.to_string())?;
        let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO entries (id, kind, content, extra, created_at, pinned, source_device_id, sources) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    entry.id,
                    entry.kind,
                    entry.content,
                    extra,
                    entry.created_at,
                    entry.pinned,
                    entry.source_device_id,
                    sources,
                ],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE entries SET kind = ?, content = ?, extra = json_patch(extra, ?), created_at = ?, pinned = ?, source_device_id = ? WHERE id = ?",
                params![
                    entry.kind,
                    entry.content,
                    presentation,
                    entry.created_at,
                    entry.pinned,
                    entry.source_device_id,
                    entry.id,
                ],
            )
            .map_err(|error| error.to_string())?;
        if inserted != 0 {
            write_entry_files(&transaction, entry)?;
        }
    }
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
    for (key, values) in [
        ("pending_deletions", &history.pending_deletions),
        ("pending_entry_updates", &history.pending_entry_updates),
    ] {
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
        thumbnail: entry.thumbnail.clone(),
        tree: entry.tree.clone(),
    })
    .map_err(|error| error.to_string())?;
    let sources = serde_json::to_string(&entry.sources).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE entries SET extra = ?, sources = ? WHERE id = ?",
            params![extra, sources, entry.id],
        )
        .map_err(|error| error.to_string())?;
    write_entry_files(connection, entry)
}

fn write_entry_files(connection: &Connection, entry: &ClipboardEntry) -> Result<(), String> {
    for file in &entry.files {
        connection
            .execute(
                "INSERT INTO files (file_id, size, created_at, available, cached) VALUES (?, ?, ?, ?, 0) ON CONFLICT(file_id) DO UPDATE SET size = excluded.size, available = MAX(files.available, excluded.available)",
                params![file.file_id, file.size, now_rfc3339(), file.available],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
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
        if let Some(tree) = &entry.tree {
            for node in &tree.files {
                if !node.f.is_empty() {
                    referenced.insert(node.f.clone());
                }
            }
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
    let mut unpinned = 0usize;
    entries.retain(|entry| {
        if entry.pinned {
            true
        } else {
            unpinned += 1;
            unpinned <= MAX_UNPINNED_ENTRIES
        }
    });
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
    fn opening_an_older_schema_resets_the_database() {
        let directory = std::env::temp_dir().join(format!("cliproam-schema-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let connection = open_history_database(&path).expect("create history database");
        connection
            .execute(
                "INSERT INTO entries (id, kind, content, created_at, pinned, source_device_id) VALUES ('a', 'text', 'kept', '', 0, 'd')",
                [],
            )
            .expect("insert entry");
        connection.execute_batch("PRAGMA user_version = 1").expect("downgrade schema");
        drop(connection);

        let connection = open_history_database(&path).expect("reopen history database");
        let remaining: i64 = connection
            .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
            .expect("count entries");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read schema version");
        drop(connection);
        fs::remove_dir_all(&directory).expect("remove temporary history database");
        assert_eq!(remaining, 0);
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn saving_history_keeps_contents_until_they_are_rewritten_explicitly() {
        use crate::content::{ClipboardTreeFile, ClipboardTreeRoot};

        let directory = std::env::temp_dir().join(format!("cliproam-contents-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let tree = |file_id: &str| ClipboardTree {
            v: crate::content::TREE_VERSION,
            roots: vec![ClipboardTreeRoot { name: "bundle".to_string(), kind: "dir".to_string() }],
            dirs: vec!["bundle".to_string()],
            files: vec![ClipboardTreeFile { p: "bundle/a.txt".to_string(), f: file_id.to_string(), s: None }],
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
            thumbnail: Some("thumbnail".to_string()),
            // Hashing has not run yet, so the content id is still empty.
            tree: Some(tree("")),
            files: Vec::new(),
            source_device_id: "device".to_string(),
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            pinned: false,
            summary: Default::default(),
            sources: crate::content::LocalSources::default(),
        });
        save_history(&path, &history).expect("store history");

        // Pinning, pasting or trimming all go through `save_history`, which must
        // never clobber a tree the hash worker filled in the meantime.
        let hashed = "a".repeat(64);
        {
            let connection = open_history_database(&path).expect("reopen database");
            let mut updated = history.active_entries()[0].clone();
            updated.tree = Some(tree(&hashed));
            write_entry_data(&connection, &updated).expect("persist hashed tree");
        }
        history.active_entries_mut()[0].pinned = true;
        save_history(&path, &history).expect("store history again");

        let reloaded = load_history(&path, LOCAL_HISTORY_KEY);
        let entry = &reloaded.active_entries()[0];
        fs::remove_dir_all(&directory).expect("remove temporary database");
        assert!(entry.pinned);
        assert_eq!(entry.html.as_deref(), Some("<b>bundle</b>"));
        assert_eq!(entry.rtf.as_deref(), Some("{\\rtf1 bundle}"));
        assert_eq!(entry.thumbnail.as_deref(), Some("thumbnail"));
        assert_eq!(entry.tree.as_ref().expect("tree").files[0].f, hashed);
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
}
