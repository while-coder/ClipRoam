//! 普通文件路径的具体实现，由各系统适配器复用。
use crate::files::{ensure_parent_dir, validate_component};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

fn destination(path: &str, name: Option<&str>) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    match name {
        Some(name) => {
            validate_component(name)?;
            Ok(path.join(name))
        }
        None => Ok(path),
    }
}

pub(crate) fn copy_file(
    _app: &AppHandle,
    source: &str,
    path: &str,
    name: Option<&str>,
) -> Result<String, String> {
    let target = destination(path, name)?;
    if Path::new(source) != target {
        ensure_parent_dir(&target)?;
        fs::copy(source, &target).map_err(|error| error.to_string())?;
    }
    Ok(target.to_string_lossy().into_owned())
}

pub(crate) fn create_directory(
    _app: &AppHandle,
    path: &str,
    name: Option<&str>,
) -> Result<String, String> {
    let target = destination(path, name)?;
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    Ok(target.to_string_lossy().into_owned())
}

pub(crate) fn remove_file(app: &AppHandle, path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if !path.exists() {
        return Ok(());
    }
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let resolved = path.canonicalize().map_err(|error| error.to_string())?;
    if resolved == root || !resolved.starts_with(&root) {
        return Err("只能清理应用缓存中的文件".into());
    }
    if fs::symlink_metadata(path)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|error| error.to_string())
}
