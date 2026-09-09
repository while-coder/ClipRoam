//! Android 平台实现：系统分享接收导入，其余复用移动端共享桩。

use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};
use tauri_plugin_cliproam_share_receiver::{PendingShare, ShareReceiverExt};

use crate::clipboard::capture::{capture_files, capture_image, capture_text, RichText, ShareImportSummary};
use crate::{active_cache_dir, AppState};

pub(crate) use super::mobile::{
    begin_window_drag, create_windows, deliver_paste, manage_platform_state, on_paste_window_focus,
    on_window_event, open_data_directory, prompt_save_destination, read_clipboard_files,
    read_clipboard_image, read_clipboard_text, requires_paste_window, setup_desktop_shell,
    set_virtual_file_clipboard, should_ignore_paste_focus_loss, should_skip_clipboard_poll,
    show_detached_toast, supports_native_file_export, supports_virtual_file_paste,
    synthesize_paste, write_clipboard_files, write_clipboard_image, write_clipboard_text,
};

pub(crate) fn register_plugins(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder.plugin(tauri_plugin_cliproam_share_receiver::init())
}

/// 分享项先复制到应用缓存，再按本地捕获一样导入历史；分享源文件可能
/// 随时被系统回收，不能直接引用。
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
        let _ = crate::platforms::write_clipboard_text(app, &rich_text);
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

pub(crate) fn consume_pending_shares(app: &AppHandle) -> Result<ShareImportSummary, String> {
    let mut imported = ShareImportSummary::default();
    for share in app.share_receiver().pending().map_err(|error| error.to_string())? {
        let summary = import_android_share(app, &share)?;
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
