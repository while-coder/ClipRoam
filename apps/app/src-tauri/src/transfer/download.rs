//! File download plumbing: transfers from the server into the local content
//! cache or straight into a save session's staging directory.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use sha2::{Digest, Sha256};
use std::{fs, sync::Mutex};
use tauri::State;

use crate::content::download_path;
use crate::store::{history_path_for_key, register_cached_file};
use crate::AppState;

#[derive(Default)]
pub(crate) struct VirtualDownloadStatus {
    requested: bool,
    pub(crate) complete: bool,
    pub(crate) error: Option<String>,
}

#[derive(Default)]
pub(crate) struct VirtualDownloads {
    pub(crate) transfers: Mutex<std::collections::HashMap<String, VirtualDownloadStatus>>,
    pub(crate) changed: std::sync::Condvar,
}

impl VirtualDownloads {
    pub(crate) fn request(&self, file_id: &str) -> bool {
        let Ok(mut transfers) = self.transfers.lock() else { return false };
        let status = transfers.entry(file_id.to_string()).or_default();
        if status.complete {
            return false;
        }
        if status.error.take().is_some() {
            status.requested = false;
        }
        if status.requested {
            false
        } else {
            status.requested = true;
            true
        }
    }

    pub(crate) fn begin(&self, file_id: &str) {
        if let Ok(mut transfers) = self.transfers.lock() {
            transfers.insert(file_id.to_string(), VirtualDownloadStatus {
                requested: true,
                complete: false,
                error: None,
            });
            self.changed.notify_all();
        }
    }

    pub(crate) fn progress(&self) {
        self.changed.notify_all();
    }

    pub(crate) fn complete(&self, file_id: &str) {
        if let Ok(mut transfers) = self.transfers.lock() {
            let status = transfers.entry(file_id.to_string()).or_default();
            status.complete = true;
            status.error = None;
            self.changed.notify_all();
        }
    }

    pub(crate) fn fail(&self, file_id: &str, error: String) {
        if let Ok(mut transfers) = self.transfers.lock() {
            let status = transfers.entry(file_id.to_string()).or_default();
            status.complete = false;
            status.error = Some(error);
            self.changed.notify_all();
        }
    }
}

pub(crate) struct DownloadState {
    pub(crate) path: std::path::PathBuf,
    pub(crate) file_id: String,
    pub(crate) expected_size: u64,
    pub(crate) received_size: u64,
    pub(crate) hasher: Sha256,
    pub(crate) target: DownloadTarget,
}

pub(crate) enum DownloadTarget {
    Cache,
    Save {
        save_id: String,
        completed_path: std::path::PathBuf,
    },
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn begin_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    file_id: String,
    expected_size: u64,
    save_id: Option<String>,
) -> Result<(), String> {
    let (path, target) = if let Some(save_id) = save_id {
        let mut sessions = state.save_sessions.lock().map_err(|error| error.to_string())?;
        let session = sessions
            .get_mut(&save_id)
            .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
        let expected = session
            .expected
            .get(&file_id)
            .ok_or_else(|| "文件不属于当前另存为任务".to_string())?;
        if *expected != expected_size {
            return Err("文件大小与另存为任务不一致".to_string());
        }
        if session.downloaded.contains(&file_id) || !session.in_progress.insert(file_id.clone()) {
            return Err("文件正在下载或已经下载".to_string());
        }
        let completed_path = session.staging_dir.join(&file_id);
        (
            session.staging_dir.join(format!("{file_id}.part")),
            DownloadTarget::Save {
                save_id,
                completed_path,
            },
        )
    } else {
        state.virtual_downloads.begin(&file_id);
        let history = state.history.lock().map_err(|error| error.to_string())?;
        (
            download_path(&crate::active_cache_dir(&state, &history), &file_id)
                .ok_or_else(|| "内容标识不合法".to_string())?,
            DownloadTarget::Cache,
        )
    };
    let prepared = (|| {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::File::create(&path).map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(error) = prepared {
        clear_download_target(&state, &target, &file_id, &error);
        return Err(error);
    }
    state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .insert(
            transfer_id,
            DownloadState {
                path,
                file_id,
                expected_size,
                received_size: 0,
                hasher: Sha256::new(),
                target,
            },
        );
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn append_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    data: String,
) -> Result<(), String> {
    use std::io::Write;

    let bytes = BASE64.decode(data).map_err(|error| error.to_string())?;
    let mut downloads = state.downloads.lock().map_err(|error| error.to_string())?;
    let download = downloads
        .get_mut(&transfer_id)
        .ok_or_else(|| "文件下载任务不存在".to_string())?;
    download.received_size += bytes.len() as u64;
    if download.received_size > download.expected_size {
        return Err("下载内容超过声明大小".to_string());
    }
    download.hasher.update(&bytes);
    fs::OpenOptions::new()
        .append(true)
        .open(&download.path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|error| error.to_string())?;
    if matches!(download.target, DownloadTarget::Cache) {
        state.virtual_downloads.progress();
    }
    Ok(())
}

/// A completed cache download is registered only after digest verification.
/// Direct-save downloads stay inside the user-selected destination's staging
/// directory and never enter the application cache or its database.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn finish_file_download(state: State<'_, AppState>, transfer_id: String) -> Result<(), String> {
    let download = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&transfer_id)
        .ok_or_else(|| "文件下载任务不存在".to_string())?;
    if download.received_size != download.expected_size {
        let _ = fs::remove_file(&download.path);
        fail_download_target(&state, &download, "文件下载不完整");
        return Err("文件下载不完整".to_string());
    }
    if crate::content::to_hex(&download.hasher.clone().finalize()) != download.file_id {
        let _ = fs::remove_file(&download.path);
        fail_download_target(&state, &download, "文件内容校验失败");
        return Err("文件内容校验失败".to_string());
    }

    match &download.target {
        DownloadTarget::Cache => {
            let database_path = {
                let history = state.history.lock().map_err(|error| error.to_string())?;
                history_path_for_key(&state.histories_dir, &history.active_history)
            };
            register_cached_file(&database_path, &download.file_id, download.expected_size)?;
            state
                .history
                .lock()
                .map_err(|error| error.to_string())?
                .cached_files
                .insert(download.file_id.clone());
            state.virtual_downloads.complete(&download.file_id);
        }
        DownloadTarget::Save {
            save_id,
            completed_path,
        } => {
            if completed_path.exists() {
                fs::remove_file(completed_path).map_err(|error| error.to_string())?;
            }
            fs::rename(&download.path, completed_path).map_err(|error| error.to_string())?;
            let mut sessions = state.save_sessions.lock().map_err(|error| error.to_string())?;
            let session = sessions
                .get_mut(save_id)
                .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
            session.in_progress.remove(&download.file_id);
            session.downloaded.insert(download.file_id.clone());
        }
    }
    Ok(())
}

pub(crate) fn fail_download_target(state: &AppState, download: &DownloadState, message: &str) {
    clear_download_target(state, &download.target, &download.file_id, message);
}

pub(crate) fn clear_download_target(state: &AppState, target: &DownloadTarget, file_id: &str, message: &str) {
    match target {
        DownloadTarget::Cache => state.virtual_downloads.fail(file_id, message.to_string()),
        DownloadTarget::Save { save_id, .. } => {
            if let Ok(mut sessions) = state.save_sessions.lock() {
                if let Some(session) = sessions.get_mut(save_id) {
                    session.in_progress.remove(file_id);
                }
            }
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn cancel_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    let download = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&transfer_id);
    if let Some(download) = download {
        let _ = fs::remove_file(&download.path);
        fail_download_target(
            &state,
            &download,
            &reason.unwrap_or_else(|| "文件下载已取消".to_string()),
        );
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn fail_virtual_file_request(
    state: State<'_, AppState>,
    file_id: String,
    message: String,
) -> Result<(), String> {
    state.virtual_downloads.fail(&file_id, message);
    Ok(())
}
