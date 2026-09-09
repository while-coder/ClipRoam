//! 跨模块复用的小工具：哈希、时间与 SQL 辅助。

use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path, time::UNIX_EPOCH};

// ---------------------------------------------------------------------------
// 哈希
// ---------------------------------------------------------------------------

pub const HASH_READ_BUFFER: usize = 512 * 1024;

/// FNV-1a: enough for non-cryptographic local identities (clipboard
/// signatures, history keys) where only repeat detection matters.
pub fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes
        .into_iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
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

// ---------------------------------------------------------------------------
// 时间与 SQL
// ---------------------------------------------------------------------------

/// 文件修改时间的毫秒时间戳；读不到时返回 `None`。
pub fn modified_millis(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis() as u64)
}

/// Comma-separated `?` marks for an IN clause.
pub fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count).collect::<Vec<_>>().join(", ")
}

/// Escapes LIKE wildcards so a keyword or a path matches literally. Callers
/// pairing it with a `LIKE ?` must add `ESCAPE '\'`.
pub fn escape_like(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len());
    for character in needle.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}
