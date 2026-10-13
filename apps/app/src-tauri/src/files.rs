//! Filesystem and OS file-picker primitives. No history, hash or task state.
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Serialize;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};
use tauri::{AppHandle, Manager};

pub(crate) fn validate_component(value: &str) -> Result<(), String> {
    if value.is_empty()
        || Path::new(value).components().count() != 1
        || !matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err("目录名称不合法".into());
    }
    Ok(())
}
#[cfg(target_os = "android")]
pub(crate) fn scope_directory(app: &AppHandle, scope: &str) -> Result<PathBuf, String> {
    validate_component(scope)?;
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("histories")
        .join(scope)
        .join("files"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativePaths {
    app_dir: String,
}
#[tauri::command]
pub(crate) fn native_paths(app: AppHandle) -> Result<NativePaths, String> {
    Ok(NativePaths {
        app_dir: app
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileStat {
    path: String,
    name: String,
    directory: bool,
    symlink: bool,
    size: u64,
    modified_at: Option<u64>,
}
fn stat(path: &Path) -> std::io::Result<FileStat> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(FileStat {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        directory: metadata.is_dir(),
        symlink: metadata.file_type().is_symlink(),
        size: metadata.len(),
        modified_at: modified_millis(&metadata),
    })
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_fs_stat(paths: Vec<String>) -> Result<Vec<Option<FileStat>>, String> {
    paths
        .iter()
        .map(|path| match stat(Path::new(path)) {
            Ok(value) => Ok(Some(value)),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error.to_string()),
        })
        .collect()
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_fs_list(path: String) -> Result<Vec<FileStat>, String> {
    fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .map(|entry| {
            entry
                .and_then(|entry| stat(&entry.path()))
                .map_err(|e| e.to_string())
        })
        .collect()
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_read_image(path: String) -> Result<crate::clipboard::ClipboardData, String> {
    crate::clipboard::image_data(&fs::read(path).map_err(|e| e.to_string())?)
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_fs_read(path: String, offset: u64, length: usize) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut bytes = vec![0; length.min(8 * 1024 * 1024)];
    let count = file.read(&mut bytes).map_err(|e| e.to_string())?;
    bytes.truncate(count);
    Ok(BASE64.encode(bytes))
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_fs_write(path: String, data: String, append: bool) -> Result<(), String> {
    let path = Path::new(&path);
    ensure_parent_dir(path)?;
    let bytes = BASE64.decode(data).map_err(|e| e.to_string())?;
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|e| e.to_string())
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_fs_move(source: String, destination: String) -> Result<(), String> {
    ensure_parent_dir(Path::new(&destination))?;
    fs::rename(source, destination).map_err(|e| e.to_string())
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_fs_copy(
    app: AppHandle,
    source: String,
    destination: String,
    name: Option<String>,
) -> Result<String, String> {
    crate::platform::copy_file(&app, &source, &destination, name.as_deref())
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_fs_mkdir(
    app: AppHandle,
    path: String,
    name: Option<String>,
) -> Result<String, String> {
    crate::platform::create_directory(&app, &path, name.as_deref())
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_fs_remove(app: AppHandle, path: String) -> Result<(), String> {
    crate::platform::remove_file(&app, &path)
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
#[allow(dead_code)] // Platform-specific variants are constructed by their OS adapter.
pub(crate) enum SaveTarget {
    Path(PathBuf),
    DocumentTree(String),
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn native_save_dialog(
    app: AppHandle,
    single: bool,
    file_name: String,
) -> Result<Option<SaveTarget>, String> {
    crate::platform::save_dialog(app, single, file_name).await
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_pick_files(
    app: AppHandle,
    mode: String,
    scope: String,
) -> Result<Vec<String>, String> {
    crate::platform::pick_files(app, mode, scope)
}

#[derive(Serialize)]
pub(crate) struct NativeShare {
    pub(crate) id: String,
    pub(crate) text: Option<String>,
    pub(crate) html: Option<String>,
    pub(crate) images: Vec<String>,
    pub(crate) paths: Vec<String>,
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_pending_shares(
    app: AppHandle,
    scope: String,
) -> Result<Vec<NativeShare>, String> {
    crate::platform::pending_shares(app, scope)
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_ack_share(app: AppHandle, id: String) -> Result<(), String> {
    crate::platform::ack_share(app, id)
}

pub(crate) fn modified_millis(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis() as u64)
}
pub(crate) fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(crate) fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    ensure_parent_dir(path)?;
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    let temp = PathBuf::from(name);
    fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        e.to_string()
    })
}
