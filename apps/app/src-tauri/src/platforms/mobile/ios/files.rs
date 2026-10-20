use crate::files::SaveTarget;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
pub(crate) async fn save_dialog(
    app: AppHandle,
    single: bool,
    file_name: String,
) -> Result<Option<SaveTarget>, String> {
    // Documents is exposed in the Files app by the iOS Info.plist settings.
    // Keep exported files separate from the private Application Support cache.
    crate::files::validate_component(&file_name)?;
    let downloads = app
        .path()
        .document_dir()
        .map_err(|error| error.to_string())?
        .join("Downloads");
    std::fs::create_dir_all(&downloads).map_err(|error| error.to_string())?;
    if single {
        let name = std::path::Path::new(&file_name);
        let stem = name.file_stem().unwrap_or_default().to_string_lossy();
        let extension = name
            .extension()
            .map(|value| format!(".{}", value.to_string_lossy()))
            .unwrap_or_default();
        let mut target = downloads.join(&file_name);
        let mut index = 1;
        while target.exists() {
            target = downloads.join(format!("{stem} ({index}){extension}"));
            index += 1;
        }
        Ok(Some(SaveTarget::Path(target)))
    } else {
        // Each batch gets its own directory, preserving names and folder trees
        // without overwriting an earlier export.
        let target = downloads.join(format!("下载-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&target).map_err(|error| error.to_string())?;
        Ok(Some(SaveTarget::Path(target)))
    }
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
