use crate::files::{NativeShare, SaveTarget};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
pub(crate) async fn save_dialog(
    app: AppHandle,
    single: bool,
    file_name: String,
) -> Result<Option<SaveTarget>, String> {
    let _ = (app, single, file_name);
    Ok(None)
}

pub(crate) fn pick_files(
    app: AppHandle,
    mode: String,
    scope: String,
) -> Result<Vec<String>, String> {
    let dialog = app.dialog().file();
    if mode == "folder" {
        return Err("ios 暂不支持文件夹导入，请选择文件".to_string());
    }
    let selected = dialog.blocking_pick_files();
    let _ = scope;
    let paths = selected
        .unwrap_or_default()
        .into_iter()
        .map(|path| path.into_path().map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(paths
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

pub(crate) fn pending_shares(app: AppHandle, scope: String) -> Result<Vec<NativeShare>, String> {
    let _ = (app, scope);
    Ok(Vec::new())
}

pub(crate) fn ack_share(app: AppHandle, id: String) -> Result<(), String> {
    let _ = (app, id);
    Ok(())
}
