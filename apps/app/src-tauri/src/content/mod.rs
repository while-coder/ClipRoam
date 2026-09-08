//! Content-addressed clipboard storage.
//!
//! A `files` entry carries one nested map (`FileInfo`): file leaves are
//! identified by `sha256(bytes)` plus their size, directories are nested maps,
//! and an image entry references a single blob. Where the bytes actually live
//! — the machine that copied them, the local blob cache, or the server pool —
//! is answered separately, so the same content is never stored twice.

mod hash;
mod paths;
mod tree;
#[cfg(test)]
pub(crate) mod test_support;

pub use hash::{hash_bytes, hash_file, is_file_id, to_hex};
pub use paths::{download_path, modified_millis, upload_image_path};
pub use tree::{
    collect_tree, describe_roots, file_entry_signature, file_signature, local_source_was_lost,
    preserve_local_sources, readable_path, rebuild_tree, refresh_summary, tree_contents,
    tree_parent_at_path,
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

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
    pub source_device_id: String,
    pub created_at: String,
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
