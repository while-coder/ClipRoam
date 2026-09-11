//! 跨模块复用的小工具：哈希、时间与 SQL 辅助。

use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::{Path, PathBuf}, time::UNIX_EPOCH};

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

// ---------------------------------------------------------------------------
// 文件写入
// ---------------------------------------------------------------------------

/// 原子写入：先写临时文件再改名，写一半崩溃/断电不会留下截断的文件（目标
/// 多为内容寻址路径，截断文件会被后续同样内容的写入直接信任）。改名失败时
/// 清理临时文件并返回错误。
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);
    fs::write(&tmp_path, bytes).map_err(|error| error.to_string())?;
    fs::rename(&tmp_path, path).map_err(|error| {
        let _ = fs::remove_file(&tmp_path);
        error.to_string()
    })
}

// ---------------------------------------------------------------------------
// 名称过滤
// ---------------------------------------------------------------------------

/// 名称是否命中任一过滤模式：大小写不敏感，`*` 匹配任意长度、`?` 匹配单个
/// 字符（`node_modules`、`*.log`、`build*`）。只匹配名称本身，不涉及路径。
pub fn name_matches_any(name: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let name = name.to_lowercase();
    patterns
        .iter()
        .any(|pattern| glob_match(&name, &pattern.trim().to_lowercase()))
}

fn glob_match(text: &str, pattern: &str) -> bool {
    match (text.chars().next(), pattern.chars().next()) {
        // 文本耗尽后仍可能被剩余的 `*` 吸收（如 "build*" 匹配 "build"）。
        (None, Some('*')) => glob_match(text, &pattern[1..]),
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => false,
        (Some(text_char), Some('*')) => {
            glob_match(text, &pattern[1..]) || glob_match(&text[text_char.len_utf8()..], pattern)
        }
        (Some(text_char), Some('?')) => glob_match(&text[text_char.len_utf8()..], &pattern[1..]),
        (Some(text_char), Some(pattern_char)) => {
            text_char == pattern_char
                && glob_match(&text[text_char.len_utf8()..], &pattern[pattern_char.len_utf8()..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::name_matches_any;

    #[test]
    fn matches_plain_names_and_wildcards() {
        let patterns = vec!["node_modules".to_string(), "*.log".to_string(), "build*".to_string()];
        assert!(name_matches_any("node_modules", &patterns));
        assert!(name_matches_any("NODE_MODULES", &patterns));
        assert!(name_matches_any("error.log", &patterns));
        assert!(name_matches_any("build-output", &patterns));
        assert!(name_matches_any("build", &patterns));
        assert!(!name_matches_any("my_node_modules_backup", &patterns));
        assert!(!name_matches_any("logs", &patterns));
        assert!(!name_matches_any("readme.md", &[]));
    }
}
