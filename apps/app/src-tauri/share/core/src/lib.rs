//! Portable share-upload state machine. Swift/Android only execute the returned
//! file-backed HTTP requests with their OS background transfer APIs. Credentials
//! are passed per call and never persisted in a plan or request body.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::{c_char, CStr, CString},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const CHUNK_SIZE: u64 = 128 * 1024;
#[cfg(target_os = "ios")]
// Also compiled into the standalone static library used by the share extension.
#[path = "../../ios/mod.rs"]
pub mod ios;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    http_url: String,
    token: String,
    device_id: String,
    scope: String,
    file_limit: u64,
    max_count: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    path: PathBuf,
    name: String,
}
#[derive(Deserialize)]
struct Share {
    text: Option<String>,
    html: Option<String>,
    items: Vec<Item>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Blob {
    path: PathBuf,
    file_id: String,
    size: u64,
    kind: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    scope: String,
    entries: Vec<Value>,
    blobs: Vec<Blob>,
    blob_index: usize,
    entry_index: usize,
    phase: String,
    missing: Vec<usize>,
    restarts: u8,
    status: String,
    error: Option<String>,
}

type Result<T> = std::result::Result<T, String>;
fn io(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn atomic(path: &Path, data: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    let mut file = fs::File::create(&temp).map_err(io)?;
    file.write_all(data).map_err(io)?;
    file.sync_all().map_err(io)?;
    // rename replaces atomically on the two mobile targets (POSIX).
    fs::rename(&temp, path).map_err(io)
}
fn save(directory: &Path, plan: &Plan) -> Result<()> {
    atomic(
        &directory.join("upload.json"),
        &serde_json::to_vec(plan).map_err(io)?,
    )
}
fn read(directory: &Path) -> Result<Plan> {
    serde_json::from_slice(&fs::read(directory.join("upload.json")).map_err(io)?).map_err(io)
}
fn hash(path: &Path) -> Result<(String, u64)> {
    let mut file = fs::File::open(path).map_err(io)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0;
    loop {
        let count = file.read(&mut buffer).map_err(io)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((format!("{:x}", hash.finalize()), size))
}
fn prepare(directory: &Path, share: Share, auth: &Session) -> Result<Value> {
    if directory.join("upload.json").exists() {
        return serde_json::to_value(read(directory)?).map_err(io);
    }
    if share.items.len() > auth.max_count {
        return Err("分享文件数量超过上限，请打开 ClipRoam 处理".into());
    }
    let mut entries = Vec::new();
    let mut blobs = Vec::new();
    if let Some(text) = share.text.filter(|text| !text.trim().is_empty()) {
        let mut entry = json!({"kind":"text", "content":text, "sourceDeviceId":auth.device_id});
        if let Some(html) = share.html {
            entry["html"] = html.into();
        }
        entries.push(entry);
    }
    let mut tree = serde_json::Map::new();
    let mut names = Vec::new();
    let root = directory.canonicalize().map_err(io)?;
    for (index, item) in share.items.into_iter().enumerate() {
        let source = item.path.canonicalize().map_err(io)?;
        if !source.starts_with(&root) {
            return Err("分享文件必须位于分享缓存目录中".into());
        }
        let path = source;
        let (file_id, size) = hash(&path)?;
        let limit = auth.file_limit;
        if size >= limit {
            return Err("分享文件超过自动上传大小限制，请打开 ClipRoam 处理".into());
        }
        blobs.push(Blob {
            path,
            file_id: file_id.clone(),
            size,
            kind: "files".into(),
        });
        let mut units = 0;
        let base: String = item
            .name
            .chars()
            .map(|c| {
                if c == '/' || c == '\\' || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .take_while(|c| {
                units += c.len_utf16();
                units <= 220
            })
            .collect();
        let base = if base.is_empty() {
            format!("shared-{index}")
        } else {
            base
        };
        let mut name = base.clone();
        let mut suffix = 1;
        while tree.contains_key(&name) {
            name = format!("{suffix}-{base}");
            suffix += 1;
        }
        names.push(name.clone());
        tree.insert(name, json!({"f":file_id,"s":size}));
    }
    if !tree.is_empty() {
        entries.push(json!({"kind":"files", "content":names.join("、"), "fileInfo":tree, "sourceDeviceId":auth.device_id}));
    }
    if entries.is_empty() {
        return Err("没有可上传的分享内容".into());
    }
    let plan = Plan {
        scope: auth.scope.clone(),
        entries,
        blobs,
        blob_index: 0,
        entry_index: 0,
        phase: "begin".into(),
        missing: Vec::new(),
        restarts: 0,
        status: "active".into(),
        error: None,
    };
    save(directory, &plan)?;
    serde_json::to_value(plan).map_err(io)
}

fn next(directory: &Path, auth: &Session) -> Result<Value> {
    let mut plan = read(directory)?;
    if plan.scope != auth.scope {
        return Err("分享上传账号不匹配".into());
    }
    let base = url::Url::parse(&auth.http_url).map_err(io)?;
    if !["http", "https"].contains(&base.scheme())
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || auth.token.is_empty()
    {
        return Err("分享上传登录状态无效".into());
    }
    let body_path = directory.join("request.body");
    let (path, method, mime, body) = if let Some(blob) = plan.blobs.get(plan.blob_index) {
        if plan.phase == "chunk" && !plan.missing.is_empty() {
            let index = plan.missing[0];
            let offset = index as u64 * CHUNK_SIZE;
            if offset >= blob.size {
                return Err("上传分片位置无效".into());
            }
            let mut source = fs::File::open(&blob.path).map_err(io)?;
            source.seek(SeekFrom::Start(offset)).map_err(io)?;
            let mut bytes = vec![0; (blob.size - offset).min(CHUNK_SIZE) as usize];
            source.read_exact(&mut bytes).map_err(io)?;
            (
                format!("/upload/{}?index={index}", blob.file_id),
                "PUT",
                "application/octet-stream",
                bytes,
            )
        } else {
            plan.phase = "begin".into();
            (
                "/upload/begin".into(),
                "POST",
                "application/json",
                serde_json::to_vec(
                    &json!({"fileId":blob.file_id,"size":blob.size,"kind":blob.kind}),
                )
                .map_err(io)?,
            )
        }
    } else if let Some(entry) = plan.entries.get(plan.entry_index) {
        plan.phase = "publish".into();
        (
            "/entries".into(),
            "POST",
            "application/json",
            serde_json::to_vec(&json!({"entry":entry})).map_err(io)?,
        )
    } else {
        plan.status = "complete".into();
        plan.error = None;
        save(directory, &plan)?;
        return Ok(json!({"complete":true}));
    };
    atomic(&body_path, &body)?;
    plan.status = "active".into();
    plan.error = None;
    save(directory, &plan)?;
    Ok(
        json!({"complete":false,"url":format!("{}{}", auth.http_url.trim_end_matches('/'), path),
        "method":method,"contentType":mime,"bodyPath":body_path}),
    )
}

fn advance(directory: &Path, status: u64, body: &str) -> Result<Value> {
    let mut plan = read(directory)?;
    if status == 404 && plan.phase == "chunk" {
        if plan.restarts >= 3 {
            return Err("服务器上传进度反复失效".into());
        }
        plan.restarts += 1;
        plan.phase = "begin".into();
        save(directory, &plan)?;
        return Ok(json!({}));
    }
    if !(200..300).contains(&status) {
        return Err(if status == 401 {
            "登录已失效，请打开 ClipRoam 重新登录".into()
        } else {
            format!("分享上传失败（HTTP {status}），请打开 ClipRoam 重试")
        });
    }
    let response: Value = serde_json::from_str(body).map_err(io)?;
    if plan.phase == "publish" {
        if response["entry"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .is_none()
        {
            return Err("服务器发布响应无效".into());
        }
        plan.entry_index += 1;
    } else {
        let blob = plan.blobs.get(plan.blob_index).ok_or("上传任务位置无效")?;
        if response["status"] == "stored" {
            if response["fileId"] != blob.file_id {
                return Err("服务器返回的文件标识不一致".into());
            }
            plan.blob_index += 1;
            plan.phase = "begin".into();
            plan.restarts = 0;
        } else {
            let expected = if plan.phase == "begin" {
                "ready"
            } else {
                "accepted"
            };
            if response["status"] != expected {
                return Err("服务器上传响应无效".into());
            }
            let received = response["receivedBytes"]
                .as_u64()
                .ok_or("服务器上传进度无效")?;
            if received > blob.size {
                return Err("服务器上传进度超出文件大小".into());
            }
            let bitmap = STANDARD
                .decode(
                    response["missingChunks"]
                        .as_str()
                        .ok_or("服务器上传位图缺失")?,
                )
                .map_err(io)?;
            let count = blob.size.div_ceil(CHUNK_SIZE) as usize;
            if bitmap.len() < count.div_ceil(8) {
                return Err("服务器上传位图无效".into());
            }
            let missing: Vec<_> = (0..count)
                .filter(|i| bitmap[i / 8] & (1 << (i % 8)) != 0)
                .collect();
            if plan.phase == "chunk" && missing.len() >= plan.missing.len() {
                return Err("服务器上传没有进展".into());
            }
            if missing.is_empty() {
                plan.blob_index += 1;
                plan.phase = "begin".into();
            } else {
                plan.missing = missing;
                plan.phase = "chunk".into();
            }
        }
    }
    save(directory, &plan)?;
    Ok(json!({}))
}

fn dispatch(input: Value) -> Result<Value> {
    #[cfg(target_os = "ios")]
    if input["command"]
        .as_str()
        .is_some_and(|c| c.starts_with("ios."))
    {
        return ios::dispatch(input);
    }
    let directory = Path::new(input["directory"].as_str().ok_or("缺少分享缓存目录")?);
    match input["command"].as_str().ok_or("缺少操作")? {
        "prepare" => prepare(
            directory,
            serde_json::from_value(input["share"].clone()).map_err(io)?,
            &serde_json::from_value(input["session"].clone()).map_err(io)?,
        ),
        "next" => next(
            directory,
            &serde_json::from_value(input["session"].clone()).map_err(io)?,
        ),
        "advance" => advance(
            directory,
            input["status"].as_u64().ok_or("缺少 HTTP 状态")?,
            input["body"].as_str().ok_or("缺少 HTTP 响应")?,
        ),
        "fail" => {
            let mut plan = read(directory)?;
            plan.status = "failed".into();
            plan.error = Some(input["error"].as_str().unwrap_or("分享上传失败").into());
            save(directory, &plan)?;
            Ok(json!({}))
        }
        _ => Err("未知分享上传操作".into()),
    }
}

pub fn call_json(input: &str) -> String {
    let result =
        std::panic::catch_unwind(|| serde_json::from_str(input).map_err(io).and_then(dispatch));
    match result {
        Ok(Ok(value)) => json!({"ok":true,"value":value}).to_string(),
        Ok(Err(error)) => json!({"ok":false,"error":error}).to_string(),
        Err(_) => json!({"ok":false,"error":"分享上传核心异常，内容已保留"}).to_string(),
    }
}

/// # Safety
/// `input` must point to a valid, NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn cliproam_share_call(input: *const c_char) -> *mut c_char {
    let result = if input.is_null() {
        "{\"ok\":false,\"error\":\"missing input\"}".into()
    } else {
        call_json(CStr::from_ptr(input).to_str().unwrap_or(""))
    };
    CString::new(result).unwrap().into_raw()
}
/// # Safety
/// `output` must be an unreleased pointer returned by `cliproam_share_call`.
#[no_mangle]
pub unsafe extern "C" fn cliproam_share_free(output: *mut c_char) {
    if !output.is_null() {
        drop(CString::from_raw(output));
    }
}
