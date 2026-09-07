//! Content-addressed clipboard storage.
//!
//! A `files` entry carries one nested map (`FileInfo`): file leaves are
//! identified by `sha256(bytes)` plus their size, directories are nested maps,
//! and an image entry references a single blob. Where the bytes actually live
//! — the machine that copied them, the local blob cache, or the server pool —
//! is answered separately, so the same content is never stored twice.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

pub const HASH_READ_BUFFER: usize = 512 * 1024;

/// A `files` entry's structure is one nested map. A file leaf carries the
/// content id and byte size; a directory is another such map keyed by child
/// name, and an empty map is an empty directory. Root names are the top-level
/// keys. `f` stays empty while the background hash is still pending.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TreeNode {
    File { f: String, s: u64 },
    Dir(IndexMap<String, TreeNode>),
}

pub type FileInfo = IndexMap<String, TreeNode>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub file_id: String,
    pub size: u64,
    /// Base64-encoded WebP thumbnail, so lists never need the full image.
    pub thumbnail: String,
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
    /// Original absolute paths, in the same order as `FileInfo`'s keys.
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
    /// Present when `kind` is `"files"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_info: Option<FileInfo>,
    /// Present when `kind` is `"image"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_info: Option<ImageInfo>,
    /// Content ids the server's pool does not hold. Only server responses fill
    /// it in; clients ignore it when publishing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<Vec<String>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_info: Option<FileInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_info: Option<ImageInfo>,
}

pub struct CollectedTree {
    pub file_info: FileInfo,
    pub sources: LocalSources,
}

/// Whether a node is a file leaf. A directory may legitimately contain a child
/// named "f" — its value is an object, not a string, so this stays unambiguous.
pub fn is_file_node(node: &TreeNode) -> bool {
    matches!(node, TreeNode::File { .. })
}

/// Every content the map references, de-duplicated in encounter order, with
/// the size each leaf reports.
pub fn tree_contents(file_info: &FileInfo) -> Vec<(String, u64)> {
    let mut contents = Vec::new();
    let mut seen = HashSet::new();
    fn walk(dir: &IndexMap<String, TreeNode>, seen: &mut HashSet<String>, contents: &mut Vec<(String, u64)>) {
        for node in dir.values() {
            match node {
                TreeNode::File { f, s } => {
                    if !f.is_empty() && seen.insert(f.clone()) {
                        contents.push((f.clone(), *s));
                    }
                }
                TreeNode::Dir(children) => walk(children, seen, contents),
            }
        }
    }
    walk(file_info, &mut seen, &mut contents);
    contents
}

/// Resolves a `/`-separated path inside the map, e.g. `bundle/sub/a.txt`.
pub fn tree_node_at_path<'a>(file_info: &'a FileInfo, path: &str) -> Option<&'a TreeNode> {
    let mut children = file_info;
    let segments = path.split('/').collect::<Vec<_>>();
    let (last, parents) = segments.split_last()?;
    for segment in parents {
        children = match children.get(*segment)? {
            TreeNode::Dir(children) => children,
            TreeNode::File { .. } => return None,
        };
    }
    children.get(*last)
}

/// The mutable map that holds the leaf of a `/`-separated path, e.g.
/// `bundle/sub/a.txt` → the directory map containing `"a.txt"`.
pub fn tree_parent_at_path<'a>(
    file_info: &'a mut FileInfo,
    path: &str,
) -> Option<&'a mut IndexMap<String, TreeNode>> {
    let mut segments = path.split('/').peekable();
    let mut children = file_info;
    loop {
        let segment = segments.next()?;
        if segments.peek().is_none() {
            return Some(children);
        }
        children = match children.get_mut(segment)? {
            TreeNode::Dir(children) => children,
            TreeNode::File { .. } => return None,
        };
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
    let mut file_info = FileInfo::default();
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
        sources.roots.push(path.display().to_string());
        collect_node(path, &metadata, &mut file_info, &mut sources, &name)?;
    }
    if file_info.is_empty() {
        return Err("剪贴板中没有可用的文件".to_string());
    }
    Ok(CollectedTree { file_info, sources })
}

fn collect_node(
    path: &Path,
    metadata: &fs::Metadata,
    file_info: &mut FileInfo,
    sources: &mut LocalSources,
    name: &str,
) -> Result<(), String> {
    if !metadata.is_dir() {
        file_info.insert(name.to_string(), TreeNode::File { f: String::new(), s: metadata.len() });
        sources.files.push(LocalSource {
            path: name.to_string(),
            source: path.display().to_string(),
            size: metadata.len(),
            modified_at: modified_millis(metadata),
            file_id: None,
        });
        return Ok(());
    }
    let children = collect_dir(path, sources, name)?;
    file_info.insert(name.to_string(), TreeNode::Dir(children));
    Ok(())
}

/// Reads a directory into its nested representation; an empty directory comes
/// back as an empty map, so it survives a round trip.
fn collect_dir(path: &Path, sources: &mut LocalSources, prefix: &str) -> Result<FileInfo, String> {
    let mut children = fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    children.sort_by_key(|child| child.file_name());
    let mut dir = FileInfo::default();
    for child in children {
        let child_path = child.path();
        // Following links could walk outside the copied tree entirely.
        let Ok(child_metadata) = fs::symlink_metadata(&child_path) else {
            continue;
        };
        if child_metadata.file_type().is_symlink() {
            continue;
        }
        let child_name = child.file_name().to_string_lossy().into_owned();
        let child_relative_path = format!("{prefix}/{child_name}");
        if child_metadata.is_dir() {
            let nested = collect_dir(&child_path, sources, &child_relative_path)?;
            dir.insert(child_name, TreeNode::Dir(nested));
            continue;
        }
        dir.insert(child_name.clone(), TreeNode::File { f: String::new(), s: child_metadata.len() });
        sources.files.push(LocalSource {
            path: child_relative_path,
            source: child_path.display().to_string(),
            size: child_metadata.len(),
            modified_at: modified_millis(&child_metadata),
            file_id: None,
        });
    }
    Ok(dir)
}

pub fn describe_roots(file_info: &FileInfo) -> String {
    let count = file_info.len();
    match count {
        0 => "文件".to_string(),
        1 => file_info.keys().next().expect("count is one").clone(),
        _ => format!("{} 等 {count} 项", file_info.keys().next().expect("count is nonzero")),
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

/// Never stats tree nodes: with hundreds of entries holding thousands of paths
/// each, a single `stat` per node would stall startup.
pub fn refresh_summary(
    entry: &mut ClipboardEntry,
    cached: &HashSet<String>,
    uploaded: &HashSet<String>,
    cache_dir: &Path,
) {
    let mut summary = EntrySummary::default();
    let contents = match (&entry.file_info, &entry.image_info) {
        (Some(file_info), _) => {
            summary.root_kind = match file_info.len() {
                0 => String::new(),
                1 => match file_info.values().next().expect("count is one") {
                    TreeNode::File { .. } => "file".to_string(),
                    TreeNode::Dir(_) => "dir".to_string(),
                },
                _ => "mixed".to_string(),
            };
            summary.file_count = file_count(file_info);
            summary.hashed_count = hashed_count(file_info);
            tree_contents(file_info)
        }
        (None, Some(image)) => {
            summary.root_kind = "file".to_string();
            summary.file_count = 1;
            summary.hashed_count = 1;
            vec![(image.file_id.clone(), image.size)]
        }
        (None, None) => Vec::new(),
    };
    let local = entry
        .sources
        .files
        .iter()
        .filter_map(|source| source.file_id.as_deref())
        .collect::<HashSet<_>>();
    summary.content_count = contents.len() as u64;
    for (file_id, size) in &contents {
        summary.total_size += size;
        summary.max_file_size = summary.max_file_size.max(*size);
        if uploaded.contains(file_id) {
            summary.uploaded_count += 1;
        }
        if cached.contains(file_id) || local.contains(file_id.as_str()) {
            summary.ready_count += 1;
            if !uploaded.contains(file_id) {
                summary.uploadable_size =
                    Some(summary.uploadable_size.unwrap_or(u64::MAX).min(*size));
            }
        } else {
            summary.pending_count += 1;
            summary.pending_size += size;
        }
    }
    if entry.kind == "image" {
        let file_id = entry.image_info.as_ref().map(|image| image.file_id.clone());
        summary.preview_path = file_id
            .and_then(|file_id| readable_path(cache_dir, cached, entry, &file_id))
            .map(|path| path.display().to_string());
    }
    entry.summary = summary;
}

fn file_count(file_info: &FileInfo) -> u64 {
    fn walk(dir: &IndexMap<String, TreeNode>) -> u64 {
        dir.values()
            .map(|node| match node {
                TreeNode::File { .. } => 1,
                TreeNode::Dir(children) => walk(children),
            })
            .sum()
    }
    walk(file_info)
}

fn hashed_count(file_info: &FileInfo) -> u64 {
    fn walk(dir: &IndexMap<String, TreeNode>) -> u64 {
        dir.values()
            .map(|node| match node {
                TreeNode::File { f, .. } => u64::from(!f.is_empty()),
                TreeNode::Dir(children) => walk(children),
            })
            .sum()
    }
    walk(file_info)
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

/// Materialises a file map under `destination`. Hard links keep repeated
/// content down to a single copy on disk; `link = false` forces real copies
/// for destinations the user owns.
pub fn rebuild_tree(
    destination: &Path,
    file_info: &FileInfo,
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
    link: bool,
) -> Result<usize, String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut written = 0usize;
    build_dir(file_info, destination, resolve, link, &mut written)?;
    Ok(written)
}

fn build_dir(
    dir: &FileInfo,
    base: &Path,
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
    link: bool,
    written: &mut usize,
) -> Result<(), String> {
    for (name, node) in dir {
        let target = base.join(clipboard_relative_path(name)?);
        match node {
            TreeNode::Dir(children) => {
                fs::create_dir_all(&target).map_err(|error| error.to_string())?;
                build_dir(children, &target, resolve, link, written)?;
            }
            TreeNode::File { f, .. } => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let source = resolve(f).ok_or_else(|| format!("文件内容不可用：{name}"))?;
                if target.exists() {
                    fs::remove_file(&target).map_err(|error| error.to_string())?;
                }
                if link && fs::hard_link(&source, &target).is_ok() {
                    *written += 1;
                    continue;
                }
                fs::copy(&source, &target).map_err(|error| format!("无法写入 {name}：{error}"))?;
                *written += 1;
            }
        }
    }
    Ok(())
}

/// Keeps the local paths of an entry that came back from the server, but only
/// when the structure still matches what this machine copied.
pub fn preserve_local_sources(remote: &mut ClipboardEntry, local: &ClipboardEntry) {
    let Some(file_info) = remote.file_info.as_ref() else {
        return;
    };
    if local.sources.roots.len() != file_info.len() {
        return;
    }
    let paths = collect_paths(file_info);
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

/// Every `/`-separated path the map's leaves live at.
fn collect_paths(file_info: &FileInfo) -> HashSet<String> {
    let mut paths = HashSet::new();
    fn walk(dir: &IndexMap<String, TreeNode>, prefix: &str, paths: &mut HashSet<String>) {
        for (name, node) in dir {
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            match node {
                TreeNode::File { .. } => {
                    paths.insert(path);
                }
                TreeNode::Dir(children) => walk(children, &path, paths),
            }
        }
    }
    walk(file_info, "", &mut paths);
    paths
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
                ClipboardTreeFile { p: "bundle/first.bin".to_string(), f: file_id.clone(), s: None },
                ClipboardTreeFile { p: "bundle/sub/second.bin".to_string(), f: file_id.clone(), s: None },
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
                .map(|node| ClipboardTreeFile { p: node.p.clone(), f: "a".repeat(64), s: None })
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
                    ClipboardTreeFile { p: "bundle/a".to_string(), f: uploaded.clone(), s: None },
                    ClipboardTreeFile { p: "bundle/b".to_string(), f: local.clone(), s: None },
                    // Still waiting on the background hash.
                    ClipboardTreeFile { p: "bundle/c".to_string(), f: String::new(), s: None },
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
