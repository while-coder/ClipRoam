//! 同步时的内容解析：files 队列行带着 `f: ""` 占位树进入队列，上传队列的
//! drain 取到该行时才在这里把内容 id（sha256）解析出来并写回该行。文本与
//! 图片在捕获时就已完成，不会走到这里。

use std::collections::HashMap;
use std::path::Path;
use tauri::{AppHandle, Emitter};

use super::{list_rows, row_entry};
use crate::content::{
    describe_roots, tree_parent_at_path, ClipboardEntry, ClipboardEntryExtra, TreeNode,
};
use crate::file::{cached_file_hash, remember_file_hash};
use crate::utils::hash_file;

/// How many freshly hashed paths are folded into the row before the UI is
/// told about the progress.
const HASH_PROGRESS_BATCH: usize = 32;

struct PendingHash {
    path: String,
    source: String,
    size: u64,
    modified_at: Option<u64>,
}

/// Resolves every still-unresolved content id of a `files` queue row, folding
/// the results back into the row itself. Runs when the upload queue's drain
/// reaches the row. Idempotent: already-resolved sources are skipped.
pub fn resolve_entry_files(
    app: &AppHandle,
    seq: i64,
    account: &crate::account::AccountContext,
) -> Result<(), String> {
    let mut entry = {
        let Some(row) = account.with_database(|connection| {
            Ok(list_rows(connection)?
                .into_iter()
                .find(|row| row.seq == seq))
        })?
        else {
            return Ok(());
        };
        row_entry(&row)
    };
    let pending = entry
        .sources
        .files
        .iter()
        .filter(|source| source.file_id.is_none())
        .map(|source| PendingHash {
            path: source.path.clone(),
            source: source.source.clone(),
            size: source.size,
            modified_at: source.modified_at,
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(());
    }

    // The hash cache shares the account database connection; each lookup only
    // holds it briefly, so hashing a large file never blocks a history write.
    let mut batch = Vec::new();
    for item in pending {
        let modified_at = item.modified_at.map(|value| value as i64).unwrap_or(-1);
        // The connection lock only covers the two quick cache reads/writes; the hash
        // itself runs unlocked — hashing a large file takes a while and every
        // command for this account queues behind the connection.
        let cached = account
            .with_database(|connection| {
                Ok(cached_file_hash(
                    connection,
                    &item.source,
                    item.size,
                    modified_at,
                ))
            })
            .ok()
            .flatten();
        let file_id = match cached {
            Some(hash) => Some(hash),
            None => {
                let hashed = hash_source(Path::new(&item.source))?;
                if let Some(hashed) = &hashed {
                    account.with_database(|connection| {
                        remember_file_hash(
                            connection,
                            &item.source,
                            item.size,
                            modified_at,
                            &hashed,
                        );
                        Ok(())
                    })?;
                }
                hashed
            }
        };
        batch.push((item.path, file_id));
        if batch.len() >= HASH_PROGRESS_BATCH {
            apply_hashes(app, seq, account, &mut entry, &batch)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        apply_hashes(app, seq, account, &mut entry, &batch)?;
    }
    Ok(())
}

/// 只有确实消失的文件才能从树中移除；权限或 I/O 错误保留来源供重试。
fn hash_source(path: &Path) -> Result<Option<String>, String> {
    match hash_file(path) {
        Ok(hash) => Ok(Some(hash)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "无法读取文件 {}：{error}。请检查文件访问权限，授权后在待上传列表中重试。",
            path.display(),
        )),
    }
}

/// Folds resolved content ids into the queue row: the tree leaves, the content
/// description (so keyword search stays true) and the sources whose files
/// vanished between copy and hash.
fn apply_hashes(
    app: &AppHandle,
    seq: i64,
    account: &crate::account::AccountContext,
    entry: &mut ClipboardEntry,
    resolved: &[(String, Option<String>)],
) -> Result<(), String> {
    let hashes = resolved
        .iter()
        .map(|(path, file_id)| (path.as_str(), file_id.as_deref()))
        .collect::<HashMap<_, _>>();
    if let Some(file_info) = entry.file_info.as_mut() {
        for (path, file_id) in resolved {
            let Some(parent) = tree_parent_at_path(file_info, path) else {
                continue;
            };
            let leaf = path.rsplit('/').next().unwrap_or_default();
            match file_id {
                Some(file_id) => {
                    if let Some(TreeNode::File { f, .. }) = parent.get_mut(leaf) {
                        *f = file_id.clone();
                    }
                }
                // A file that vanished between copy and hash drops out of the tree.
                None => {
                    parent.shift_remove(leaf);
                }
            }
        }
    }
    entry
        .sources
        .files
        .retain_mut(|source| match hashes.get(source.path.as_str()) {
            Some(Some(file_id)) => {
                source.file_id = Some((*file_id).to_string());
                true
            }
            Some(None) => false,
            None => true,
        });
    let content = match &entry.file_info {
        Some(file_info) => describe_roots(file_info),
        None => entry.content.clone(),
    };
    // sources 已并入 extra（localSources），随 extra 一起写回。
    let extra = ClipboardEntryExtra::of(entry).json()?;
    {
        account.with_database(|connection| {
            connection
                .execute(
                    "UPDATE pending_entries SET content = ?, extra = ? WHERE seq = ?",
                    rusqlite::params![content, extra, seq],
                )
                .map_err(|error| error.to_string())
        })?;
    }
    app.emit("cliproam://pending-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::hash_source;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn permission_failure_is_retryable_and_does_not_mean_file_disappeared() {
        let directory =
            std::env::temp_dir().join(format!("cliproam-hash-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("source.txt");
        fs::write(&path, b"recover after permission grant").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = hash_source(&path);
        // 恢复权限再断言，避免测试失败时留下不可读的临时文件。
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let restored = hash_source(&path);
        fs::remove_dir_all(&directory).unwrap();
        assert!(denied.unwrap_err().contains("授权后"));
        assert!(restored.unwrap().is_some());
        assert!(hash_source(&path).unwrap().is_none());
    }
}
