//! 下载域：全局下载管理器（Downloader）+ 它的磁盘侧原语 + 为前端提供条目
//! 内容的命令。
//!
//! Downloader 拥有任务表 / FIFO 队列 / 跨条目并发上限 / 按 `(saveId, fileId)`
//! 在途去重 / reqwest 直连拉流写盘。main 与 paste 两个 webview 的下载都进
//! 这同一个实例，跨窗口同文件天然合并——旧前端方案「两个窗口各写各的
//! `.part` 互踩」的问题在此根除。拉流重试策略对齐旧前端管线：断流/401 等
//! 3 秒退避后整个传输从零重启（GET 无 offset），总超时 5 分钟。
//!
//! 锁纪律：Downloader 的 `inner` 锁内只做纯内存操作；事件 emit、
//! `virtual_downloads` 调用、其他 Mutex 一律 clone 所需数据 → drop inner →
//! 再做，避免嵌套锁。

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::StreamExt;
use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

use crate::clipboard::output::{missing_files, snapshot_entry, FilePasteStrategy};
use crate::content::MissingFile;
use crate::entry::entry_contents_of;
use crate::file::{cached_file_path, cached_source_for, download_path, partial_download_path};
use crate::store::select_entry;
use crate::sync::server_http_url;
use crate::AppState;

#[derive(Default)]
pub(crate) struct VirtualDownloadStatus {
    requested: bool,
    pub(crate) complete: bool,
    pub(crate) error: Option<String>,
}

#[derive(Default)]
pub(crate) struct VirtualDownloads {
    pub(crate) transfers: Mutex<std::collections::HashMap<String, VirtualDownloadStatus>>,
    pub(crate) changed: std::sync::Condvar,
}

impl VirtualDownloads {
    pub(crate) fn request(&self, file_id: &str) -> bool {
        let Ok(mut transfers) = self.transfers.lock() else { return false };
        let status = transfers.entry(file_id.to_string()).or_default();
        if status.complete {
            return false;
        }
        if status.error.take().is_some() {
            status.requested = false;
        }
        if status.requested {
            false
        } else {
            status.requested = true;
            true
        }
    }

    pub(crate) fn begin(&self, file_id: &str) {
        if let Ok(mut transfers) = self.transfers.lock() {
            transfers.insert(file_id.to_string(), VirtualDownloadStatus {
                requested: true,
                complete: false,
                error: None,
            });
            self.changed.notify_all();
        }
    }

    pub(crate) fn progress(&self) {
        self.changed.notify_all();
    }

    pub(crate) fn complete(&self, file_id: &str) {
        if let Ok(mut transfers) = self.transfers.lock() {
            let status = transfers.entry(file_id.to_string()).or_default();
            status.complete = true;
            status.error = None;
            self.changed.notify_all();
        }
    }

    pub(crate) fn fail(&self, file_id: &str, error: String) {
        if let Ok(mut transfers) = self.transfers.lock() {
            let status = transfers.entry(file_id.to_string()).or_default();
            status.complete = false;
            status.error = Some(error);
            self.changed.notify_all();
        }
    }
}

pub(crate) struct DownloadState {
    pub(crate) path: std::path::PathBuf,
    pub(crate) file_id: String,
    pub(crate) expected_size: u64,
    pub(crate) received_size: u64,
    pub(crate) hasher: Sha256,
    pub(crate) target: DownloadTarget,
}

pub(crate) enum DownloadTarget {
    Cache {
        /// The content-addressed name the verified `.part` is renamed to.
        final_path: std::path::PathBuf,
    },
    Save {
        save_id: String,
        completed_path: std::path::PathBuf,
    },
}

/// 开启一个下载传输：确定落盘目标（内容缓存或另存 staging）并创建 `.part`。
/// 传输状态记录在 `state.downloads`，由 `append_chunk`/`finish_transfer`/
/// `cancel_transfer` 接力。
pub(crate) fn begin_transfer(
    state: &AppState,
    transfer_id: &str,
    file_id: &str,
    expected_size: u64,
    save_id: Option<&str>,
) -> Result<(), String> {
    let (path, target) = if let Some(save_id) = save_id {
        let mut sessions = state.save_sessions.lock().map_err(|error| error.to_string())?;
        let session = sessions
            .get_mut(save_id)
            .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
        let expected = session
            .expected
            .get(file_id)
            .ok_or_else(|| "文件不属于当前另存为任务".to_string())?;
        if *expected != expected_size {
            return Err("文件大小与另存为任务不一致".to_string());
        }
        if session.downloaded.contains(file_id) || !session.in_progress.insert(file_id.to_string()) {
            return Err("文件正在下载或已经下载".to_string());
        }
        let completed_path = session.staging_dir.join(file_id);
        (
            session.staging_dir.join(format!("{file_id}.part")),
            DownloadTarget::Save {
                save_id: save_id.to_string(),
                completed_path,
            },
        )
    } else {
        state.virtual_downloads.begin(file_id);
        let final_path = {
            let history = state.history.lock().map_err(|error| error.to_string())?;
            let cache_dir = state.active_cache_dir(&history)?;
            download_path(&cache_dir, file_id).ok_or_else(|| "内容标识不合法".to_string())?
        };
        // Cache downloads stage at `.part` exactly like direct saves; the
        // digest-verified rename in `finish_transfer` is the promotion.
        let path = partial_download_path(&final_path);
        (path, DownloadTarget::Cache { final_path })
    };
    let prepared = (|| {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::File::create(&path).map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(error) = prepared {
        clear_download_target(state, &target, file_id, &error);
        return Err(error);
    }
    state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .insert(
            transfer_id.to_string(),
            DownloadState {
                path,
                file_id: file_id.to_string(),
                expected_size,
                received_size: 0,
                hasher: Sha256::new(),
                target,
            },
        );
    Ok(())
}

/// 追加一段已收字节：累计大小与哈希、写 `.part`；Cache 目标顺带唤醒等待
/// 前缀的虚拟文件读循环。
pub(crate) fn append_chunk(state: &AppState, transfer_id: &str, bytes: &[u8]) -> Result<(), String> {
    let mut downloads = state.downloads.lock().map_err(|error| error.to_string())?;
    let download = downloads
        .get_mut(transfer_id)
        .ok_or_else(|| "文件下载任务不存在".to_string())?;
    download.received_size += bytes.len() as u64;
    if download.received_size > download.expected_size {
        return Err("下载内容超过声明大小".to_string());
    }
    download.hasher.update(bytes);
    fs::OpenOptions::new()
        .append(true)
        .open(&download.path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|error| error.to_string())?;
    if matches!(download.target, DownloadTarget::Cache { .. }) {
        state.virtual_downloads.progress();
    }
    Ok(())
}

/// 完成一个传输：size + Sha256 校验通过后才把 `.part` 晋级（进缓存或另存
/// staging 的最终文件），并更新另存会话簿记。
pub(crate) fn finish_transfer(state: &AppState, transfer_id: &str) -> Result<(), String> {
    let download = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(transfer_id)
        .ok_or_else(|| "文件下载任务不存在".to_string())?;
    if download.received_size != download.expected_size {
        let _ = fs::remove_file(&download.path);
        fail_download_target(state, &download, "文件下载不完整");
        return Err("文件下载不完整".to_string());
    }
    if crate::utils::to_hex(&download.hasher.clone().finalize()) != download.file_id {
        let _ = fs::remove_file(&download.path);
        fail_download_target(state, &download, "文件内容校验失败");
        return Err("文件内容校验失败".to_string());
    }

    match &download.target {
        DownloadTarget::Cache { final_path } => {
            // Promote only now, after the size and digest checks above: a
            // truncated download must never enter the cache, where the blob
            // scan accepts any file whose name is a content id.
            fs::rename(&download.path, final_path).map_err(|error| error.to_string())?;
            state.virtual_downloads.complete(&download.file_id);
        }
        DownloadTarget::Save {
            save_id,
            completed_path,
        } => {
            if completed_path.exists() {
                fs::remove_file(completed_path).map_err(|error| error.to_string())?;
            }
            fs::rename(&download.path, completed_path).map_err(|error| error.to_string())?;
            let mut sessions = state.save_sessions.lock().map_err(|error| error.to_string())?;
            let session = sessions
                .get_mut(save_id)
                .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
            session.in_progress.remove(&download.file_id);
            session.downloaded.insert(download.file_id.clone());
        }
    }
    Ok(())
}

/// 取消/失败一个传输：删 `.part` 并回滚落盘目标（Save 会话移出 in_progress、
/// Cache 置虚拟文件错误态唤醒等待者）。
pub(crate) fn cancel_transfer(state: &AppState, transfer_id: &str, reason: &str) {
    let download = state.downloads.lock().ok().and_then(|mut downloads| downloads.remove(transfer_id));
    if let Some(download) = download {
        let _ = fs::remove_file(&download.path);
        fail_download_target(state, &download, reason);
    }
}

pub(crate) fn fail_download_target(state: &AppState, download: &DownloadState, message: &str) {
    clear_download_target(state, &download.target, &download.file_id, message);
}

pub(crate) fn clear_download_target(state: &AppState, target: &DownloadTarget, file_id: &str, message: &str) {
    match target {
        DownloadTarget::Cache { final_path } => {
            // Drop the staging file a cancelled or failed transfer leaves behind.
            let _ = fs::remove_file(partial_download_path(final_path));
            state.virtual_downloads.fail(file_id, message.to_string());
        }
        DownloadTarget::Save { save_id, .. } => {
            if let Ok(mut sessions) = state.save_sessions.lock() {
                if let Some(session) = sessions.get_mut(save_id) {
                    session.in_progress.remove(file_id);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 为前端提供条目内容：上传用分块读取，以及驱动下载的可用性快照。
// ---------------------------------------------------------------------------

/// Largest chunk served per `read_upload_chunk` call.
const FILE_CHUNK_LIMIT: usize = 128 * 1024;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EntryFileCandidate {
    file_id: String,
    size: u64,
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn list_entry_files(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<Vec<EntryFileCandidate>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let history_path = state.active_history_path(&history)?;
    let entry = state
        .with_database(&history_path, |connection| select_entry(connection, &entry_id))?
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    Ok(entry_contents_of(&entry)
        .into_iter()
        .map(|(file_id, size)| EntryFileCandidate { file_id, size })
        .collect())
}

/// Contents this machine cannot read yet, de-duplicated — the frontend turns
/// each one into a download. With `paste_only` only the contents that must
/// exist before this platform can start a paste are returned, so the frontend
/// does not need to know which operating system it runs on.
fn prepare_entry(state: &AppState, entry_id: &str, paste_only: bool) -> Result<Vec<MissingFile>, String> {
    let snapshot = snapshot_entry(state, entry_id)?;
    if paste_only
        && !FilePasteStrategy::for_entry(&snapshot.entry).requires_complete_content(&snapshot.entry.kind)
    {
        return Ok(Vec::new());
    }
    Ok(missing_files(&snapshot))
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn prepare_entry_files(state: State<'_, AppState>, entry_id: String) -> Result<Vec<MissingFile>, String> {
    prepare_entry(&state, &entry_id, false)
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn prepare_paste_entry(state: State<'_, AppState>, entry_id: String) -> Result<Vec<MissingFile>, String> {
    prepare_entry(&state, &entry_id, true)
}

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
) -> Result<String, String> {
    let path = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = state.active_cache_dir(&history)?;
        let history_path = state.active_history_path(&history)?;
        // `cached_file_path` stats the candidates itself, so a hit is always
        // a file that exists right now.
        cached_file_path(&cache_dir, &file_id).or_else(|| {
            state
                .with_database(&history_path, |connection| {
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

// ---------------------------------------------------------------------------
// Downloader：全局下载管理器（队列 / 并发 / 去重 / 拉流 worker / 命令）
// ---------------------------------------------------------------------------
/// 同一时刻最多在下载的文件数（跨条目全局）。
const MAX_CONCURRENT_DOWNLOADS: usize = 4;
/// 字节进度事件的节流间隔；状态转换不受限，立即推送。
const PROGRESS_THROTTLE: Duration = Duration::from_millis(200);
/// 终态任务在列表里保留多久供面板展示结果。
const TERMINAL_RETENTION: Duration = Duration::from_secs(60);
/// 断流后的固定退避；期间可被取消打断。
const RETRY_DELAY: Duration = Duration::from_secs(3);
/// 单文件下载总预算：超时即认为没有设备能提供该内容。
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(5 * 60);

pub(crate) const DOWNLOAD_CHANGED_EVENT: &str = "cliproam://download-changed";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadRequest {
    file_id: String,
    size: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BatchOutcome {
    cancelled: bool,
    failed_count: usize,
    total: usize,
}

#[derive(Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DownloadStatus {
    Queued,
    Downloading,
    Succeeded,
    Failed,
    Cancelled,
}

/// 推给前端的任务快照，字段与 TS 的 `DownloadTaskSnapshot` 一一对应。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskSnapshot {
    id: String,
    batch_id: u64,
    entry_id: String,
    file_id: String,
    save_id: Option<String>,
    label: String,
    size: u64,
    status: DownloadStatus,
    received_bytes: u64,
    error: Option<String>,
}

/// 单个任务的终态；经 watch 单播给所有等待方（含去重合并方）。
#[derive(Clone)]
pub(crate) enum TaskOutcome {
    Succeeded,
    Failed(String),
    Cancelled(String),
}

struct DownloadTask {
    id: String,
    batch_id: u64,
    entry_id: String,
    file_id: String,
    save_id: Option<String>,
    label: String,
    size: u64,
    status: DownloadStatus,
    received_bytes: u64,
    error: Option<String>,
    /// 是否占过并发名额（未启动的任务不做 active 计数）。
    started: bool,
    cancel_tx: watch::Sender<bool>,
    outcome_tx: watch::Sender<Option<TaskOutcome>>,
}

struct DownloaderInner {
    tasks: HashMap<String, DownloadTask>,
    queue: VecDeque<String>,
    /// `(saveId, fileId) -> taskId`，只含非终态任务。
    inflight: HashMap<(Option<String>, String), String>,
    active: usize,
    next_batch_id: u64,
    last_progress_emit: Option<Instant>,
    emit_pending: bool,
}

pub(crate) struct Downloader {
    app: AppHandle,
    inner: Mutex<DownloaderInner>,
    client: OnceLock<reqwest::Client>,
}

/// worker 需要的任务参数；入队时一次性 clone 出去。
struct TaskSpec {
    task_id: String,
    entry_id: String,
    file_id: String,
    save_id: Option<String>,
    size: u64,
    cancel_rx: watch::Receiver<bool>,
}

enum PullEnd {
    Failed(String),
    Cancelled(String),
}

impl Downloader {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self {
            app,
            inner: Mutex::new(DownloaderInner {
                tasks: HashMap::new(),
                queue: VecDeque::new(),
                inflight: HashMap::new(),
                active: 0,
                next_batch_id: 1,
                last_progress_emit: None,
                emit_pending: false,
            }),
            client: OnceLock::new(),
        }
    }

    fn client(&self) -> reqwest::Client {
        self.client
            .get_or_init(reqwest::Client::new)
            .clone()
    }

    /// 入队一批文件，返回每个任务的终态接收端（去重命中的返回共享任务）。
    pub(crate) fn enqueue_batch(
        &self,
        entry_id: &str,
        files: &[DownloadRequest],
        save_id: Option<String>,
        entry_label: Option<&str>,
    ) -> Vec<(String, watch::Receiver<Option<TaskOutcome>>)> {
        let mut watchers = Vec::with_capacity(files.len());
        {
            let mut inner = self.inner.lock().expect("downloader lock");
            let batch_id = inner.next_batch_id;
            inner.next_batch_id += 1;
            for (index, file) in files.iter().enumerate() {
                let key = (save_id.clone(), file.file_id.clone());
                if let Some(existing) = inner
                    .inflight
                    .get(&key)
                    .and_then(|task_id| inner.tasks.get(task_id))
                {
                    // 去重合并：同 key 在途任务直接共享（取消会连带所有合并方）。
                    watchers.push((existing.id.clone(), existing.outcome_tx.subscribe()));
                    continue;
                }
                let (cancel_tx, _) = watch::channel(false);
                let (outcome_tx, outcome_rx) = watch::channel(None);
                let task_id = uuid::Uuid::new_v4().to_string();
                inner.tasks.insert(
                    task_id.clone(),
                    DownloadTask {
                        id: task_id.clone(),
                        batch_id,
                        entry_id: entry_id.to_string(),
                        file_id: file.file_id.clone(),
                        save_id: save_id.clone(),
                        label: task_label(entry_label, index, files.len(), &file.file_id),
                        size: file.size,
                        status: DownloadStatus::Queued,
                        received_bytes: 0,
                        error: None,
                        started: false,
                        cancel_tx,
                        outcome_tx,
                    },
                );
                inner.inflight.insert(key, task_id.clone());
                inner.queue.push_back(task_id.clone());
                watchers.push((task_id.clone(), outcome_rx));
            }
            self.pump_locked(&mut inner);
        }
        self.emit_snapshot();
        watchers
    }

    /// Windows 虚拟文件入口（virtual_files.rs 调用）；内部已去重，重复调用安全。
    pub(crate) fn enqueue_virtual(&self, entry_id: &str, file_id: &str, size: u64) {
        self.enqueue_batch(
            entry_id,
            &[DownloadRequest { file_id: file_id.to_string(), size }],
            None,
            None,
        );
    }

    pub(crate) fn cancel_task(&self, task_id: &str, reason: &str) {
        if self.cancel_one(task_id, reason) {
            self.emit_snapshot();
        }
    }

    /// 取消该条目所有非终态任务；返回取消数（0 = 没有下载在跑）。
    pub(crate) fn cancel_entry(&self, entry_id: &str) -> usize {
        let ids = {
            let inner = self.inner.lock().expect("downloader lock");
            inner
                .tasks
                .values()
                .filter(|task| task.entry_id == entry_id && is_active(task.status))
                .map(|task| task_id_of(task))
                .collect::<Vec<_>>()
        };
        let mut cancelled = 0;
        for task_id in ids {
            if self.cancel_one(&task_id, "已取消") {
                cancelled += 1;
            }
        }
        if cancelled > 0 {
            self.emit_snapshot();
        }
        cancelled
    }

    /// 中止全部活动任务并清空队列（断开同步 / 面板「全部取消」共用）。
    pub(crate) fn stop_all(&self, reason: &str) {
        let ids = {
            let inner = self.inner.lock().expect("downloader lock");
            inner
                .tasks
                .values()
                .filter(|task| is_active(task.status))
                .map(|task| task_id_of(task))
                .collect::<Vec<_>>()
        };
        let mut cancelled = 0;
        for task_id in ids {
            if self.cancel_one(&task_id, reason) {
                cancelled += 1;
            }
        }
        if cancelled > 0 {
            self.emit_snapshot();
        }
    }

    /// 另存会话被取消时（cancel_save_entry）停掉属于它的任务。
    pub(crate) fn cancel_by_save_id(&self, save_id: &str) {
        let ids = {
            let inner = self.inner.lock().expect("downloader lock");
            inner
                .tasks
                .values()
                .filter(|task| task.save_id.as_deref() == Some(save_id) && is_active(task.status))
                .map(|task| task_id_of(task))
                .collect::<Vec<_>>()
        };
        let mut cancelled = 0;
        for task_id in ids {
            if self.cancel_one(&task_id, "另存为已取消") {
                cancelled += 1;
            }
        }
        if cancelled > 0 {
            self.emit_snapshot();
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<TaskSnapshot> {
        let inner = self.inner.lock().expect("downloader lock");
        self.snapshot_locked(&inner)
    }

    // ----- 内部 -----

    /// 放行队列：并发名额内的 queued 任务标记 downloading 并 spawn worker。
    /// 持锁调用安全——spawn 只调度，不内联执行。
    fn pump_locked(&self, inner: &mut DownloaderInner) {
        while inner.active < MAX_CONCURRENT_DOWNLOADS {
            let Some(task_id) = inner.queue.pop_front() else { break };
            let Some(task) = inner.tasks.get_mut(&task_id) else { continue };
            if task.status != DownloadStatus::Queued {
                continue;
            }
            inner.active += 1;
            task.started = true;
            task.status = DownloadStatus::Downloading;
            let spec = TaskSpec {
                task_id: task_id.clone(),
                entry_id: task.entry_id.clone(),
                file_id: task.file_id.clone(),
                save_id: task.save_id.clone(),
                size: task.size,
                cancel_rx: task.cancel_tx.subscribe(),
            };
            tauri::async_runtime::spawn(run_task(self.app.clone(), spec));
        }
    }

    /// 取消单个任务；返回是否真的取消了（false = 不存在或已终态）。
    fn cancel_one(&self, task_id: &str, reason: &str) -> bool {
        let mut inner = self.inner.lock().expect("downloader lock");
        let key = {
            let Some(task) = inner.tasks.get_mut(task_id) else { return false };
            if !is_active(task.status) {
                return false;
            }
            task.status = DownloadStatus::Cancelled;
            task.error = Some(reason.to_string());
            task.received_bytes = 0;
            let key = (task.save_id.clone(), task.file_id.clone());
            let _ = task.cancel_tx.send(true);
            // 终态立即 resolve 等待方；downloading 任务的传输清理由 worker 的
            // 取消路径异步完成（run_task 里 cancel_transfer）。
            let _ = task.outcome_tx.send(Some(TaskOutcome::Cancelled(reason.to_string())));
            key
        };
        inner.queue.retain(|id| id != task_id);
        if inner.inflight.get(&key).map(|id| id == task_id).unwrap_or(false) {
            inner.inflight.remove(&key);
        }
        true
    }

    /// worker 终态回写：先落盘后 resolve 的顺序由 run_task 保证（finish_transfer
    /// 完成后才可能拿到 Succeeded），另存会话的簿记因此先于批次返回。
    fn complete(&self, task_id: &str, outcome: TaskOutcome) {
        {
            let mut inner = self.inner.lock().expect("downloader lock");
            let (started, key) = {
                let Some(task) = inner.tasks.get_mut(task_id) else { return };
                task.status = match &outcome {
                    TaskOutcome::Succeeded => DownloadStatus::Succeeded,
                    TaskOutcome::Failed(_) => DownloadStatus::Failed,
                    TaskOutcome::Cancelled(_) => DownloadStatus::Cancelled,
                };
                task.error = match &outcome {
                    TaskOutcome::Succeeded => None,
                    TaskOutcome::Failed(message) | TaskOutcome::Cancelled(message) => {
                        Some(message.clone())
                    }
                };
                if task.status == DownloadStatus::Succeeded {
                    task.received_bytes = task.size;
                } else if task.status == DownloadStatus::Cancelled {
                    task.received_bytes = 0;
                }
                let started = task.started;
                let key = (task.save_id.clone(), task.file_id.clone());
                let _ = task.outcome_tx.send(Some(outcome));
                (started, key)
            };
            if started {
                inner.active -= 1;
            }
            if inner.inflight.get(&key).map(|id| id == task_id).unwrap_or(false) {
                inner.inflight.remove(&key);
            }
            self.schedule_prune_locked(&inner, task_id);
        }
        self.emit_snapshot();
        {
            let mut inner = self.inner.lock().expect("downloader lock");
            self.pump_locked(&mut inner);
        }
    }

    /// 字节进度：更新任务并按节流窗口决定是否立即推送。
    fn progress(&self, task_id: &str, received_bytes: u64) {
        let snapshot = {
            let mut inner = self.inner.lock().expect("downloader lock");
            if let Some(task) = inner.tasks.get_mut(task_id) {
                task.received_bytes = received_bytes;
            }
            let due = inner
                .last_progress_emit
                .map_or(true, |last| last.elapsed() >= PROGRESS_THROTTLE);
            if due {
                inner.last_progress_emit = Some(Instant::now());
                inner.emit_pending = false;
                Some(self.snapshot_locked(&inner))
            } else {
                if !inner.emit_pending {
                    inner.emit_pending = true;
                    let app = self.app.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(PROGRESS_THROTTLE).await;
                        app.state::<AppState>().downloader.flush_pending();
                    });
                }
                None
            }
        };
        if let Some(snapshot) = snapshot {
            let _ = self.app.emit(DOWNLOAD_CHANGED_EVENT, snapshot);
        }
    }

    fn flush_pending(&self) {
        let snapshot = {
            let mut inner = self.inner.lock().expect("downloader lock");
            if !inner.emit_pending {
                return;
            }
            inner.emit_pending = false;
            inner.last_progress_emit = Some(Instant::now());
            self.snapshot_locked(&inner)
        };
        let _ = self.app.emit(DOWNLOAD_CHANGED_EVENT, snapshot);
    }

    /// 全量快照：downloading → queued → 终态 排序。
    fn snapshot_locked(&self, inner: &DownloaderInner) -> Vec<TaskSnapshot> {
        let priority = |status: DownloadStatus| match status {
            DownloadStatus::Downloading => 0,
            DownloadStatus::Queued => 1,
            _ => 2,
        };
        let mut tasks: Vec<&DownloadTask> = inner.tasks.values().collect();
        tasks.sort_by_key(|task| (priority(task.status), task.batch_id));
        tasks
            .into_iter()
            .map(|task| TaskSnapshot {
                id: task_id_of(task),
                batch_id: task.batch_id,
                entry_id: task.entry_id.clone(),
                file_id: task.file_id.clone(),
                save_id: task.save_id.clone(),
                label: task.label.clone(),
                size: task.size,
                status: task.status,
                received_bytes: task.received_bytes,
                error: task.error.clone(),
            })
            .collect()
    }

    fn emit_snapshot(&self) {
        let snapshot = self.snapshot();
        let _ = self.app.emit(DOWNLOAD_CHANGED_EVENT, snapshot);
    }

    /// 终态任务保留一段时间后剪枝，再推一次快照。
    fn schedule_prune_locked(&self, inner: &DownloaderInner, task_id: &str) {
        if inner.tasks.get(task_id).map(|task| is_active(task.status)) == Some(true) {
            return;
        }
        let app = self.app.clone();
        let task_id = task_id.to_string();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(TERMINAL_RETENTION).await;
            let state = app.state::<AppState>();
            let removed = {
                let downloader = &state.downloader;
                let mut inner = downloader.inner.lock().expect("downloader lock");
                let terminal = inner
                    .tasks
                    .get(&task_id)
                    .map(|task| !is_active(task.status))
                    .unwrap_or(false);
                if terminal {
                    inner.tasks.remove(&task_id);
                }
                terminal
            };
            if removed {
                let snapshot = state.downloader.snapshot();
                let _ = app.emit(DOWNLOAD_CHANGED_EVENT, snapshot);
            }
        });
    }
}

fn is_active(status: DownloadStatus) -> bool {
    matches!(status, DownloadStatus::Queued | DownloadStatus::Downloading)
}

fn task_id_of(task: &DownloadTask) -> String {
    task.id.clone()
}

/// 面板显示名：有 entryLabel 时多文件拼序号，缺省用 fileId 前 8 位。
fn task_label(entry_label: Option<&str>, index: usize, total: usize, file_id: &str) -> String {
    let trimmed = entry_label.map(str::trim).filter(|value| !value.is_empty());
    let name = match trimmed {
        Some(name) => name.to_string(),
        None => format!("{}…", &file_id[..file_id.len().min(8)]),
    };
    if total > 1 {
        format!("{name} ({}/{total})", index + 1)
    } else {
        name
    }
}

// ---------------------------------------------------------------------------
// worker：拉流 + 重试
// ---------------------------------------------------------------------------

async fn run_task(app: AppHandle, spec: TaskSpec) {
    let outcome = download_with_retries(&app, &spec).await;
    if let TaskOutcome::Cancelled(reason) = &outcome {
        // 取消路径的传输清理（删 .part / 回滚另存会话 / 唤醒虚拟文件等待者）。
        let state = app.state::<AppState>();
        cancel_transfer(&state, &spec.task_id, reason);
    }
    app.state::<AppState>().downloader.complete(&spec.task_id, outcome);
}

async fn download_with_retries(app: &AppHandle, spec: &TaskSpec) -> TaskOutcome {
    let state = app.state::<AppState>();
    // 入队后被取消的任务不再发起传输。
    if *spec.cancel_rx.borrow() {
        return TaskOutcome::Cancelled("已取消".to_string());
    }
    if let Err(error) =
        begin_transfer(&state, &spec.task_id, &spec.file_id, spec.size, spec.save_id.as_deref())
    {
        return TaskOutcome::Failed(error);
    }

    let deadline = Instant::now() + DOWNLOAD_DEADLINE;
    let mut received: u64 = 0;
    loop {
        if received >= spec.size {
            return match finish_transfer(&state, &spec.task_id) {
                Ok(()) => TaskOutcome::Succeeded,
                Err(error) => TaskOutcome::Failed(error),
            };
        }
        if Instant::now() >= deadline {
            return TaskOutcome::Failed("文件下载超时，没有设备能够提供该文件".to_string());
        }
        match pull_once(app, spec, &mut received).await {
            Ok(()) => continue,
            Err(PullEnd::Cancelled(reason)) => return TaskOutcome::Cancelled(reason),
            Err(PullEnd::Failed(error)) => {
                // 断流退避：GET 无 offset，整个传输从零重启（删 `.part` 重开）。
                cancel_transfer(&state, &spec.task_id, &error);
                if cancellable_sleep(RETRY_DELAY, &spec.cancel_rx).await {
                    return TaskOutcome::Cancelled("已取消".to_string());
                }
                if let Err(error) = begin_transfer(
                    &state,
                    &spec.task_id,
                    &spec.file_id,
                    spec.size,
                    spec.save_id.as_deref(),
                ) {
                    return TaskOutcome::Failed(error);
                }
                received = 0;
                state.downloader.progress(&spec.task_id, 0);
            }
        }
    }
}

/// 一次 GET 拉流：服务器池有内容直接流回，否则请求挂在 relay 管道上由持有
/// 设备回填。流正常结束（可能不完整）返回 Ok，由外层判断是否重试。
async fn pull_once(app: &AppHandle, spec: &TaskSpec, received: &mut u64) -> Result<(), PullEnd> {
    let state = app.state::<AppState>();
    // 凭据现读：保证拿到最新 token；未登录直接失败。
    let (http_url, token) = {
        let config = state
            .sync_config
            .lock()
            .map_err(|error| PullEnd::Failed(error.to_string()))?
            .clone();
        match config.filter(|config| !config.session_token.trim().is_empty()) {
            Some(config) => (server_http_url(&config).map_err(PullEnd::Failed)?, config.session_token),
            None => {
                return Err(PullEnd::Failed("同步未登录，无法获取其他设备的文件".to_string()))
            }
        }
    };
    let client = state.downloader.client();
    let mut cancel_rx = spec.cancel_rx.clone();
    let response = tokio::select! {
        biased;
        _ = wait_cancelled(&mut cancel_rx) => {
            return Err(PullEnd::Cancelled("已取消".to_string()));
        }
        result = client
            .get(format!("{http_url}/files/{}/{}", spec.entry_id, spec.file_id))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .send() =>
        {
            match result {
                Ok(response) => response,
                Err(_) => return Err(PullEnd::Failed("同步连接已断开".to_string())),
            }
        }
    };
    if response.status().as_u16() == 401 {
        return Err(PullEnd::Failed("登录已失效，请重新登录".to_string()));
    }
    if !response.status().is_success() {
        // 错误响应是 JSON（{ message }）；与旧前端管线保持一致的文案回退。
        let status = response.status().as_u16();
        let message = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|body| body.get("message").and_then(|message| message.as_str()).map(str::to_string))
            .unwrap_or_else(|| format!("服务器返回错误 {status}"));
        return Err(PullEnd::Failed(message));
    }

    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = wait_cancelled(&mut cancel_rx) => {
                return Err(PullEnd::Cancelled("已取消".to_string()));
            }
            item = stream.next() => match item {
                Some(Ok(bytes)) => bytes,
                Some(Err(_)) => return Err(PullEnd::Failed("同步连接已断开".to_string())),
                None => return Ok(()),
            }
        };
        append_chunk(&state, &spec.task_id, &chunk).map_err(PullEnd::Failed)?;
        *received += chunk.len() as u64;
        state.downloader.progress(&spec.task_id, *received);
    }
}

/// 等待取消信号；sender 被丢弃（管理器销毁）按未取消处理。
async fn wait_cancelled(cancel_rx: &mut watch::Receiver<bool>) {
    loop {
        if *cancel_rx.borrow() {
            return;
        }
        if cancel_rx.changed().await.is_err() {
            return;
        }
    }
}

/// 可被取消打断的退避等待；返回 true 表示等待期间被取消。
async fn cancellable_sleep(delay: Duration, cancel_rx: &watch::Receiver<bool>) -> bool {
    let mut rx = cancel_rx.clone();
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        _ = wait_cancelled(&mut rx) => true,
    }
}

// ---------------------------------------------------------------------------
// tauri 命令
// ---------------------------------------------------------------------------

/// 入队一批文件并等待全部终态。webview 中途销毁只影响本次调用方，任务
/// 本身继续在 Rust 侧跑完。
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) async fn download_files(
    state: State<'_, AppState>,
    entry_id: String,
    files: Vec<DownloadRequest>,
    save_id: Option<String>,
    entry_label: Option<String>,
) -> Result<BatchOutcome, String> {
    let total = files.len();
    let watchers = state.downloader.enqueue_batch(&entry_id, &files, save_id, entry_label.as_deref());
    let mut cancelled = false;
    let mut failed_count = 0usize;
    for (_, mut rx) in watchers {
        let outcome = rx
            .wait_for(|outcome| outcome.is_some())
            .await
            .map_err(|error| error.to_string())?
            .clone()
            .expect("wait_for guarantees a matching value");
        match outcome {
            TaskOutcome::Succeeded => {}
            TaskOutcome::Failed(_) => failed_count += 1,
            TaskOutcome::Cancelled(_) => cancelled = true,
        }
    }
    Ok(BatchOutcome { cancelled, failed_count, total })
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn cancel_download(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    state.downloader.cancel_task(&task_id, "已取消");
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn cancel_entry_downloads(state: State<'_, AppState>, entry_id: String) -> Result<usize, String> {
    Ok(state.downloader.cancel_entry(&entry_id))
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn stop_all_downloads(state: State<'_, AppState>, reason: Option<String>) -> Result<(), String> {
    state
        .downloader
        .stop_all(reason.as_deref().unwrap_or("已取消"));
    Ok(())
}

/// 初始拉取：窗口打开时先取一次全量，之后靠 download-changed 事件跟进。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn download_tasks(state: State<'_, AppState>) -> Result<Vec<TaskSnapshot>, String> {
    Ok(state.downloader.snapshot())
}
