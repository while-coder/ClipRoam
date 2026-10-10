//! 另存会话：下载到账号缓存完成后，将内容复制到用户选择的目标目录。

use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
};
use tauri::{AppHandle, State};

use crate::history::clipboard::snapshot_entry_for;
use crate::content::{rebuild_tree, sanitize_root_name, TreeNode};
use crate::content::entry_contents_of;
use crate::AppState;

// Each platform constructs only the destination kind it supports.
#[allow(dead_code)]
pub(crate) enum SaveDestination {
    Path(PathBuf),
    DocumentTree(String),
}

pub(crate) struct SaveSession {
    account: std::sync::Arc<crate::account::AccountContext>,
    entry_id: String,
    destination: PathBuf,
    single_file: bool,
    document_tree: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavePreparation {
    save_id: String,
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) async fn prepare_save_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    session_id: String,
) -> Result<Option<SavePreparation>, String> {
    let account = state.account(&session_id)?;
    if !crate::platforms::supports_native_file_export() {
        return Err("移动端文件已保存在应用缓存中，请使用系统分享或文件导出入口".to_string());
    }
    let snapshot = snapshot_entry_for(&entry_id, &account)?;
    // 图片条目没有 file_info，按单文件另存：内容是编码后的 WebP，
    // 名字取条目标题（如「截图（367 × 109）.webp」）。
    let (single_file, name) = match snapshot.entry.file_info.as_ref() {
        Some(file_info) if !file_info.is_empty() => {
            let single_file = file_info.len() == 1
                && matches!(file_info.values().next(), Some(TreeNode::File { .. }));
            let name = file_info.keys().next().expect("count is one").clone();
            (single_file, name)
        }
        _ if snapshot.entry.image_info.is_some() => (
            true,
            format!("{}.webp", sanitize_root_name(&snapshot.entry.content)),
        ),
        _ => return Err("该记录不包含可另存的文件".to_string()),
    };
    let Some(target) = crate::platforms::prompt_save_destination(&app, single_file, &name).await? else {
        return Ok(None);
    };

    let save_id = uuid::Uuid::new_v4().to_string();
    let (destination, document_tree) = match target {
        SaveDestination::Path(path) => (path, None),
        SaveDestination::DocumentTree(uri) => {
            // SAF URIs are not filesystem paths. Reconstruct the export in the
            // sandbox, then let ContentResolver write it into the chosen tree.
            let directory = snapshot.cache_dir.join(format!(".cliproam-export-{save_id}"));
            let path = if single_file { directory.join(sanitize_root_name(&name)) } else { directory };
            (path, Some(uri))
        }
    };
    state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .insert(
            save_id.clone(),
            SaveSession {
                account,
                entry_id,
                destination,
                single_file,
                document_tree,
            },
        );
    Ok(Some(SavePreparation { save_id }))
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn cancel_save_entry(state: State<'_, AppState>, save_id: String) -> Result<(), String> {
    let session = state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&save_id);
    if let Some(session) = session {
        if session.document_tree.is_some() {
            let root = if session.single_file {
                session.destination.parent().expect("single-file destination has a parent")
            } else {
                session.destination.as_path()
            };
            let _ = fs::remove_dir_all(root);
        }
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn finish_save_entry(app: AppHandle, state: State<'_, AppState>, save_id: String) -> Result<usize, String> {
    let session = state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&save_id)
        .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
    let mut result = (|| {
        let snapshot = snapshot_entry_for(&session.entry_id, &session.account)?;

        let mut resolved = HashMap::<String, PathBuf>::new();
        // entry_contents_of 同时覆盖文件树与图片条目，比 tree_contents 多兜住图片。
        for (file_id, _) in entry_contents_of(&snapshot.entry) {
            if resolved.contains_key(&file_id) {
                continue;
            }
            let source = snapshot.resolve(&file_id)
                .ok_or_else(|| format!("文件内容不可用：{file_id}"))?;
            resolved.insert(file_id, source);
        }

        if session.single_file {
            // 图片条目没有 file_info：保存 image_info 指向的那份编码图。
            let image = snapshot
                .entry
                .image_info
                .as_ref()
                .filter(|_| snapshot.entry.file_info.is_none());
            let source = if let Some(image) = image {
                resolved
                    .get(&image.file_id)
                    .ok_or_else(|| format!("文件内容不可用：{}", image.file_id))?
            } else {
                let (name, node) = snapshot
                    .entry
                    .file_info
                    .as_ref()
                    .and_then(|file_info| file_info.iter().next())
                    .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;
                let TreeNode::File { f, .. } = node else {
                    return Err("该记录不包含可另存的文件".to_string());
                };
                resolved
                    .get(f)
                    .ok_or_else(|| format!("文件内容不可用：{name}"))?
            };
            if fs::canonicalize(source).ok() == fs::canonicalize(&session.destination).ok() {
                return Ok(0);
            }
            crate::utils::ensure_parent_dir(&session.destination)?;
            fs::copy(source, &session.destination).map_err(|error| format!("无法保存文件：{error}"))?;
            Ok(1)
        } else {
            let file_info = snapshot
                .entry
                .file_info
                .as_ref()
                .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;
            // Real copies: the user owns the destination, and a hard link would
            // let a later edit reach back into the source or cache.
            rebuild_tree(&session.destination, file_info, &|file_id| resolved.get(file_id).cloned(), false)
        }
    })();
    if let Some(uri) = &session.document_tree {
        let export_root = if session.single_file {
            session.destination.parent().expect("single-file destination has a parent")
        } else {
            session.destination.as_path()
        };
        if result.is_ok() {
            result = crate::platforms::export_saved_directory(&app, export_root, uri);
        }
        let _ = fs::remove_dir_all(export_root);
    }
    result
}
