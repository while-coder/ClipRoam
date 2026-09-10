//! 文件内容（content-addressed blobs）的本地存储与查询。
//!
//! 条目行只记录内容 id（sha256），字节的落盘与查找都在这里：
//! - **cache**：`hash_cache` 表（源文件 → 内容 id 的缓存与反查）、blob 目录
//!   扫描（按需现扫，磁盘即真相）、以及不再被任何条目引用的本地垃圾回收；
//! - **store**：`files` 表——服务器池可用性（`stored`）与内容大小的持久层，
//!   由 `/files/query` 回填（`upsert_server_files`）唯一写入；
//! - **query**：历史引用的内容 id 与候选上传条目两个查询命令。
//!
//! 路径函数也在这：内容 id 决定字节落在磁盘哪里，所以在变成路径之前先
//! 校验形状。

pub(crate) mod cache;
pub(crate) mod query;
pub(crate) mod store;

pub use cache::{blob_ids_on_disk, cached_file_hash, cached_source_for, collect_local_garbage, remember_file_hash};
pub(crate) use store::history_stored_ids;

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

/// Cache downloads land at `<final>.part` and are renamed into place only
/// after digest verification, so a crash can never leave a truncated file
/// that the startup blob scan would accept as valid content.
pub fn partial_download_path(final_path: &Path) -> PathBuf {
    final_path.with_extension("part")
}

pub fn cached_file_path(cache_dir: &Path, file_id: &str) -> Option<PathBuf> {
    [upload_image_path(cache_dir, file_id), download_path(cache_dir, file_id)]
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
}
