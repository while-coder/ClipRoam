use crate::files::SaveTarget;
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
