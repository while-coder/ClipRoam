use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::store::{cache_dir_for, HistoryData};
use crate::content::{tree_contents, ClipboardEntryExtra, LocalSources};
use crate::utils::modified_millis;

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

/// Every content id the durable history references (image contents plus file
/// tree leaves), streamed row by row so the entries themselves never load.
/// This is the input of the frontend's live pool-availability query.
pub fn history_file_ids(connection: &Connection) -> HashSet<String> {
    let mut statement = match connection.prepare("SELECT extra FROM entries") {
        Ok(statement) => statement,
        Err(_) => return HashSet::new(),
    };
    let rows = statement
        .query_map([], |row| row.get::<_, String>("extra"))
        .into_iter()
        .flatten()
        .flatten();
    let mut ids = HashSet::new();
    for extra in rows {
        let extra: ClipboardEntryExtra = serde_json::from_str(&extra).unwrap_or_default();
        if let Some(image_info) = &extra.image_info {
            ids.insert(image_info.file_id.clone());
        }
        for (file_id, _) in extra.file_info.as_ref().map(tree_contents).unwrap_or_default() {
            ids.insert(file_id);
        }
    }
    ids
}

/// Content ids this machine holds a blob for. The blob directories are the
/// source of truth, so the set is read straight off the disk; it can never
/// disagree with what is actually pasteable. Blob file names are the content
/// hashes themselves, so a directory listing is all it takes.
pub fn scan_cached_blobs(cache_dir: &Path) -> HashSet<String> {
    [cache_dir.join("upload").join("images"), cache_dir.join("download")]
        .into_iter()
        .filter_map(|directory| fs::read_dir(directory).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|file| file.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Reverse lookup over content this machine has hashed before: any recorded
/// source file whose bytes still verify against the recorded size and modified
/// time can stand in for the content, sparing a download. Every candidate is
/// checked and the first surviving one wins; an unverified match is never
/// trusted, and no match at all simply falls through to the download path.
pub fn cached_source_for(connection: &Connection, file_id: &str) -> Option<PathBuf> {
    let mut statement = connection
        .prepare("SELECT source, size, modified_at FROM hash_cache WHERE hash = ?")
        .ok()?;
    let candidates = statement
        .query_map([file_id], |row| {
            Ok((
                row.get::<_, String>("source")?,
                row.get::<_, i64>("size")?,
                row.get::<_, i64>("modified_at")?,
            ))
        })
        .ok()?
        .flatten();
    for (source, size, modified_at) in candidates {
        let Ok(metadata) = fs::symlink_metadata(&source) else {
            continue;
        };
        // `modified_at` is `-1` when the capture could not read it; the size
        // check alone still screens out replaced files.
        let unchanged = metadata.is_file()
            && metadata.len() == u64::try_from(size).unwrap_or(u64::MAX)
            && (modified_at < 0 || modified_millis(&metadata) == Some(modified_at as u64));
        if unchanged {
            return Some(PathBuf::from(source));
        }
    }
    None
}

/// Mark-sweep over local uploads, downloaded content, and views for entries
/// that are gone. Incomplete downloads expire after one day.
pub fn collect_local_garbage(
    connection: &mut Connection,
    histories_dir: &Path,
    history: &mut HistoryData,
) -> Result<usize, String> {
    let cache_dir = cache_dir_for(histories_dir, &history.active_history);
    let share_dir = cache_dir.join("share");
    let mut referenced = HashSet::new();
    let mut entry_ids = HashSet::new();
    let mut referenced_share_requests = HashSet::new();
    // Streamed row by row: only the derived id sets stay in memory, never the
    // entries themselves.
    {
        let mut statement = connection
            .prepare("SELECT id, extra, sources FROM entries")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>("id")?,
                    row.get::<_, String>("extra")?,
                    row.get::<_, String>("sources")?,
                ))
            })
            .map_err(|error| error.to_string())?;
        for (entry_id, extra, sources) in rows.flatten() {
            entry_ids.insert(entry_id);
            let sources: LocalSources = serde_json::from_str(&sources).unwrap_or_default();
            for root in &sources.roots {
                let path = PathBuf::from(root);
                if let Ok(relative) = path.strip_prefix(&share_dir) {
                    if let Some(component) = relative.components().next() {
                        referenced_share_requests.insert(component.as_os_str().to_owned());
                    }
                }
            }
            let extra: ClipboardEntryExtra = serde_json::from_str(&extra).unwrap_or_default();
            if let Some(image_info) = &extra.image_info {
                referenced.insert(image_info.file_id.clone());
            }
            for (file_id, _) in extra.file_info.as_ref().map(tree_contents).unwrap_or_default() {
                referenced.insert(file_id);
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
            if (!referenced.contains(&file_id) || expired)
                && fs::remove_file(file.path()).is_ok()
            {
                removed.push(file_id);
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
    for file_id in &removed {
        history.cached_files.remove(file_id);
    }
    Ok(removed.len() + removed_share_requests)
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}
