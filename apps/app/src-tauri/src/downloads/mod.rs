//! 下载域：全局下载管理器（Downloader）+ 它的磁盘侧原语 + 为前端提供条目
//! 内容的命令。
//!
//! Downloader 拥有文件任务表 / FIFO 队列 / 全局并发上限 / 按账号和 fileId
//! 在途去重 / reqwest 直连拉流写盘。main 与 paste 两个 webview 的下载都进
//! 这同一个实例，跨窗口同文件天然合并——旧前端方案「两个窗口各写各的
//! `.part` 互踩」的问题在此根除。拉流重试策略对齐旧前端管线：断流/401 等
//! 3 秒退避后整个传输从零重启（GET 无 offset），总超时 5 分钟。
//!
//! 锁纪律：Downloader 的 `inner` 锁内只做纯内存操作；事件 emit、
//! 其他 Mutex 一律 clone 所需数据 → drop inner →
//! 再做，避免嵌套锁。

pub(crate) mod save;

use futures_util::StreamExt;
use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::Write,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

use crate::history::clipboard::snapshot_entry_for;
use crate::content::entry_contents_of;
use crate::file::{download_path};
use crate::store::select_entry;
use crate::sync::server_http_url;
use crate::account::AccountContext;
use crate::utils::ensure_parent_dir;
use crate::AppState;

pub(crate) struct DownloadState {
    pub(crate) path: std::path::PathBuf,
    pub(crate) file_id: String,
    pub(crate) expected_size: u64,
    pub(crate) received_size: u64,
    pub(crate) hasher: Sha256,
    pub(crate) final_path: std::path::PathBuf,
}

/// 创建账号缓存中的临时文件；完成校验后才晋级为完整缓存。
pub(crate) fn begin_transfer(
    state: &AppState,
    transfer_id: &str,
    file_id: &str,
    expected_size: u64,
    account: &AccountContext,
) -> Result<(), String> {
    let final_path = download_path(&account.cache_dir, file_id).ok_or("内容标识不合法")?;
    let path = final_path.with_extension(format!("{transfer_id}.part"));
    ensure_parent_dir(&path)?;
    fs::File::create(&path).map_err(|error| error.to_string())?;
    state.downloads.lock().map_err(|error| error.to_string())?.insert(
        transfer_id.to_string(),
        DownloadState {
            path, final_path, file_id: file_id.to_string(), expected_size,
            received_size: 0, hasher: Sha256::new(),
        },
    );
    Ok(())
}

/// 追加一段已收字节：累计大小与哈希、写 `.part`。
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
    Ok(())
}

/// 校验大小和 SHA256 后，将临时文件晋级为完整缓存。
pub(crate) fn finish_transfer(state: &AppState, transfer_id: &str) -> Result<(), String> {
    let download = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(transfer_id)
        .ok_or_else(|| "文件下载任务不存在".to_string())?;
    if download.received_size != download.expected_size {
        let _ = fs::remove_file(&download.path);
        return Err("文件下载不完整".to_string());
    }
    if crate::utils::to_hex(&download.hasher.clone().finalize()) != download.file_id {
        let _ = fs::remove_file(&download.path);
        return Err("文件内容校验失败".to_string());
    }

    if let Err(error) = fs::rename(&download.path, &download.final_path) {
        let _ = fs::remove_file(&download.path);
        return Err(error.to_string());
    }
    Ok(())
}

/// 取消或失败时清理临时文件。
pub(crate) fn cancel_transfer(state: &AppState, transfer_id: &str, _reason: &str) {
    let download = state.downloads.lock().ok().and_then(|mut downloads| downloads.remove(transfer_id));
    if let Some(download) = download {
        let _ = fs::remove_file(&download.path);
    }
}

// ---------------------------------------------------------------------------
// 为前端提供条目内容候选与驱动下载的可用性快照。
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Downloader：全局下载管理器（队列 / 并发 / 去重 / 拉流 worker / 命令）
// ---------------------------------------------------------------------------
/// 同一时刻最多在下载的文件数（跨条目全局）。
const MAX_CONCURRENT_DOWNLOADS: usize = 1;
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
    account_key: String,
    id: String,
    batch_id: u64,
    entry_id: String,
    request_ids: Vec<String>,
    file_id: String,
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
    requesters: HashSet<(String, String, String)>,
    account: Arc<AccountContext>,
    id: String,
    batch_id: u64,
    entry_id: String,
    file_id: String,
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

struct DownloadCall {
    account_key: String,
    owner_id: String,
    cancel_tx: watch::Sender<bool>,
}

struct DownloaderInner {
    calls: HashMap<String, DownloadCall>,
    tasks: HashMap<String, DownloadTask>,
    queue: VecDeque<String>,
    /// (accountKey, fileId) -> taskId，只含非终态任务。
    inflight: HashMap<(String, String), String>,
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
    account: Arc<AccountContext>,
    task_id: String,
    entry_id: String,
    file_id: String,
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
                calls: HashMap::new(),
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
        account: &Arc<AccountContext>,
        owner_id: &str,
        request_id: &str,
        entry_id: &str,
        files: &[DownloadRequest],
        entry_label: Option<&str>,
    ) -> Result<Vec<(String, watch::Receiver<Option<TaskOutcome>>)>, String> {
        let mut watchers = Vec::with_capacity(files.len());
        {
            let mut inner = self.inner.lock().expect("downloader lock");
            // Closing marks the context before taking this same queue lock.
            if account.is_closed() { return Err("账号会话已结束".to_string()); }
            let (cancel_tx, _) = watch::channel(false);
            inner.calls.insert(request_id.to_string(), DownloadCall { account_key: account.key.clone(), owner_id: owner_id.to_string(), cancel_tx });
            let batch_id = inner.next_batch_id;
            inner.next_batch_id += 1;
            for (index, file) in files.iter().enumerate() {
                let key = (account.key.clone(), file.file_id.clone());
                if let Some(existing) = inner
                    .inflight
                    .get(&key)
                    .cloned()
                    .and_then(|task_id| inner.tasks.get_mut(&task_id))
                {
                    existing.requesters.insert((owner_id.to_string(), entry_id.to_string(), request_id.to_string()));
                    // 同账号同文件共享任务及完成通知。
                    watchers.push((existing.id.clone(), existing.outcome_tx.subscribe()));
                    continue;
                }
                let (cancel_tx, _) = watch::channel(false);
                let (outcome_tx, outcome_rx) = watch::channel(None);
                let task_id = uuid::Uuid::new_v4().to_string();
                inner.tasks.insert(
                    task_id.clone(),
                    DownloadTask {
                        account: account.clone(),
                        requesters: HashSet::from([(owner_id.to_string(), entry_id.to_string(), request_id.to_string())]),
                        id: task_id.clone(),
                        batch_id,
                        entry_id: entry_id.to_string(),
                        file_id: file.file_id.clone(),
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
        Ok(watchers)
    }

    pub(crate) fn cancel_task(&self, task_id: &str, reason: &str) {
        if self.cancel_one(task_id, reason) {
            self.emit_snapshot();
        }
    }

    /// 中止全部活动任务并清空队列（断开同步 / 面板「全部取消」共用）。
    pub(crate) fn stop_all(&self, reason: &str) {
        self.cancel_matching(|_| true, reason);
    }

    pub(crate) fn stop_account(&self, account: &AccountContext, owner_id: Option<&str>, reason: &str) {
        let ids: Vec<String> = {
            let inner = self.inner.lock().expect("downloader lock");
            inner.calls.iter().filter(|(_, call)| call.account_key == account.key
                && owner_id.is_none_or(|owner| call.owner_id == owner))
                .map(|(id, _)| id.clone()).collect()
        };
        for id in ids { self.release_call(&id, Some(reason)); }
    }

    /// Detach one invocation; files still needed by another invocation continue.
    fn release_call(&self, request_id: &str, reason: Option<&str>) {
        let ids = {
            let mut inner = self.inner.lock().expect("downloader lock");
            if let Some(call) = inner.calls.remove(request_id) {
                if reason.is_some() { let _ = call.cancel_tx.send(true); }
            }
            let mut ids = Vec::new();
            for task in inner.tasks.values_mut() {
                task.requesters.retain(|(_, _, request)| request != request_id);
                if is_active(task.status) && task.requesters.is_empty() { ids.push(task.id.clone()); }
            }
            ids
        };
        for id in ids { self.cancel_one_if_unused(&id, reason.unwrap_or("已取消")); }
        self.emit_snapshot();
    }

    fn call_cancel_receiver(&self, request_id: &str, owner_id: &str) -> Result<watch::Receiver<bool>, String> {
        let inner = self.inner.lock().expect("downloader lock");
        let call = inner.calls.get(request_id).filter(|call| call.owner_id == owner_id)
            .ok_or("下载调用已结束")?;
        Ok(call.cancel_tx.subscribe())
    }

    pub(crate) fn snapshot(&self) -> Vec<TaskSnapshot> {
        let inner = self.inner.lock().expect("downloader lock");
        self.snapshot_locked(&inner)
    }

    // ----- 内部 -----

    /// 取消所有命中条件的活动任务；返回取消数，有取消时推送一次快照。
    fn cancel_matching(&self, filter: impl Fn(&DownloadTask) -> bool, reason: &str) -> usize {
        let ids = {
            let inner = self.inner.lock().expect("downloader lock");
            inner
                .tasks
                .values()
                .filter(|task| filter(task) && is_active(task.status))
                .map(|task| task.id.clone())
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
        cancelled
    }

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
                account: task.account.clone(),
                task_id: task_id.clone(),
                entry_id: task.entry_id.clone(),
                file_id: task.file_id.clone(),
                size: task.size,
                cancel_rx: task.cancel_tx.subscribe(),
            };
            tauri::async_runtime::spawn(run_task(self.app.clone(), spec));
        }
    }

    /// 取消单个任务；返回是否真的取消了（false = 不存在或已终态）。
    fn cancel_one_if_unused(&self, task_id: &str, reason: &str) -> bool {
        self.cancel_one_inner(task_id, reason, true)
    }

    fn cancel_one(&self, task_id: &str, reason: &str) -> bool {
        self.cancel_one_inner(task_id, reason, false)
    }

    fn cancel_one_inner(&self, task_id: &str, reason: &str, only_unused: bool) -> bool {
        let mut inner = self.inner.lock().expect("downloader lock");
        let key = {
            let Some(task) = inner.tasks.get_mut(task_id) else { return false };
            if !is_active(task.status) || (only_unused && !task.requesters.is_empty()) {
                return false;
            }
            task.status = DownloadStatus::Cancelled;
            task.error = Some(reason.to_string());
            task.received_bytes = 0;
            let key = (task.account.key.clone(), task.file_id.clone());
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
    /// 完成后才可能拿到 Succeeded）。
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
                let key = (task.account.key.clone(), task.file_id.clone());
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
                account_key: task.account.key.clone(),
                id: task.id.clone(),
                batch_id: task.batch_id,
                entry_id: task.entry_id.clone(),
                request_ids: task.requesters.iter().map(|(_, _, request)| request.clone()).collect(),
                file_id: task.file_id.clone(),
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
    if let TaskOutcome::Cancelled(reason) | TaskOutcome::Failed(reason) = &outcome {
        // Clean up partial files before resolving the waiting batch.
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
    match snapshot_entry_for(&spec.entry_id, &spec.account) {
        Ok(snapshot) => {
            if snapshot.resolve(&spec.file_id).and_then(|path| fs::metadata(path).ok())
                .is_some_and(|metadata| metadata.is_file() && metadata.len() == spec.size) {
                return TaskOutcome::Succeeded;
            }
        }
        Err(error) => return TaskOutcome::Failed(error),
    }
    if let Err(error) =
        begin_transfer(&state, &spec.task_id, &spec.file_id, spec.size, &spec.account)
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
        // Bound waiting for both response headers and stream data. Checking
        // only between pulls cannot stop a server that never sends anything.
        let remaining = deadline.saturating_duration_since(Instant::now());
        let pull = match tokio::time::timeout(remaining, pull_once(app, spec, &mut received)).await {
            Ok(result) => result,
            Err(_) => return TaskOutcome::Failed("文件下载超时，没有设备能够提供该文件".to_string()),
        };
        match pull {
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
                    &spec.account,
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
    // Credentials are fixed when the account session enqueues this task.
    let http_url = server_http_url(&spec.account.config).map_err(PullEnd::Failed)?;
    let token = &spec.account.config.session_token;
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
pub(crate) async fn download_entry(
    state: State<'_, AppState>,
    entry_id: String,
    request_id: String,
    session_id: String,
) -> Result<BatchOutcome, String> {
    let account = state.account(&session_id)?;
    let snapshot = snapshot_entry_for(&entry_id, &account)?;
    let files: Vec<DownloadRequest> = entry_contents_of(&snapshot.entry).into_iter()
        .map(|(file_id, size)| DownloadRequest { file_id, size }).collect();
    let total = files.len();
    let watchers = state.downloader.enqueue_batch(&account, &session_id, &request_id, &entry_id, &files, Some(&snapshot.entry.content))?;
    let mut cancel_rx = match state.downloader.call_cancel_receiver(&request_id, &session_id) {
        Ok(receiver) => receiver,
        Err(_) => return Ok(BatchOutcome { cancelled: true, failed_count: 0, total }),
    };
    let result = tokio::select! {
        biased;
        _ = wait_cancelled(&mut cancel_rx) => Ok(BatchOutcome { cancelled: true, failed_count: 0, total }),
        outcome = async {
            let mut cancelled = false;
            let mut failed_count = 0;
            for (_, mut rx) in watchers {
                let outcome = rx.wait_for(|outcome| outcome.is_some()).await
                    .map_err(|error| error.to_string())?.clone().expect("terminal outcome");
                match outcome {
                    TaskOutcome::Succeeded => {}
                    TaskOutcome::Failed(_) => failed_count += 1,
                    TaskOutcome::Cancelled(_) => cancelled = true,
                }
            }
            Ok::<_, String>(BatchOutcome { cancelled, failed_count, total })
        } => outcome,
    };
    state.downloader.release_call(&request_id, None);
    result
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn cancel_entry_download_call(state: State<'_, AppState>, request_id: String, session_id: String) -> Result<(), String> {
    state.downloader.call_cancel_receiver(&request_id, &session_id)?;
    state.downloader.release_call(&request_id, Some("已取消"));
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn cancel_download(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    state.downloader.cancel_task(&task_id, "已取消");
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn stop_all_downloads(state: State<'_, AppState>, reason: Option<String>, session_id: String) -> Result<(), String> {
    let account = state.account(&session_id)?;
    state
        .downloader
        .stop_account(&account, None, reason.as_deref().unwrap_or("已取消"));
    Ok(())
}

/// 初始拉取：窗口打开时先取一次全量，之后靠 download-changed 事件跟进。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn download_tasks(state: State<'_, AppState>) -> Result<Vec<TaskSnapshot>, String> {
    Ok(state.downloader.snapshot())
}
