//! Content-addressed clipboard storage.
//!
//! An entry only carries structure (`ClipboardTree`) plus a de-duplicated list
//! of contents (`ClipboardFile`), each identified by `sha256(bytes)`. Where the
//! bytes actually live — the machine that copied them, or the local blob cache —
//! is answered separately, so the same content is never stored twice.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};
use uuid::Uuid;

pub const HASH_READ_BUFFER: usize = 512 * 1024;
pub const TREE_VERSION: u32 = 2;
const PACK_MAGIC: &[u8] = b"CLIPROAM-PACK-1\n";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardTreeRoot {
    pub name: String,
    /// `"file"` or `"dir"`.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardTreeFile {
    /// Relative path inside the entry, always `/`-separated.
    pub p: String,
    /// Content id; empty while the background hash is still pending.
    pub f: String,
    /// Original file size. Transfer packs have their own size in `entry.files`;
    /// this value is what virtual-file paste exposes to the destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s: Option<u64>,
    /// Optional transfer pack containing `f`. Paths still address the original
    /// content, while upload/download operates on this bounded pack blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardTree {
    pub v: u32,
    pub roots: Vec<ClipboardTreeRoot>,
    #[serde(default)]
    pub dirs: Vec<String>,
    #[serde(default)]
    pub files: Vec<ClipboardTreeFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFile {
    pub file_id: String,
    pub size: u64,
    /// Whether the server already holds the content.
    #[serde(default)]
    pub available: bool,
}

/// Where a tree path came from on this machine. Never sent to the server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSource {
    pub path: String,
    pub source: String,
    pub size: u64,
    #[serde(default)]
    pub modified_at: Option<u64>,
    #[serde(default)]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSources {
    /// Original absolute paths, in the same order as `ClipboardTree::roots`.
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub files: Vec<LocalSource>,
}

/// Aggregates the UI needs without shipping a whole tree to the frontend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntrySummary {
    /// Root shape used by the list icon without sending the whole tree.
    /// Values are `"file"`, `"dir"`, `"mixed"`, or empty when unknown.
    #[serde(default)]
    pub root_kind: String,
    pub file_count: u64,
    pub hashed_count: u64,
    pub content_count: u64,
    pub total_size: u64,
    pub max_file_size: u64,
    pub uploaded_count: u64,
    pub ready_count: u64,
    pub pending_count: u64,
    pub pending_size: u64,
    /// Size of the smallest content this device could still upload, so the UI
    /// can tell "nothing left to upload" from "everything is too large".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploadable_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardEntry {
    pub id: String,
    pub kind: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtf: Option<String>,
    /// Base64-encoded WebP thumbnail carried with image metadata, so lists do
    /// not need the full-resolution clipboard image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<ClipboardTree>,
    #[serde(default)]
    pub files: Vec<ClipboardFile>,
    pub source_device_id: String,
    pub created_at: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub summary: EntrySummary,
    #[serde(skip)]
    pub sources: LocalSources,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardEntryExtra {
    pub html: Option<String>,
    pub rtf: Option<String>,
    pub thumbnail: Option<String>,
    pub tree: Option<ClipboardTree>,
}

pub struct CollectedTree {
    pub tree: ClipboardTree,
    pub sources: LocalSources,
}

pub fn new_tree() -> ClipboardTree {
    ClipboardTree {
        v: TREE_VERSION,
        roots: Vec::new(),
        dirs: Vec::new(),
        files: Vec::new(),
    }
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_READ_BUFFER];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(to_hex(&hasher.finalize()))
}

pub fn transfer_file_id(node: &ClipboardTreeFile) -> &str {
    node.b.as_deref().unwrap_or(&node.f)
}

pub fn unpacked_file_path(cache_dir: &Path, pack_id: &str, file_id: &str) -> Option<PathBuf> {
    (is_file_id(pack_id) && is_file_id(file_id))
        .then(|| cache_dir.join("unpacked").join(pack_id).join(file_id))
}

/// Creates a deterministic, uncompressed pack. Compression is deliberately
/// omitted: the optimization targets per-file protocol and filesystem
/// overhead, and a bounded pack remains cheap to retry and stream.
pub fn create_pack(temp_path: &Path, contents: &[(String, PathBuf)]) -> Result<(String, u64), String> {
    if let Some(parent) = temp_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let result = (|| {
        let mut output = fs::File::create(temp_path).map_err(|error| error.to_string())?;
        output.write_all(PACK_MAGIC).map_err(|error| error.to_string())?;
        output
            .write_all(&(contents.len() as u32).to_le_bytes())
            .map_err(|error| error.to_string())?;
        for (file_id, source) in contents {
            if !is_file_id(file_id) {
                return Err("打包内容标识不合法".to_string());
            }
            let size = fs::metadata(source).map_err(|error| error.to_string())?.len();
            output.write_all(file_id.as_bytes()).map_err(|error| error.to_string())?;
            output.write_all(&size.to_le_bytes()).map_err(|error| error.to_string())?;

            let mut input = fs::File::open(source).map_err(|error| error.to_string())?;
            let mut hasher = Sha256::new();
            let mut copied = 0u64;
            let mut buffer = vec![0u8; HASH_READ_BUFFER];
            loop {
                let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                copied += count as u64;
                hasher.update(&buffer[..count]);
                output.write_all(&buffer[..count]).map_err(|error| error.to_string())?;
            }
            if copied != size || to_hex(&hasher.finalize()) != *file_id {
                return Err("打包时源文件发生了变化".to_string());
            }
        }
        output.flush().map_err(|error| error.to_string())?;
        let size = output.metadata().map_err(|error| error.to_string())?.len();
        drop(output);
        Ok((hash_file(temp_path)?, size))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

/// Expands a pack into an id-only directory. No archived path is trusted, and
/// every member is verified against its own content id before it is exposed.
pub fn unpack_pack(pack_path: &Path, destination: &Path) -> Result<(), String> {
    if destination.join(".complete").is_file() {
        return Ok(());
    }
    let temporary = destination.with_extension(format!("tmp-{}", Uuid::new_v4()));
    let result = (|| {
        fs::create_dir_all(&temporary).map_err(|error| error.to_string())?;
        let mut input = fs::File::open(pack_path).map_err(|error| error.to_string())?;
        let mut magic = vec![0u8; PACK_MAGIC.len()];
        input.read_exact(&mut magic).map_err(|error| error.to_string())?;
        if magic != PACK_MAGIC {
            return Err("文件包格式不受支持".to_string());
        }
        let mut count_bytes = [0u8; 4];
        input.read_exact(&mut count_bytes).map_err(|error| error.to_string())?;
        let count = u32::from_le_bytes(count_bytes);
        if count > 1_000_000 {
            return Err("文件包项目过多".to_string());
        }
        for _ in 0..count {
            let mut id_bytes = [0u8; 64];
            input.read_exact(&mut id_bytes).map_err(|error| error.to_string())?;
            let file_id = std::str::from_utf8(&id_bytes).map_err(|error| error.to_string())?;
            if !is_file_id(file_id) {
                return Err("文件包内容标识不合法".to_string());
            }
            let mut size_bytes = [0u8; 8];
            input.read_exact(&mut size_bytes).map_err(|error| error.to_string())?;
            let size = u64::from_le_bytes(size_bytes);
            let target = temporary.join(file_id);
            let mut output = fs::File::create(&target).map_err(|error| error.to_string())?;
            let mut remaining = size;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0u8; HASH_READ_BUFFER];
            while remaining > 0 {
                let requested = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
                input.read_exact(&mut buffer[..requested]).map_err(|error| error.to_string())?;
                hasher.update(&buffer[..requested]);
                output.write_all(&buffer[..requested]).map_err(|error| error.to_string())?;
                remaining -= requested as u64;
            }
            if to_hex(&hasher.finalize()) != file_id {
                return Err("文件包内容校验失败".to_string());
            }
        }
        let mut trailing = [0u8; 1];
        if input.read(&mut trailing).map_err(|error| error.to_string())? != 0 {
            return Err("文件包包含多余数据".to_string());
        }
        fs::write(temporary.join(".complete"), b"").map_err(|error| error.to_string())?;
        if destination.exists() {
            fs::remove_dir_all(destination).map_err(|error| error.to_string())?;
        }
        fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

pub fn is_file_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

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
fn sanitize_root_name(name: &str) -> String {
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

/// Walks the copied paths collecting structure only — hashing happens later on
/// a background thread so a large folder shows up in the UI immediately.
pub fn collect_tree(paths: &[PathBuf]) -> Result<CollectedTree, String> {
    let mut tree = new_tree();
    let mut sources = LocalSources::default();
    let mut used = HashSet::new();
    for path in paths {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        let base = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let name = unique_root_name(&sanitize_root_name(&base), &mut used);
        tree.roots.push(ClipboardTreeRoot {
            name: name.clone(),
            kind: if metadata.is_dir() { "dir" } else { "file" }.to_string(),
        });
        sources.roots.push(path.display().to_string());
        collect_node(path, &name, &metadata, &mut tree, &mut sources)?;
    }
    if tree.roots.is_empty() {
        return Err("剪贴板中没有可用的文件".to_string());
    }
    Ok(CollectedTree { tree, sources })
}

fn collect_node(
    path: &Path,
    relative_path: &str,
    metadata: &fs::Metadata,
    tree: &mut ClipboardTree,
    sources: &mut LocalSources,
) -> Result<(), String> {
    if !metadata.is_dir() {
        tree.files.push(ClipboardTreeFile {
            p: relative_path.to_string(),
            f: String::new(),
            s: Some(metadata.len()),
            b: None,
        });
        sources.files.push(LocalSource {
            path: relative_path.to_string(),
            source: path.display().to_string(),
            size: metadata.len(),
            modified_at: modified_millis(metadata),
            file_id: None,
        });
        return Ok(());
    }
    // Empty directories only exist in `dirs`, so they survive a round trip.
    tree.dirs.push(relative_path.to_string());
    let mut children = fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    children.sort_by_key(|child| child.file_name());
    for child in children {
        let child_path = child.path();
        // Following links could walk outside the copied tree entirely.
        let Ok(child_metadata) = fs::symlink_metadata(&child_path) else {
            continue;
        };
        if child_metadata.file_type().is_symlink() {
            continue;
        }
        let child_relative_path = format!("{relative_path}/{}", child.file_name().to_string_lossy());
        collect_node(&child_path, &child_relative_path, &child_metadata, tree, sources)?;
    }
    Ok(())
}

pub fn describe_roots(roots: &[ClipboardTreeRoot]) -> String {
    match roots.len() {
        0 => "文件".to_string(),
        1 => roots[0].name.clone(),
        count => format!("{} 等 {count} 项", roots[0].name),
    }
}

pub fn file_signature(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| {
            let metadata = fs::symlink_metadata(path).ok();
            let size = metadata.as_ref().map(|value| value.len()).unwrap_or_default();
            let modified_at = metadata.as_ref().and_then(modified_millis).unwrap_or_default();
            format!("{}:{size}:{modified_at}", path.to_string_lossy().to_ascii_lowercase())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The signature is taken over the copied roots, matching what the clipboard
/// monitor sees, so re-copying the same folder reuses the existing entry.
pub fn file_entry_signature(entry: &ClipboardEntry) -> String {
    let roots = entry.sources.roots.iter().map(PathBuf::from).collect::<Vec<_>>();
    file_signature(&roots)
}

/// Derives the de-duplicated content list from the tree, keeping whatever the
/// server already told us about each content.
pub fn rebuild_entry_files(entry: &mut ClipboardEntry) {
    let known = entry
        .files
        .drain(..)
        .map(|file| (file.file_id.clone(), file))
        .collect::<HashMap<_, _>>();
    let sizes = entry
        .sources
        .files
        .iter()
        .map(|source| (source.path.as_str(), source.size))
        .collect::<HashMap<_, _>>();
    let Some(tree) = entry.tree.as_ref() else {
        return;
    };
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for node in &tree.files {
        let transfer_id = transfer_file_id(node);
        if transfer_id.is_empty() || !seen.insert(transfer_id) {
            continue;
        }
        if let Some(file) = known.get(transfer_id) {
            files.push(file.clone());
            continue;
        }
        files.push(ClipboardFile {
            file_id: transfer_id.to_string(),
            size: sizes.get(node.p.as_str()).copied().unwrap_or_default(),
            available: false,
        });
    }
    entry.files = files;
}

/// Never stats tree nodes: with hundreds of entries holding thousands of paths
/// each, a single `stat` per node would stall startup.
pub fn refresh_summary(entry: &mut ClipboardEntry, cached: &HashSet<String>, cache_dir: &Path) {
    let mut summary = EntrySummary::default();
    if let Some(tree) = &entry.tree {
        summary.root_kind = match tree.roots.as_slice() {
            [] => String::new(),
            [root] => root.kind.clone(),
            _ => "mixed".to_string(),
        };
        summary.file_count = tree.files.len() as u64;
        summary.hashed_count = tree.files.iter().filter(|node| !node.f.is_empty()).count() as u64;
    }
    let local = entry
        .sources
        .files
        .iter()
        .filter_map(|source| source.file_id.as_deref())
        .collect::<HashSet<_>>();
    summary.content_count = entry.files.len() as u64;
    for file in &entry.files {
        summary.total_size += file.size;
        summary.max_file_size = summary.max_file_size.max(file.size);
        if file.available {
            summary.uploaded_count += 1;
        }
        if cached.contains(&file.file_id) || local.contains(file.file_id.as_str()) {
            summary.ready_count += 1;
            if !file.available {
                summary.uploadable_size =
                    Some(summary.uploadable_size.unwrap_or(u64::MAX).min(file.size));
            }
        } else {
            summary.pending_count += 1;
            summary.pending_size += file.size;
        }
    }
    if entry.kind == "image" {
        let file_id = entry.files.first().map(|file| file.file_id.clone());
        summary.preview_path = file_id
            .and_then(|file_id| readable_path(cache_dir, cached, entry, &file_id))
            .map(|path| path.display().to_string());
    }
    entry.summary = summary;
}

/// Resolves the original path a content came from, rejecting it when the file
/// has since been edited or replaced.
pub fn local_source_of(entry: &ClipboardEntry, file_id: &str) -> Option<PathBuf> {
    entry
        .sources
        .files
        .iter()
        .filter(|source| source.file_id.as_deref() == Some(file_id))
        .find_map(|source| {
            let path = PathBuf::from(&source.source);
            let metadata = fs::symlink_metadata(&path).ok()?;
            let unchanged = metadata.is_file()
                && metadata.len() == source.size
                && (source.modified_at.is_none() || modified_millis(&metadata) == source.modified_at);
            unchanged.then_some(path)
        })
}

/// A recorded source existed for this content, but it no longer resolves to
/// the same file. This is distinct from content that only lives on another
/// device and therefore must be downloaded rather than discarded.
pub fn local_source_was_lost(entry: &ClipboardEntry, file_id: &str) -> bool {
    entry
        .sources
        .files
        .iter()
        .any(|source| source.file_id.as_deref() == Some(file_id))
        && local_source_of(entry, file_id).is_none()
}

pub fn readable_path(
    cache_dir: &Path,
    cached: &HashSet<String>,
    entry: &ClipboardEntry,
    file_id: &str,
) -> Option<PathBuf> {
    if cached.contains(file_id) {
        if let Some(path) = cached_file_path(cache_dir, file_id) {
            return Some(path);
        }
    }
    local_source_of(entry, file_id)
}

/// Materialises a tree under `destination`. Hard links keep repeated content
/// down to a single copy on disk; `link = false` forces real copies for
/// destinations the user owns.
pub fn rebuild_tree(
    destination: &Path,
    tree: &ClipboardTree,
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
    link: bool,
) -> Result<usize, String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for directory in &tree.dirs {
        fs::create_dir_all(destination.join(clipboard_relative_path(directory)?))
            .map_err(|error| error.to_string())?;
    }
    let mut written = 0usize;
    for node in &tree.files {
        let target = destination.join(clipboard_relative_path(&node.p)?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let source = resolve(&node.f).ok_or_else(|| format!("文件内容不可用：{}", node.p))?;
        if target.exists() {
            fs::remove_file(&target).map_err(|error| error.to_string())?;
        }
        if link && fs::hard_link(&source, &target).is_ok() {
            written += 1;
            continue;
        }
        fs::copy(&source, &target).map_err(|error| format!("无法写入 {}：{error}", node.p))?;
        written += 1;
    }
    Ok(written)
}

/// Keeps the local paths of an entry that came back from the server, but only
/// when the structure still matches what this machine copied.
pub fn preserve_local_sources(remote: &mut ClipboardEntry, local: &ClipboardEntry) {
    let Some(tree) = remote.tree.as_ref() else {
        return;
    };
    if local.sources.roots.len() != tree.roots.len() {
        return;
    }
    let paths = tree.files.iter().map(|node| node.p.as_str()).collect::<HashSet<_>>();
    let mut sources = LocalSources {
        roots: local.sources.roots.clone(),
        files: local
            .sources
            .files
            .iter()
            .filter(|source| paths.contains(source.path.as_str()))
            .cloned()
            .collect(),
    };
    sources.files.shrink_to_fit();
    remote.sources = sources;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("cliproam-{name}-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).expect("create temporary directory");
            Self(path)
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn collect_tree_preserves_directory_tree() {
        let directory = TemporaryDirectory::new("tree");
        let root = directory.0.join("project");
        fs::create_dir_all(root.join("nested")).expect("create nested directory");
        fs::create_dir_all(root.join("empty")).expect("create empty directory");
        fs::write(root.join("root.txt"), "root").expect("write root file");
        fs::write(root.join("nested").join("child.txt"), "child").expect("write nested file");

        let collected = collect_tree(&[root]).expect("collect tree");

        assert_eq!(collected.tree.roots.len(), 1);
        assert_eq!(collected.tree.roots[0].kind, "dir");
        assert_eq!(
            collected.tree.dirs,
            vec!["project", "project/empty", "project/nested"]
        );
        assert_eq!(
            collected.tree.files.iter().map(|node| node.p.as_str()).collect::<Vec<_>>(),
            vec!["project/nested/child.txt", "project/root.txt"]
        );
        // Hashing is deferred, so contents start out unresolved.
        assert!(collected.tree.files.iter().all(|node| node.f.is_empty()));
        assert_eq!(collected.sources.files.len(), 2);
        assert_eq!(collected.sources.files[0].size, "child".len() as u64);
    }

    #[test]
    fn collect_tree_disambiguates_duplicate_root_names() {
        let directory = TemporaryDirectory::new("roots");
        let first = directory.0.join("a").join("docs");
        let second = directory.0.join("b").join("docs");
        fs::create_dir_all(&first).expect("create first root");
        fs::create_dir_all(&second).expect("create second root");
        fs::write(first.join("one.txt"), "one").expect("write first file");
        fs::write(second.join("two.txt"), "two").expect("write second file");

        let collected = collect_tree(&[first, second]).expect("collect tree");

        assert_eq!(
            collected.tree.roots.iter().map(|root| root.name.as_str()).collect::<Vec<_>>(),
            vec!["docs", "docs (2)"]
        );
        assert_eq!(
            collected.tree.files.iter().map(|node| node.p.as_str()).collect::<Vec<_>>(),
            vec!["docs/one.txt", "docs (2)/two.txt"]
        );
    }

    #[test]
    fn describe_roots_distinguishes_multiple_roots_from_files() {
        let roots = vec![
            ClipboardTreeRoot { name: "docs".to_string(), kind: "dir".to_string() },
            ClipboardTreeRoot { name: "readme.md".to_string(), kind: "file".to_string() },
            ClipboardTreeRoot { name: "assets".to_string(), kind: "dir".to_string() },
        ];

        assert_eq!(describe_roots(&roots), "docs 等 3 项");
    }

    #[test]
    fn rebuild_tree_restores_structure_and_shares_repeated_content() {
        let directory = TemporaryDirectory::new("rebuild");
        let source = directory.0.join("payload.bin");
        fs::write(&source, "shared").expect("write payload");
        let file_id = hash_bytes(b"shared");
        let tree = ClipboardTree {
            v: TREE_VERSION,
            roots: vec![ClipboardTreeRoot { name: "bundle".to_string(), kind: "dir".to_string() }],
            dirs: vec!["bundle".to_string(), "bundle/empty".to_string(), "bundle/sub".to_string()],
            files: vec![
                ClipboardTreeFile { p: "bundle/first.bin".to_string(), f: file_id.clone(), s: None, b: None },
                ClipboardTreeFile { p: "bundle/sub/second.bin".to_string(), f: file_id.clone(), s: None, b: None },
            ],
        };

        let destination = directory.0.join("view");
        let written = rebuild_tree(&destination, &tree, &|_| Some(source.clone()), false)
            .expect("rebuild tree");

        assert_eq!(written, 2);
        assert!(destination.join("bundle").join("empty").is_dir());
        assert_eq!(
            fs::read_to_string(destination.join("bundle").join("sub").join("second.bin")).unwrap(),
            "shared"
        );
    }

    #[test]
    fn pack_round_trip_verifies_and_restores_each_content() {
        let directory = TemporaryDirectory::new("pack");
        let first = directory.0.join("first.txt");
        let second = directory.0.join("second.txt");
        fs::write(&first, b"first payload").expect("write first");
        fs::write(&second, b"second payload").expect("write second");
        let first_id = hash_file(&first).expect("hash first");
        let second_id = hash_file(&second).expect("hash second");
        let pack = directory.0.join("contents.pack");

        let (pack_id, pack_size) = create_pack(
            &pack,
            &[(first_id.clone(), first), (second_id.clone(), second)],
        )
        .expect("create pack");
        assert_eq!(hash_file(&pack).expect("hash pack"), pack_id);
        assert_eq!(fs::metadata(&pack).expect("pack metadata").len(), pack_size);

        let unpacked = directory.0.join("unpacked");
        unpack_pack(&pack, &unpacked).expect("unpack");
        assert_eq!(fs::read(unpacked.join(first_id)).expect("read first"), b"first payload");
        assert_eq!(fs::read(unpacked.join(second_id)).expect("read second"), b"second payload");
        assert!(unpacked.join(".complete").is_file());
    }

    /// Mirrors `MAX_MESSAGE_BYTES` in `@cliproam/protocol`: a whole folder has to
    /// publish as a single message, so the tree encoding must stay compact.
    const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

    #[test]
    fn a_large_folder_publishes_as_one_message() {
        let directory = TemporaryDirectory::new("stress");
        let root = directory.0.join("archive");
        for bucket in 0..30 {
            let nested = root.join(format!("bucket-{bucket:02}")).join("nested");
            fs::create_dir_all(&nested).expect("create nested directory");
            for index in 0..100 {
                fs::write(nested.join(format!("file-{index:04}.bin")), b"payload").expect("write file");
            }
        }

        let collected = collect_tree(&[root]).expect("collect tree");
        // Measure the worst case: every path resolved to a distinct content id.
        let hashed = ClipboardTree {
            files: collected
                .tree
                .files
                .iter()
                .map(|node| ClipboardTreeFile { p: node.p.clone(), f: "a".repeat(64), s: None, b: None })
                .collect(),
            ..collected.tree.clone()
        };
        let bytes = serde_json::to_string(&hashed).expect("serialize tree").len();

        assert_eq!(collected.tree.files.len(), 3000);
        assert_eq!(collected.tree.dirs.len(), 61);
        assert!(bytes < MAX_MESSAGE_BYTES / 8, "3000 files encoded into {bytes} bytes");
    }

    #[test]
    fn refresh_summary_separates_ready_pending_and_uploadable_contents() {
        let uploaded = hash_bytes(b"uploaded");
        let local = hash_bytes(b"local");
        let remote = hash_bytes(b"remote");
        let mut entry = ClipboardEntry {
            id: "entry".to_string(),
            kind: "files".to_string(),
            content: String::new(),
            html: None,
            rtf: None,
            thumbnail: None,
            tree: Some(ClipboardTree {
                v: TREE_VERSION,
                roots: vec![ClipboardTreeRoot { name: "bundle".to_string(), kind: "dir".to_string() }],
                dirs: vec!["bundle".to_string()],
                files: vec![
                    ClipboardTreeFile { p: "bundle/a".to_string(), f: uploaded.clone(), s: None, b: None },
                    ClipboardTreeFile { p: "bundle/b".to_string(), f: local.clone(), s: None, b: None },
                    // Still waiting on the background hash.
                    ClipboardTreeFile { p: "bundle/c".to_string(), f: String::new(), s: None, b: None },
                ],
            }),
            files: vec![
                ClipboardFile { file_id: uploaded.clone(), size: 100, available: true },
                ClipboardFile { file_id: local.clone(), size: 300, available: false },
                ClipboardFile { file_id: remote, size: 500, available: false },
            ],
            source_device_id: "device".to_string(),
            created_at: "now".to_string(),
            pinned: false,
            summary: EntrySummary::default(),
            sources: LocalSources {
                roots: Vec::new(),
                files: vec![LocalSource {
                    path: "bundle/b".to_string(),
                    source: "D:/bundle/b".to_string(),
                    size: 300,
                    modified_at: None,
                    file_id: Some(local),
                }],
            },
        };

        refresh_summary(&mut entry, &HashSet::from([uploaded]), Path::new("cache"));

        let summary = &entry.summary;
        assert_eq!(summary.root_kind, "dir");
        assert_eq!((summary.file_count, summary.hashed_count), (3, 2));
        assert_eq!(summary.content_count, 3);
        assert_eq!((summary.total_size, summary.max_file_size), (900, 500));
        assert_eq!(summary.uploaded_count, 1);
        assert_eq!(summary.ready_count, 2);
        assert_eq!((summary.pending_count, summary.pending_size), (1, 500));
        // Only the cached-but-unpublished content can still be uploaded.
        assert_eq!(summary.uploadable_size, Some(300));
        assert!(summary.preview_path.is_none());
    }

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
