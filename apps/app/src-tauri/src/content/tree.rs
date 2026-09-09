use indexmap::IndexMap;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use super::paths::{
    cached_file_path, clipboard_relative_path, modified_millis, sanitize_root_name, unique_root_name,
};
use super::{ClipboardEntry, CollectedTree, EntrySummary, FileInfo, LocalSource, LocalSources, TreeNode};

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
        if uploaded.contains(file_id) {
            summary.uploaded_count += 1;
        }
        if uploaded.contains(file_id) || cached.contains(file_id) || local.contains(file_id.as_str())
        {
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
    use super::super::hash::hash_bytes;
    use super::super::test_support::TemporaryDirectory;
    use super::*;
    use std::path::Path;

    #[test]
    fn collect_tree_preserves_directory_tree() {
        let directory = TemporaryDirectory::new("tree");
        let root = directory.0.join("project");
        fs::create_dir_all(root.join("nested")).expect("create nested directory");
        fs::create_dir_all(root.join("empty")).expect("create empty directory");
        fs::write(root.join("root.txt"), "root").expect("write root file");
        fs::write(root.join("nested").join("child.txt"), "child").expect("write nested file");

        let collected = collect_tree(&[root]).expect("collect tree");

        assert_eq!(collected.file_info.len(), 1);
        let TreeNode::Dir(project) = &collected.file_info["project"] else {
            panic!("project should be a directory");
        };
        assert_eq!(project.len(), 3);
        let TreeNode::Dir(nested) = &project["nested"] else {
            panic!("nested should be a directory");
        };
        let TreeNode::File { f, s } = &nested["child.txt"] else {
            panic!("child.txt should be a file");
        };
        // Hashing is deferred, so contents start out unresolved.
        assert_eq!((f.as_str(), *s), ("", "child".len() as u64));
        let TreeNode::Dir(empty) = &project["empty"] else {
            panic!("empty should be a directory");
        };
        assert!(empty.is_empty());
        let TreeNode::File { f, s } = &project["root.txt"] else {
            panic!("root.txt should be a file");
        };
        assert_eq!((f.as_str(), *s), ("", "root".len() as u64));
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
            collected.file_info.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["docs", "docs (2)"]
        );
        let TreeNode::Dir(docs) = &collected.file_info["docs"] else {
            panic!("docs should be a directory");
        };
        assert!(matches!(docs["one.txt"], TreeNode::File { .. }));
        let TreeNode::Dir(renamed) = &collected.file_info["docs (2)"] else {
            panic!("docs (2) should be a directory");
        };
        assert!(matches!(renamed["two.txt"], TreeNode::File { .. }));
    }

    #[test]
    fn describe_roots_distinguishes_multiple_roots_from_files() {
        let mut roots = FileInfo::new();
        roots.insert("docs".to_string(), TreeNode::Dir(IndexMap::new()));
        roots.insert(
            "readme.md".to_string(),
            TreeNode::File { f: "a".repeat(64), s: 0 },
        );
        roots.insert("assets".to_string(), TreeNode::Dir(IndexMap::new()));

        assert_eq!(describe_roots(&roots), "docs 等 3 项");
    }

    #[test]
    fn rebuild_tree_restores_structure_and_shares_repeated_content() {
        let directory = TemporaryDirectory::new("rebuild");
        let source = directory.0.join("payload.bin");
        fs::write(&source, "shared").expect("write payload");
        let file_id = hash_bytes(b"shared");
        let mut inner = IndexMap::new();
        inner.insert("first.bin".to_string(), TreeNode::File { f: file_id.clone(), s: 6 });
        let mut sub = IndexMap::new();
        sub.insert("second.bin".to_string(), TreeNode::File { f: file_id.clone(), s: 6 });
        inner.insert("sub".to_string(), TreeNode::Dir(sub));
        inner.insert("empty".to_string(), TreeNode::Dir(IndexMap::new()));
        let mut bundle = FileInfo::new();
        bundle.insert("bundle".to_string(), TreeNode::Dir(inner));

        let destination = directory.0.join("view");
        let written = rebuild_tree(&destination, &bundle, &|_| Some(source.clone()), false)
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
        fn set_hashed(dir: &FileInfo) -> FileInfo {
            dir.iter()
                .map(|(name, node)| {
                    let node = match node {
                        TreeNode::File { s, .. } => TreeNode::File { f: "a".repeat(64), s: *s },
                        TreeNode::Dir(children) => TreeNode::Dir(set_hashed(children)),
                    };
                    (name.clone(), node)
                })
                .collect()
        }
        fn count_dirs(dir: &FileInfo) -> u64 {
            dir.values()
                .map(|node| match node {
                    TreeNode::File { .. } => 0,
                    TreeNode::Dir(children) => 1 + count_dirs(children),
                })
                .sum()
        }
        let hashed = set_hashed(&collected.file_info);
        let bytes = serde_json::to_string(&hashed).expect("serialize tree").len();

        fn count_files(dir: &FileInfo) -> u64 {
            dir.values()
                .map(|node| match node {
                    TreeNode::File { .. } => 1,
                    TreeNode::Dir(children) => count_files(children),
                })
                .sum()
        }
        assert_eq!(count_files(&collected.file_info), 3000);
        assert_eq!(count_dirs(&collected.file_info), 61);
        assert!(bytes < MAX_MESSAGE_BYTES / 8, "3000 files encoded into {bytes} bytes");
    }

    #[test]
    fn refresh_summary_separates_ready_pending_and_uploadable_contents() {
        let uploaded = hash_bytes(b"uploaded");
        let local = hash_bytes(b"local");
        let remote = hash_bytes(b"remote");
        let mut inner = IndexMap::new();
        inner.insert("a".to_string(), TreeNode::File { f: uploaded.clone(), s: 100 });
        inner.insert("b".to_string(), TreeNode::File { f: local.clone(), s: 300 });
        inner.insert("c".to_string(), TreeNode::File { f: remote.clone(), s: 500 });
        let mut bundle = FileInfo::new();
        bundle.insert("bundle".to_string(), TreeNode::Dir(inner));
        let mut entry = ClipboardEntry {
            id: "entry".to_string(),
            kind: "files".to_string(),
            content: String::new(),
            html: None,
            rtf: None,
            file_info: Some(bundle),
            image_info: None,
            source_device_id: "device".to_string(),
            created_at: "now".to_string(),
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

        refresh_summary(
            &mut entry,
            &HashSet::new(),
            &HashSet::from([uploaded]),
            Path::new("cache"),
        );

        let summary = &entry.summary;
        assert_eq!(summary.root_kind, "dir");
        assert_eq!((summary.file_count, summary.hashed_count), (3, 3));
        assert_eq!(summary.content_count, 3);
        assert_eq!((summary.total_size, summary.max_file_size), (900, 500));
        assert_eq!(summary.uploaded_count, 1);
        assert_eq!(summary.ready_count, 2);
        assert_eq!((summary.pending_count, summary.pending_size), (1, 500));
        // Only the locally available but unpublished content can still be uploaded.
        assert_eq!(summary.uploadable_size, Some(300));
        assert!(summary.preview_path.is_none());
    }
}
