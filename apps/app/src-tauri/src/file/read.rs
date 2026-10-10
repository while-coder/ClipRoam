//! Shared local content reads for pool uploads and relay uploads.
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::{fs, io::{Read, Seek, SeekFrom}};
use tauri::State;
use crate::file::{cached_file_path, cached_source_for};
use crate::AppState;
const FILE_CHUNK_LIMIT: usize = 128 * 1024;
/// Reads one chunk of a content by content id alone — the upload HTTP is
/// content-addressed and never involves an entry. The path comes from the
/// local blob cache first, then from the hash cache's reverse lookup of the
/// original source file (a file hashed here before can stand in for content
/// that never landed as a blob).
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn read_upload_chunk(
    state: State<'_, AppState>,
    file_id: String,
    offset: u64,
    length: usize,
    session_id: String,
) -> Result<String, String> {
    let account = state.account(&session_id)?;
    let path = {
        let cache_dir = account.cache_dir.clone();
        // `cached_file_path` stats the candidates itself, so a hit is always
        // a file that exists right now.
        cached_file_path(&cache_dir, &file_id).or_else(|| {
            account.with_database(|connection| {
                    Ok(cached_source_for(connection, &file_id))
                })
                .ok()
                .flatten()
        })
    }
    .ok_or_else(|| "本机文件内容不可用".to_string())?;
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = vec![0; length.min(FILE_CHUNK_LIMIT)];
    let count = file.read(&mut bytes).map_err(|error| error.to_string())?;
    bytes.truncate(count);
    Ok(BASE64.encode(bytes))
}

