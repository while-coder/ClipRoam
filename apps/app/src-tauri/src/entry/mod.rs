//! 条目（entries）的处理逻辑：读取、更新与删除。
//!
//! - `query.rs`：manifest / query / ids 等读取命令，行离开后端前重算
//!   派生 summary；
//! - `mutate.rs`：服务器来源的条目写入（upsert、发布换 id）与删除；
//! - 本文件放两种条目视图共用的整形与内容枚举辅助。

pub(crate) mod mutate;
pub(crate) mod query;

use crate::content::{describe_roots, tree_contents, ClipboardEntry, LocalSources};

/// Every content an entry references, whichever kind carries it.
pub(crate) fn entry_contents_of(entry: &ClipboardEntry) -> Vec<(String, u64)> {
    match (&entry.file_info, &entry.image_info) {
        (Some(file_info), _) => tree_contents(file_info),
        (None, Some(image)) => vec![(image.file_id.clone(), image.size)],
        (None, None) => Vec::new(),
    }
}

/// The frontend renders lists of hundreds of entries; shipping their trees
/// would mean tens of thousands of nodes per refresh.
pub(crate) fn lightweight_entry(entry: &ClipboardEntry) -> ClipboardEntry {
    // html/rtf can be hundreds of kilobytes per rich-text entry and the list
    // never renders them, so they stay behind `get_entry`. Built field by
    // field: a struct-update clone would copy those strings just to drop them.
    let mut lightweight = ClipboardEntry {
        id: entry.id.clone(),
        kind: entry.kind.clone(),
        content: entry.content.clone(),
        html: None,
        rtf: None,
        file_info: None,
        image_info: None,
        source_device_id: entry.source_device_id.clone(),
        created_at: entry.created_at.clone(),
        summary: entry.summary.clone(),
        sources: LocalSources::default(),
    };
    if lightweight.kind == "files" {
        if let Some(file_info) = &entry.file_info {
            lightweight.content = describe_roots(file_info);
        }
    }
    lightweight
}
