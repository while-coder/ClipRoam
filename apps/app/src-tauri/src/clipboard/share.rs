//! Android share-target intake: shared items arrive as pending shares and are
//! imported into history as if captured locally.

use serde::Serialize;
use tauri::{AppHandle, State};

#[cfg(target_os = "android")]
use crate::active_cache_dir;
#[cfg(target_os = "android")]
use std::{
    fs,
    path::{Path, PathBuf},
};
#[cfg(target_os = "android")]
use tauri::Manager;
use crate::AppState;

#[cfg(target_os = "android")]
use super::capture::{capture_files, capture_image, capture_text, RichText};
#[cfg(target_os = "android")]
use super::output::write_clipboard_text;
#[cfg(target_os = "android")]
use tauri_plugin_cliproam_share_receiver::{PendingShare, ShareReceiverExt};

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareImportSummary {
    shares: usize,
    texts: usize,
    images: usize,
    files: usize,
}

#[cfg(target_os = "android")]
fn persist_shared_files(app: &AppHandle, share: &PendingShare) -> Result<Vec<PathBuf>, String> {
    let state = app.state::<AppState>();
    let cache_dir = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        active_cache_dir(&state, &history)
    };
    let request_id = uuid::Uuid::parse_str(&share.id).map_err(|_| "分享请求标识不合法".to_string())?;
    let directory = cache_dir.join("share").join(request_id.to_string());
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;

    let mut paths = Vec::with_capacity(share.items.len());
    for (index, item) in share.items.iter().enumerate() {
        let source = PathBuf::from(&item.path);
        if !source.is_file() {
            return Err(format!("分享文件已失效：{}", item.name));
        }
        let name = Path::new(&item.name)
            .file_name()
            .filter(|name| !name.is_empty())
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("shared-{}", index + 1).into());
        let target = directory.join(name);
        let source_size = fs::metadata(&source).map_err(|error| error.to_string())?.len();
        let target_matches = fs::metadata(&target)
            .map(|metadata| metadata.is_file() && metadata.len() == source_size)
            .unwrap_or(false);
        if !target_matches {
            fs::copy(&source, &target).map_err(|error| format!("无法保存分享文件 {}：{error}", item.name))?;
        }
        paths.push(target);
    }
    Ok(paths)
}

#[cfg(target_os = "android")]
fn import_android_share(app: &AppHandle, share: &PendingShare) -> Result<ShareImportSummary, String> {
    let mut summary = ShareImportSummary::default();
    if let Some(text) = share.text.as_ref().filter(|text| !text.trim().is_empty()) {
        let rich_text = RichText {
            text: text.clone(),
            html: share.html.clone(),
            rtf: None,
        };
        // A share is the mobile equivalent of a fresh local clipboard capture.
        // Keep the OS clipboard useful too, but never discard the history item
        // just because a particular device rejected the clipboard write.
        let _ = write_clipboard_text(app, &rich_text);
        capture_text(app, rich_text)?;
        summary.texts = 1;
    }

    if share.items.len() == 1 && share.items[0].mime_type.starts_with("image/") {
        let image = fs::read(&share.items[0].path)
            .map_err(|error| format!("无法读取分享图片：{error}"))?;
        capture_image(app, image)?;
        summary.images = 1;
    } else if !share.items.is_empty() {
        let paths = persist_shared_files(app, share)?;
        capture_files(app, paths)?;
        summary.files = share.items.len();
    }
    summary.shares = 1;
    Ok(summary)
}

#[tauri::command]
pub(crate) fn consume_mobile_shares(app: AppHandle, state: State<'_, AppState>) -> Result<ShareImportSummary, String> {
    let _guard = state.share_import.lock().map_err(|error| error.to_string())?;
    #[cfg(target_os = "android")]
    {
        let mut imported = ShareImportSummary::default();
        for share in app.share_receiver().pending().map_err(|error| error.to_string())? {
            let summary = import_android_share(&app, &share)?;
            app.share_receiver()
                .acknowledge(&share.id)
                .map_err(|error| error.to_string())?;
            imported.shares += summary.shares;
            imported.texts += summary.texts;
            imported.images += summary.images;
            imported.files += summary.files;
        }
        Ok(imported)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(ShareImportSummary::default())
    }
}
