//! 内容 id 的校验与落盘位置：内容 id 决定字节落在磁盘哪里，所以在变成
//! 路径之前先校验形状。

use std::path::{Path, PathBuf};

/// Content ids decide where bytes land on disk, so the shape is validated
/// before it is ever turned into a path.
pub fn is_file_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn upload_image_path(cache_dir: &Path, file_id: &str) -> Option<PathBuf> {
    is_file_id(file_id).then(|| cache_dir.join("upload").join("images").join(file_id))
}

pub fn download_path(cache_dir: &Path, file_id: &str) -> Option<PathBuf> {
    is_file_id(file_id).then(|| cache_dir.join("download").join(file_id))
}

pub fn cached_file_path(cache_dir: &Path, file_id: &str) -> Option<PathBuf> {
    [upload_image_path(cache_dir, file_id), download_path(cache_dir, file_id)]
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
}
