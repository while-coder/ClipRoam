use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{history_path_for_key, cache_dir_for, now_rfc3339, open_history_database, HistoryData};
use crate::content::tree_contents;

pub const HASH_CACHE_LIMIT: i64 = 20_000;
pub const DOWNLOAD_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

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

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

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
