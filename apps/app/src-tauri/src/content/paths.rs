use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use super::hash::is_file_id;

/// Content ids decide where bytes land on disk, so the shape is validated
/// before it is ever turned into a path.
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

pub fn modified_millis(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis() as u64)
}

pub fn clipboard_relative_path(relative_path: &str) -> Result<PathBuf, String> {
    let mut path = PathBuf::new();
    for component in Path::new(relative_path).components() {
        match component {
            Component::Normal(component) => path.push(component),
            _ => return Err("文件相对路径不合法".to_string()),
        }
    }
    if path.as_os_str().is_empty() {
        return Err("文件相对路径为空".to_string());
    }
    Ok(path)
}

/// Root names double as path components when the tree is rebuilt, so the
/// characters a filesystem would reject are replaced up front.
pub(super) fn sanitize_root_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|character| {
            if matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned
    }
}

/// `docs`, `docs (2)`, `docs (3)` — copying `D:\a\docs` and `E:\b\docs` at once
/// must not collapse both trees into one.
pub fn unique_root_name(base: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let (stem, extension) = match base.rfind('.') {
        Some(index) if index > 0 => (&base[..index], &base[index..]),
        _ => (base, ""),
    };
    let name = (2u32..)
        .map(|counter| format!("{stem} ({counter}){extension}"))
        .find(|candidate| !used.contains(candidate))
        .expect("the counter range is unbounded");
    used.insert(name.clone());
    name
}

#[cfg(test)]
mod tests {
    use super::super::hash::hash_bytes;
    use super::{download_path, upload_image_path};
    use std::path::Path;

    #[test]
    fn cache_paths_reject_ids_that_are_not_content_hashes() {
        let cache = Path::new("cache");
        assert!(upload_image_path(cache, "../escape").is_none());
        assert!(download_path(cache, &"A".repeat(64)).is_none());
        let file_id = hash_bytes(b"content");
        assert_eq!(
            upload_image_path(cache, &file_id).unwrap(),
            cache.join("upload").join("images").join(&file_id)
        );
        assert_eq!(
            download_path(cache, &file_id).unwrap(),
            cache.join("download").join(&file_id)
        );
    }
}
