//! Account-independent LAN transfer. Discovery is untrusted; data connections pin
//! the certificate obtained during connection setup. Users compare its code.
use axum::{
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Path as RoutePath, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::BodyExt;
mod discovery;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};
use tokio::{io::AsyncWriteExt, sync::oneshot};
use tokio_util::{io::ReaderStream, sync::CancellationToken};
use uuid::Uuid;

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const SERVICE: &str = "_cliproam._tcp.local.";
const MAX_FILE: u64 = 20 * 1024 * 1024 * 1024;
const MAX_TEXT: u64 = 1024 * 1024;
const MAX_TASKS: usize = 8;
const OFFER_TTL: Duration = Duration::from_secs(120);
const TRANSFER_TTL: Duration = Duration::from_secs(1800);
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn code(cert: &[u8]) -> String {
    Sha256::digest(cert)[..8]
        .iter()
        .map(|v| format!("{v:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

type ApiError = (StatusCode, String);
fn api_error(message: impl std::fmt::Display) -> ApiError {
    (StatusCode::BAD_REQUEST, message.to_string())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Device {
    id: String,
    name: String,
    protocol: u8,
    code: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Peer {
    address: String,
    name: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Offer {
    sender: String,
    kind: String,
    name: String,
    size: u64,
}
impl Offer {
    fn validate(&self) -> Result<(), String> {
        if self.sender.is_empty() || self.sender.len() > 128 {
            return Err("设备名称不合法".into());
        }
        if self.kind != "text" && self.kind != "file" {
            return Err("不支持的传输类型".into());
        }
        safe_name(&self.name)?;
        if self.size
            > if self.kind == "text" {
                MAX_TEXT
            } else {
                MAX_FILE
            }
        {
            return Err("内容超过传输大小限制".into());
        }
        Ok(())
    }
}
fn safe_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 240
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
        || name.ends_with(['.', ' '])
    {
        return Err("文件名不合法，请重命名后发送".into());
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    id: String,
    direction: String,
    peer: String,
    kind: String,
    name: String,
    size: u64,
    transferred: u64,
    status: String,
    created_at: u64,
    path: Option<String>,
    text: Option<String>,
    error: Option<String>,
}
struct Pending {
    offer: Offer,
    ip: IpAddr,
    expires: Instant,
    decision: Option<oneshot::Sender<bool>>,
    token: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Incoming {
    id: String,
    offer: Offer,
    address: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    running: bool,
    device: Option<Device>,
    addresses: Vec<String>,
    peers: Vec<Peer>,
    incoming: Vec<Incoming>,
    records: Vec<Record>,
    discovery_error: Option<String>,
}
#[derive(Clone)]
struct Connection {
    client: reqwest::Client,
    base: String,
    device: Device,
}
#[derive(Default)]
struct Inner {
    peers: HashMap<String, Vec<Peer>>,
    pending: HashMap<String, Pending>,
    records: Vec<Record>,
    connections: HashMap<String, Connection>,
}
struct Service {
    device: Device,
    port: u16,
    root: PathBuf,
    inner: Mutex<Inner>,
    cancel: CancellationToken,
    server: axum_server::Handle,
    discovery: Mutex<Option<discovery::Discovery>>,
    discovery_error: Mutex<Option<String>>,
}
impl Drop for Service {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.server.shutdown();
        if let Some(d) = self.discovery.get_mut().unwrap().take() {
            d.shutdown();
        }
    }
}
#[derive(Default)]
pub(crate) struct LanState(tokio::sync::Mutex<Option<Arc<Service>>>);

fn prune(inner: &mut Inner) {
    inner.pending.retain(|_, p| p.expires > Instant::now());
}
impl Service {
    fn update(&self, id: &str, transferred: u64, status: &str, error: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(record) = inner.records.iter_mut().find(|r| r.id == id) {
            record.transferred = transferred;
            record.status = status.into();
            record.error = error;
        }
    }
    fn record(&self, id: &str, offer: &Offer, direction: &str) {
        let mut inner = self.inner.lock().unwrap();
        // Retain all active jobs, but bound completed session history.
        while inner.records.len() >= 100 {
            if let Some(index) = inner
                .records
                .iter()
                .rposition(|r| matches!(r.status.as_str(), "completed" | "failed"))
            {
                inner.records.remove(index);
            } else {
                break;
            }
        }
        inner.records.insert(
            0,
            Record {
                id: id.into(),
                direction: direction.into(),
                peer: offer.sender.clone(),
                kind: offer.kind.clone(),
                name: offer.name.clone(),
                size: offer.size,
                transferred: 0,
                status: "transferring".into(),
                created_at: now(),
                path: None,
                text: None,
                error: None,
            },
        );
    }
}
async fn running(state: &LanState) -> Result<Arc<Service>, String> {
    state
        .0
        .lock()
        .await
        .clone()
        .filter(|service| !service.cancel.is_cancelled())
        .ok_or_else(|| "请先开启局域网传输".into())
}
fn addresses(port: u16) -> Vec<String> {
    let mut result: Vec<_> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
                Some(format!("{ip}:{port}"))
            }
            _ => None,
        })
        .collect();
    result.sort();
    result.dedup();
    result
}

#[tauri::command]
pub(crate) async fn native_lan_start(
    app: AppHandle,
    state: tauri::State<'_, LanState>,
    name: String,
) -> Result<(), String> {
    let mut slot = state.0.lock().await;
    if slot.as_ref().is_some_and(|s| !s.cancel.is_cancelled()) {
        return Ok(());
    }
    slot.take();
    let root = app.path().app_local_data_dir().map_err(err)?.join("lan");
    tokio::fs::create_dir_all(root.join("inbox"))
        .await
        .map_err(err)?;
    let name: String = name.trim().chars().take(40).collect();
    if name.is_empty() || name.len() > 128 {
        return Err("请输入设备名称（最多 40 个字符）".into());
    }
    let identity = root.join("device-id");
    let id = match tokio::fs::read_to_string(&identity).await {
        Ok(id) => Uuid::parse_str(id.trim()).map_err(err)?.to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = Uuid::new_v4().to_string();
            tokio::fs::write(identity, &id).await.map_err(err)?;
            id
        }
        Err(e) => return Err(err(e)),
    };
    let cert = rcgen::generate_simple_self_signed(vec!["cliproam.local".into()]).map_err(err)?;
    let der = cert.cert.der().to_vec();
    let tls = axum_server::tls_rustls::RustlsConfig::from_der(
        vec![der.clone()],
        cert.key_pair.serialize_der(),
    )
    .await
    .map_err(err)?;
    // Prefer a memorable port; DNS-SD advertises the actual fallback port.
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 53319))
        .or_else(|_| std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)))
        .map_err(err)?;
    listener.set_nonblocking(true).map_err(err)?;
    let port = listener.local_addr().map_err(err)?.port();
    let device = Device {
        id,
        name,
        protocol: 1,
        code: code(&der),
    };
    let mut inner = Inner::default();
    // Only a completed receipt is durable. Never expose interrupted partial files.
    let mut dirs = tokio::fs::read_dir(root.join("inbox")).await.map_err(err)?;
    while let Some(entry) = dirs.next_entry().await.map_err(err)? {
        if let Ok(bytes) = tokio::fs::read(entry.path().join("receipt.json")).await {
            if let Ok(record) = serde_json::from_slice::<Record>(&bytes) {
                inner.records.push(record);
            }
        } else if entry.file_type().await.map_err(err)?.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| Uuid::parse_str(name).is_ok())
            {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
    }
    inner
        .records
        .sort_by_key(|r| std::cmp::Reverse(r.created_at));
    inner.records.truncate(100);
    let service = Arc::new(Service {
        device,
        port,
        root,
        inner: Mutex::new(inner),
        cancel: CancellationToken::new(),
        server: axum_server::Handle::new(),
        discovery: Mutex::new(None),
        discovery_error: Mutex::new(None),
    });
    match discovery::start(&service) {
        Ok(discovery) => *service.discovery.lock().unwrap() = Some(discovery),
        Err(e) => {
            *service.discovery_error.lock().unwrap() =
                Some(format!("自动发现不可用，可手动连接：{e}"))
        }
    }
    let router = router(&service);
    let handle = service.server.clone();
    let cancel = service.cancel.clone();
    tauri::async_runtime::spawn(async move {
        let result = axum_server::from_tcp_rustls(listener, tls)
            .handle(handle)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await;
        if let Err(e) = result {
            log::error!("[lan] server failed: {e}");
        }
        cancel.cancel();
    });
    *slot = Some(service);
    Ok(())
}
#[tauri::command]
pub(crate) async fn native_lan_stop(state: tauri::State<'_, LanState>) -> Result<(), String> {
    if let Some(service) = state.0.lock().await.take() {
        service.cancel.cancel();
        service.server.shutdown();
        if let Some(discovery) = service.discovery.lock().unwrap().take() {
            discovery.shutdown();
        }
    }
    Ok(())
}
#[tauri::command]
pub(crate) async fn native_lan_snapshot(
    state: tauri::State<'_, LanState>,
) -> Result<Snapshot, String> {
    let slot = state.0.lock().await;
    let Some(s) = slot.as_ref() else {
        return Ok(Snapshot {
            running: false,
            device: None,
            addresses: vec![],
            peers: vec![],
            incoming: vec![],
            records: vec![],
            discovery_error: None,
        });
    };
    let mut inner = s.inner.lock().unwrap();
    prune(&mut inner);
    let discovery_error = s.discovery_error.lock().unwrap().clone();
    Ok(Snapshot {
        running: !s.cancel.is_cancelled(),
        device: Some(s.device.clone()),
        addresses: addresses(s.port),
        peers: inner.peers.values().flatten().cloned().collect(),
        incoming: inner
            .pending
            .iter()
            .filter(|(_, p)| p.decision.is_some())
            .map(|(id, p)| Incoming {
                id: id.clone(),
                offer: p.offer.clone(),
                address: p.ip.to_string(),
            })
            .collect(),
        records: inner.records.clone(),
        discovery_error,
    })
}
fn router(s: &Arc<Service>) -> Router {
    Router::new()
        .route("/v1/info", get(info))
        .route("/v1/offers", post(offer))
        .route("/v1/upload/{id}", put(upload))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(s.clone())
}
async fn info(State(s): State<Arc<Service>>) -> Json<Device> {
    Json(s.device.clone())
}

struct PendingOffer {
    service: Arc<Service>,
    id: String,
    keep: bool,
}
impl Drop for PendingOffer {
    fn drop(&mut self) {
        if !self.keep {
            self.service.inner.lock().unwrap().pending.remove(&self.id);
        }
    }
}
async fn offer(
    State(s): State<Arc<Service>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    Json(offer): Json<Offer>,
) -> Result<Json<Permit>, ApiError> {
    offer.validate().map_err(api_error)?;
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    {
        let mut inner = s.inner.lock().unwrap();
        prune(&mut inner);
        let active = inner
            .records
            .iter()
            .filter(|r| r.direction == "receive" && r.status == "transferring")
            .count();
        if inner.pending.len() + active >= MAX_TASKS {
            return Err((StatusCode::TOO_MANY_REQUESTS, "接收任务已满".into()));
        }
        inner.pending.insert(
            id.clone(),
            Pending {
                offer,
                ip: source.ip(),
                expires: Instant::now() + OFFER_TTL,
                decision: Some(tx),
                token: None,
            },
        );
    }
    let mut cleanup = PendingOffer {
        service: s.clone(),
        id: id.clone(),
        keep: false,
    };
    let accepted = tokio::select! { _ = s.cancel.cancelled() => false, result = tokio::time::timeout(OFFER_TTL, rx) => matches!(result, Ok(Ok(true))) };
    if !accepted {
        s.inner.lock().unwrap().pending.remove(&id);
        return Err((StatusCode::FORBIDDEN, "接收方拒绝、取消或确认超时".into()));
    }
    let mut inner = s.inner.lock().unwrap();
    let pending = inner
        .pending
        .get_mut(&id)
        .ok_or_else(|| api_error("请求已过期"))?;
    let token = Uuid::new_v4().to_string();
    pending.token = Some(token.clone());
    pending.expires = Instant::now() + Duration::from_secs(600);
    cleanup.keep = true;
    Ok(Json(Permit { id, token }))
}
#[derive(Serialize, Deserialize)]
struct Permit {
    id: String,
    token: String,
}
#[tauri::command]
pub(crate) async fn native_lan_decide(
    state: tauri::State<'_, LanState>,
    id: String,
    accept: bool,
) -> Result<(), String> {
    let s = running(&state).await?;
    let mut inner = s.inner.lock().unwrap();
    prune(&mut inner);
    let pending = inner.pending.get_mut(&id).ok_or("请求已过期")?;
    pending
        .decision
        .take()
        .ok_or("已处理此请求")?
        .send(accept)
        .map_err(|_| "发送方已断开连接".to_string())
}

// Files are never named by a remote path. A unique directory prevents collisions.
struct PartialDirectory {
    path: PathBuf,
    committed: bool,
}
impl Drop for PartialDirectory {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
async fn upload(
    State(s): State<Arc<Service>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<bool>, ApiError> {
    let offer = {
        let mut inner = s.inner.lock().unwrap();
        prune(&mut inner);
        let pending = inner
            .pending
            .get(&id)
            .ok_or((StatusCode::FORBIDDEN, "传输凭证无效或已过期".into()))?;
        let auth = headers.get("authorization").and_then(|h| h.to_str().ok());
        if pending.ip != source.ip()
            || pending
                .token
                .as_ref()
                .map(|token| format!("Bearer {token}"))
                .as_deref()
                != auth
            || pending.token.is_none()
        {
            return Err((StatusCode::FORBIDDEN, "传输凭证无效".into()));
        }
        inner.pending.remove(&id).unwrap().offer
    };
    s.record(&id, &offer, "receive");
    let result = tokio::select! { _ = s.cancel.cancelled() => Err("已关闭局域网传输".into()), result = tokio::time::timeout(TRANSFER_TTL, receive(&s, &id, &offer, body)) => result.unwrap_or_else(|_| Err("传输超时".into())) };
    if let Err(e) = result {
        let transferred = s
            .inner
            .lock()
            .unwrap()
            .records
            .iter()
            .find(|r| r.id == id)
            .map_or(0, |r| r.transferred);
        s.update(&id, transferred, "failed", Some(e.clone()));
        return Err(api_error(e));
    }
    Ok(Json(true))
}
async fn receive(s: &Service, id: &str, offer: &Offer, body: Body) -> Result<(), String> {
    let directory = s.root.join("inbox").join(id);
    tokio::fs::create_dir_all(&directory).await.map_err(err)?;
    let mut cleanup = PartialDirectory {
        path: directory.clone(),
        committed: false,
    };
    let partial = directory.join("content.part");
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(err)?;
    let mut stream = body.into_data_stream();
    let mut transferred = 0u64;
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(30), stream.next())
        .await
        .map_err(|_| "连接空闲超时")?
    {
        let chunk = chunk.map_err(err)?;
        transferred = transferred
            .checked_add(chunk.len() as u64)
            .ok_or("内容大小溢出")?;
        if transferred > offer.size {
            return Err("发送内容超过声明大小".into());
        }
        file.write_all(&chunk).await.map_err(err)?;
        s.update(id, transferred, "transferring", None);
    }
    if transferred != offer.size {
        return Err("内容未接收完整，请重新发送".into());
    }
    file.flush().await.map_err(err)?;
    file.sync_all().await.map_err(err)?;
    drop(file);
    let text = if offer.kind == "text" {
        Some(
            tokio::fs::read_to_string(&partial)
                .await
                .map_err(err)?
                .chars()
                .take(2000)
                .collect(),
        )
    } else {
        None
    };
    let content = directory.join("files");
    tokio::fs::create_dir_all(&content).await.map_err(err)?;
    let destination = content.join(&offer.name);
    tokio::fs::rename(&partial, &destination)
        .await
        .map_err(err)?;
    let record = {
        let inner = s.inner.lock().unwrap();
        let mut r = inner
            .records
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or("传输记录不存在")?;
        r.status = "completed".into();
        r.path = Some(destination.to_string_lossy().into_owned());
        r.text = text;
        r
    };
    tokio::fs::write(
        directory.join("receipt.tmp"),
        serde_json::to_vec(&record).map_err(err)?,
    )
    .await
    .map_err(err)?;
    tokio::fs::rename(
        directory.join("receipt.tmp"),
        directory.join("receipt.json"),
    )
    .await
    .map_err(err)?;
    cleanup.committed = true;
    let mut inner = s.inner.lock().unwrap();
    if let Some(r) = inner.records.iter_mut().find(|r| r.id == id) {
        *r = record;
    }
    Ok(())
}

// Parse literal IPs only: a manual address must never become an arbitrary URL.
fn address(value: &str) -> Result<SocketAddr, String> {
    let addr = value
        .trim()
        .parse::<SocketAddr>()
        .or_else(|_| {
            value
                .trim()
                .parse::<std::net::Ipv4Addr>()
                .map(|ip| SocketAddr::new(ip.into(), 53319))
        })
        .map_err(|_| "请输入 IPv4 地址及端口，例如 192.168.1.10:53319".to_string())?;
    if !addr.is_ipv4()
        || addr.ip().is_unspecified()
        || addr.ip().is_multicast()
        || addr.port() == 0
        || addr.ip() == IpAddr::V4(std::net::Ipv4Addr::BROADCAST)
    {
        return Err("无效的 IPv4 地址或端口".into());
    }
    Ok(addr)
}
fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(OFFER_TTL + Duration::from_secs(15))
}
async fn small_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, String> {
    if !response.status().is_success() {
        return Err(format!(
            "对方返回错误（{}），可能已拒绝、超时或服务不可用",
            response.status()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(err)? {
        if bytes.len() + chunk.len() > 16 * 1024 {
            return Err("对方返回数据过大".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(err)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Connected {
    connection_id: String,
    device: Device,
}
#[tauri::command]
pub(crate) async fn native_lan_connect(
    state: tauri::State<'_, LanState>,
    target: String,
) -> Result<Connected, String> {
    connect(&running(&state).await?, target).await
}
async fn connect(s: &Arc<Service>, target: String) -> Result<Connected, String> {
    let addr = address(&target)?;
    // The unauthenticated probe carries no user content or credentials.
    let response = client_builder()
        .danger_accept_invalid_certs(true)
        .tls_info(true)
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(err)?
        .get(format!("https://{addr}/v1/info"))
        .send()
        .await
        .map_err(err)?;
    let certificate = response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|info| info.peer_certificate())
        .ok_or("无法获取设备证书")?
        .to_vec();
    let device: Device = small_json(response).await?;
    if device.protocol != 1
        || device.code != code(&certificate)
        || device.name.is_empty()
        || device.name.len() > 128
    {
        return Err("对方不是兼容的 ClipRoam 设备".into());
    }
    if device.id == s.device.id {
        return Err("不能连接本机".into());
    }
    let client = client_builder()
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_der(&certificate).map_err(err)?)
        .resolve("cliproam.local", addr)
        .build()
        .map_err(err)?;
    let connection_id = Uuid::new_v4().to_string();
    let mut inner = s.inner.lock().unwrap();
    if inner.connections.len() >= 32 {
        inner.connections.clear();
    }
    inner.connections.insert(
        connection_id.clone(),
        Connection {
            client,
            base: format!("https://cliproam.local:{}", addr.port()),
            device: device.clone(),
        },
    );
    Ok(Connected {
        connection_id,
        device,
    })
}
#[tauri::command]
pub(crate) async fn native_lan_send(
    state: tauri::State<'_, LanState>,
    connection_id: String,
    text: Option<String>,
    path: Option<String>,
) -> Result<(), String> {
    send(&running(&state).await?, connection_id, text, path).await
}
async fn send(
    s: &Arc<Service>,
    connection_id: String,
    text: Option<String>,
    path: Option<String>,
) -> Result<(), String> {
    let connection = {
        let inner = s.inner.lock().unwrap();
        if inner
            .records
            .iter()
            .filter(|r| {
                r.direction == "send" && matches!(r.status.as_str(), "waiting" | "transferring")
            })
            .count()
            >= MAX_TASKS
        {
            return Err("发送任务已满".into());
        }
        inner
            .connections
            .get(&connection_id)
            .cloned()
            .ok_or("请重新连接目标设备")?
    };
    let (offer, body) = match (text, path) {
        (Some(text), None) => {
            let offer = Offer {
                sender: s.device.name.clone(),
                kind: "text".into(),
                name: "文本.txt".into(),
                size: text.len() as u64,
            };
            offer.validate()?;
            (offer, reqwest::Body::from(text))
        }
        (None, Some(path)) => {
            let file = tokio::fs::File::open(&path).await.map_err(err)?;
            let metadata = file.metadata().await.map_err(err)?;
            if !metadata.is_file() {
                return Err("仅支持普通文件，请将文件夹打包后发送".into());
            }
            let offer = Offer {
                sender: s.device.name.clone(),
                kind: "file".into(),
                name: Path::new(&path)
                    .file_name()
                    .and_then(|v| v.to_str())
                    .ok_or("文件名无效")?
                    .into(),
                size: metadata.len(),
            };
            offer.validate()?;
            (offer, reqwest::Body::wrap_stream(ReaderStream::new(file)))
        }
        _ => return Err("请选择文本或文件".into()),
    };
    let id = Uuid::new_v4().to_string();
    let mut local_offer = offer.clone();
    local_offer.sender = connection.device.name.clone();
    s.record(&id, &local_offer, "send");
    s.update(&id, 0, "waiting", None);
    let result = tokio::select! { _ = s.cancel.cancelled() => Err("已关闭局域网传输".into()), result = tokio::time::timeout(TRANSFER_TTL, async {
        let permit: Permit = small_json(connection.client.post(format!("{}/v1/offers", connection.base)).timeout(OFFER_TTL + Duration::from_secs(10)).json(&offer).send().await.map_err(err)?).await?;
        if Uuid::parse_str(&permit.id).is_err() || Uuid::parse_str(&permit.token).is_err() { return Err("对方返回无效的传输凭证".into()); }
        s.update(&id, 0, "transferring", None);
        // Track streamed bytes without buffering the file in memory.
        let weak = Arc::downgrade(&s); let progress_id = id.clone(); let mut sent = 0u64;
        let stream = body.into_data_stream().map_ok(move |chunk| { sent += chunk.len() as u64; if let Some(s) = weak.upgrade() { s.update(&progress_id, sent, "transferring", None); } chunk });
        let response = connection.client.put(format!("{}/v1/upload/{}", connection.base, permit.id)).bearer_auth(permit.token).header(reqwest::header::CONTENT_LENGTH, offer.size).body(reqwest::Body::wrap_stream(stream)).send().await.map_err(err)?;
        let _: bool = small_json(response).await?;
        Ok::<(), String>(())
    }) => result.unwrap_or_else(|_| Err("传输超时".into())) };
    match result {
        Ok(()) => {
            s.update(&id, offer.size, "completed", None);
            Ok(())
        }
        Err(e) => {
            let transferred = s
                .inner
                .lock()
                .unwrap()
                .records
                .iter()
                .find(|r| r.id == id)
                .map_or(0, |r| r.transferred);
            s.update(&id, transferred, "failed", Some(e.clone()));
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_names_cannot_escape_inbox() {
        for name in [
            "../a", "..", "/tmp/a", "a\\b", "a\nb", "a:b", "a.", "a ", "",
        ] {
            assert!(safe_name(name).is_err(), "{name:?}");
        }
        assert!(safe_name("报告 2026.pdf").is_ok());
    }
    #[test]
    fn addresses_are_literal_and_have_valid_ports() {
        assert_eq!(address("192.168.1.2").unwrap().port(), 53319);
        assert_eq!(address("192.168.1.2:1234").unwrap().port(), 1234);
        for invalid in [
            "https://example.com",
            "example.com:443",
            "0.0.0.0:1",
            "224.0.0.1:1",
            "192.168.1.1:0",
            "255.255.255.255:42",
        ] {
            assert!(address(invalid).is_err());
        }
    }
    #[test]
    fn offers_are_bounded() {
        let mut offer = Offer {
            sender: "Test".into(),
            kind: "text".into(),
            name: "文本.txt".into(),
            size: MAX_TEXT,
        };
        assert!(offer.validate().is_ok());
        offer.size += 1;
        assert!(offer.validate().is_err());
        offer.kind = "file".into();
        offer.size = MAX_FILE;
        assert!(offer.validate().is_ok());
        offer.size += 1;
        assert!(offer.validate().is_err());
        offer.kind = "folder".into();
        assert!(offer.validate().is_err());
    }
}

#[cfg(test)]
mod tests_network;
