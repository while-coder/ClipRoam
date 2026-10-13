use crate::files::{ensure_parent_dir, scope_directory, NativeShare, SaveTarget};
use std::{fs, path::Path};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
pub(crate) async fn save_dialog(
    app: AppHandle,
    single: bool,
    file_name: String,
) -> Result<Option<SaveTarget>, String> {
    use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
    let _ = (single, file_name);
    app.share_receiver()
        .pick_directory()
        .map(|selection| selection.uri.map(SaveTarget::DocumentTree))
        .map_err(|error| error.to_string())
}

pub(crate) fn pick_files(
    app: AppHandle,
    mode: String,
    scope: String,
) -> Result<Vec<String>, String> {
    let dialog = app.dialog().file();
    if mode == "folder" {
        return Err("android 暂不支持文件夹导入，请选择文件".to_string());
    }
    let selected = dialog.blocking_pick_files();
    let paths = super::resolve_picker_paths(&app, selected.unwrap_or_default(), &scope)?;
    Ok(paths
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

pub(crate) fn pending_shares(app: AppHandle, scope: String) -> Result<Vec<NativeShare>, String> {
    use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
    let root = scope_directory(&app, &scope)?.join("share");
    app.share_receiver()
        .pending()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|share| {
            uuid::Uuid::parse_str(&share.id).map_err(|_| "分享标识不合法")?;
            let dir = root.join(&share.id);
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let mut images = Vec::new();
            let mut paths = Vec::new();
            let image = share.items.len() == 1 && share.items[0].mime_type.starts_with("image/");
            for (index, item) in share.items.iter().enumerate() {
                let name = Path::new(&item.name)
                    .file_name()
                    .filter(|name| !name.is_empty())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("shared-{index}"));
                // Each OS share item gets its own directory; duplicate names never overwrite.
                let target = dir.join(index.to_string()).join(name);
                ensure_parent_dir(&target)?;
                if !target.exists() {
                    fs::copy(&item.path, &target).map_err(|e| e.to_string())?;
                }
                if image {
                    images.push(target.to_string_lossy().into_owned());
                } else {
                    paths.push(target.to_string_lossy().into_owned());
                }
            }
            Ok(NativeShare {
                id: share.id,
                text: share.text,
                html: share.html,
                images,
                paths,
            })
        })
        .collect()
}

pub(crate) fn ack_share(app: AppHandle, id: String) -> Result<(), String> {
    use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
    app.share_receiver()
        .acknowledge(&id)
        .map_err(|e| e.to_string())
}

fn document_name(name: Option<&str>) -> Result<&str, String> {
    let name = name.ok_or("系统目录需要指定子项名称")?;
    crate::files::validate_component(name)?;
    Ok(name)
}
pub(crate) fn copy_file(
    app: &AppHandle,
    source: &str,
    destination: &str,
    name: Option<&str>,
) -> Result<String, String> {
    if !destination.starts_with("content://") {
        return crate::platforms::files::copy_file(app, source, destination, name);
    }
    use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
    app.share_receiver()
        .copy_file(source, destination, document_name(name)?)
        .map(|result| result.uri)
        .map_err(|error| error.to_string())
}
pub(crate) fn create_directory(
    app: &AppHandle,
    path: &str,
    name: Option<&str>,
) -> Result<String, String> {
    if !path.starts_with("content://") {
        return crate::platforms::files::create_directory(app, path, name);
    }
    use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
    app.share_receiver()
        .create_directory(path, document_name(name)?)
        .map(|result| result.uri)
        .map_err(|error| error.to_string())
}
pub(crate) fn remove_file(app: &AppHandle, path: &str) -> Result<(), String> {
    if !path.starts_with("content://") {
        return crate::platforms::files::remove_file(app, path);
    }
    use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
    app.share_receiver()
        .remove_document(path)
        .map_err(|error| error.to_string())
}
