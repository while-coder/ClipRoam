//! 文件内容（content-addressed blobs）的本地存储与查询。
//!
//! 条目行只记录内容 id（sha256），字节的落盘与查找都在这里：
//! - **path**：内容 id 的形状校验与落盘路径（`upload/images`、`download`）；
//! - **cache**：`hash_cache` 表（源文件 → 内容 id 的缓存与反查）、blob 目录
//!   扫描、以及不再被任何条目引用的本地垃圾回收；
//! - **query**：历史引用的内容 id 与候选上传条目两个查询命令。服务器池的
//!   可用性由前端经 `/files/query` 实时查询，本地不持久化。

mod cache;
mod path;
pub(crate) mod query;

pub use cache::{
    cached_hash, cached_source_for, collect_local_garbage, remember_hash, scan_cached_blobs,
};
pub use path::{cached_file_path, download_path, is_file_id, upload_image_path};
