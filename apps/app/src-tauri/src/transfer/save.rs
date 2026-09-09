//! Save-to-disk sessions: staging directories that collect the entry's
//! contents before they are materialized at the user-chosen destination.

use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};
use tauri::State;

use crate::clipboard::output::{missing_files, snapshot_entry};
use crate::content::{rebuild_tree, tree_contents, MissingFile, TreeNode};
use crate::transfer::download::DownloadTarget;
use crate::AppState;

pub(crate) struct SaveSession {
    entry_id: String,
    destination: PathBuf,
    pub(crate) staging_dir: PathBuf,
    single_file: bool,
    pub(crate) expected: HashMap<String, u64>,
    pub(crate) in_progress: HashSet<String>,
    pub(crate) downloaded: HashSet<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavePreparation {
    save_id: String,
    missing: Vec<MissingFile>,
}

#[tauri::command(rename_all = "camelCase")]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn prepare_save_entry(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<Option<SavePreparation>, String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let file_info = snapshot
        .entry
        .file_info
        .as_ref()
        .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;
    if file_info.is_empty() {
        return Err("该记录不包含可另存的文件".to_string());
    }

    let single_file = file_info.len() == 1
        && matches!(file_info.values().next(), Some(TreeNode::File { .. }));
    let destination = if single_file {
        let name = file_info.keys().next().expect("count is one");
        let Some(destination) = rfd::FileDialog::new()
            .set_file_name(name)
            .save_file()
        else {
            return Ok(None);
        };
        destination
    } else {
        let Some(destination) = rfd::FileDialog::new().pick_folder() else {
            return Ok(None);
        };
        destination
    };

    let save_id = uuid::Uuid::new_v4().to_string();
    let staging_parent = if single_file {
        destination
            .parent()
            .ok_or_else(|| "无法确定目标目录".to_string())?
            .to_path_buf()
    } else {
        destination.clone()
    };
    let staging_dir = staging_parent.join(format!(".cliproam-save-{save_id}"));
    fs::create_dir_all(&staging_dir).map_err(|error| format!("无法准备目标目录：{error}"))?;

    let missing = missing_files(&snapshot);
    let expected = missing
        .iter()
        .map(|file| (file.file_id.clone(), file.size))
        .collect();
    state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .insert(
            save_id.clone(),
            SaveSession {
                entry_id,
                destination,
                staging_dir,
                single_file,
                expected,
                in_progress: HashSet::new(),
                downloaded: HashSet::new(),
            },
        );
    Ok(Some(SavePreparation { save_id, missing }))
}

#[tauri::command(rename_all = "camelCase")]
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) fn prepare_save_entry(
    _state: State<'_, AppState>,
    _entry_id: String,
) -> Result<Option<SavePreparation>, String> {
    Err("移动端文件已保存在应用缓存中，请使用系统分享或文件导出入口".to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn cancel_save_entry(state: State<'_, AppState>, save_id: String) -> Result<(), String> {
    let session = state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&save_id);
    if let Some(session) = session {
        let mut downloads = state.downloads.lock().map_err(|error| error.to_string())?;
        let transfer_ids = downloads
            .iter()
            .filter_map(|(transfer_id, download)| match &download.target {
                DownloadTarget::Save { save_id: target_id, .. } if target_id == &save_id => {
                    Some(transfer_id.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for transfer_id in transfer_ids {
            if let Some(download) = downloads.remove(&transfer_id) {
                let _ = fs::remove_file(download.path);
            }
        }
        drop(downloads);
        let _ = fs::remove_dir_all(session.staging_dir);
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn finish_save_entry(state: State<'_, AppState>, save_id: String) -> Result<usize, String> {
    let session = state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&save_id)
        .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
    let result = (|| {
        if !session.in_progress.is_empty() || session.downloaded.len() != session.expected.len() {
            return Err("另存为所需文件尚未下载完成".to_string());
        }
        let snapshot = snapshot_entry(&state, &session.entry_id)?;
        let file_info = snapshot
            .entry
            .file_info
            .as_ref()
            .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;

        let mut resolved = HashMap::<String, PathBuf>::new();
        for (file_id, _) in tree_contents(file_info) {
            if resolved.contains_key(&file_id) {
                continue;
            }
            if session.downloaded.contains(&file_id) {
                resolved.insert(file_id.clone(), session.staging_dir.join(&file_id));
            } else {
                let source = snapshot
                    .resolve(&file_id)
                    .ok_or_else(|| format!("文件内容不可用：{file_id}"))?;
                resolved.insert(file_id.clone(), source);
            }
        }

        if session.single_file {
            let (name, node) = file_info
                .iter()
                .next()
                .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;
            let TreeNode::File { f, .. } = node else {
                return Err("该记录不包含可另存的文件".to_string());
            };
            let source = resolved
                .get(f)
                .ok_or_else(|| format!("文件内容不可用：{name}"))?;
            if fs::canonicalize(source).ok() == fs::canonicalize(&session.destination).ok() {
                return Ok(0);
            }
            fs::copy(source, &session.destination).map_err(|error| format!("无法保存文件：{error}"))?;
            Ok(1)
        } else {
            // Real copies: the user owns the destination, and a hard link would
            // let a later edit reach back into the source or cache.
            rebuild_tree(&session.destination, file_info, &|file_id| resolved.get(file_id).cloned(), false)
        }
    })();
    let _ = fs::remove_dir_all(&session.staging_dir);
    result
}
