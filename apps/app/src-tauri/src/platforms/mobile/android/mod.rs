//! Android OS document access.
pub(crate) use super::{
    open_data_directory, read_clipboard_files, read_clipboard_image, read_clipboard_text,
    show_notification, write_clipboard_files, write_clipboard_image, write_clipboard_text,
};
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;

pub(crate) fn resolve_picker_paths(
    app: &AppHandle,
    selected: Vec<tauri_plugin_dialog::FilePath>,
    scope: &str,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let mut uris = Vec::new();
    for path in selected {
        match path {
            tauri_plugin_dialog::FilePath::Url(uri) if uri.scheme() == "content" => {
                uris.push(uri.to_string())
            }
            other => paths.push(other.into_path().map_err(|e| e.to_string())?),
        }
    }
    if !uris.is_empty() {
        let directory = crate::files::scope_directory(app, scope)?
            .join("picked")
            .join(uuid::Uuid::new_v4().to_string());
        let imported = app
            .share_receiver()
            .copy_documents(&uris, &directory.to_string_lossy())
            .map_err(|e| e.to_string())?;
        paths.extend(imported.paths.into_iter().map(PathBuf::from));
    }
    Ok(paths)
}

mod files;
pub(crate) use super::{
    begin_window_drag, capture_paste_target, create_windows, deliver_paste, manage_platform_state,
    on_paste_window_focus, on_window_event, register_plugins, should_skip_clipboard_poll,
};
pub(crate) use files::{
    ack_share, copy_file, create_directory, pending_shares, pick_files, remove_file, save_dialog,
};
