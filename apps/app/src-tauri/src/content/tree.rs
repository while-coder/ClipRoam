use indexmap::IndexMap;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use super::{
    clipboard_relative_path, sanitize_root_name, unique_root_name,
    ClipboardEntry, CollectedTree, EntrySummary, FileInfo, LocalSource, LocalSources, TreeNode,
};
use crate::file::cached_file_path;
use crate::utils::modified_millis;

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

/// Everything `refresh_summary` reads beyond the entry itself, gathered once
/// per command instead of once per entry: the server-pool set comes from the
/// `files` table's in-memory cache, and the blob set is one directory scan —
/// the disk is the source of truth for local content, so there is no
/// long-lived mirror to invalidate.
pub struct SummaryContext<'a> {
    /// Content ids the server pool holds (`files` table, `stored = 1`).
    pub stored: &'a HashSet<String>,
    /// Content ids this machine has a blob file for on disk.
    pub blobs: &'a HashSet<String>,
    pub cache_dir: &'a Path,
}

impl SummaryContext<'_> {
    pub fn readable_path(&self, entry: &ClipboardEntry, file_id: &str) -> Option<PathBuf> {
        readable_path(self.cache_dir, entry, file_id)
    }
}

/// Never stats tree nodes: with hundreds of entries holding thousands of paths
/// each, a single `stat` per node would stall startup.
pub fn refresh_summary(entry: &mut ClipboardEntry, context: &SummaryContext) {
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
            summary.file_count = count_files(file_info, false);
            summary.hashed_count = count_files(file_info, true);
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
        if context.stored.contains(file_id) {
            summary.stored_count += 1;
        }
        // A locally readable content is uploadable from this device; everything
        // else still pends a download or a source re-hash.
        if context.blobs.contains(file_id) || local.contains(file_id.as_str()) {
            summary.ready_count += 1;
            summary.uploadable_size = Some(summary.uploadable_size.unwrap_or(u64::MAX).min(*size));
        } else {
            summary.pending_count += 1;
            summary.pending_size += size;
        }
    }
    if entry.kind == "image" {
        let file_id = entry.image_info.as_ref().map(|image| image.file_id.clone());
        summary.preview_path = file_id
            .and_then(|file_id| context.readable_path(entry, &file_id))
            .map(|path| path.display().to_string());
    }
    entry.summary = summary;
}

/// Counts the tree's file leaves; `hashed_only` keeps only leaves whose
/// content id the background hash has already resolved.
fn count_files(file_info: &FileInfo, hashed_only: bool) -> u64 {
    fn walk(dir: &IndexMap<String, TreeNode>, hashed_only: bool) -> u64 {
        dir.values()
            .map(|node| match node {
                TreeNode::File { f, .. } => u64::from(!hashed_only || !f.is_empty()),
                TreeNode::Dir(children) => walk(children, hashed_only),
            })
            .sum()
    }
    walk(file_info, hashed_only)
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

pub fn readable_path(cache_dir: &Path, entry: &ClipboardEntry, file_id: &str) -> Option<PathBuf> {
    // `cached_file_path` stats the blob candidates itself, so the disk is
    // checked live and a stale in-memory set can never disagree with it.
    if let Some(path) = cached_file_path(cache_dir, file_id) {
        return Some(path);
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
