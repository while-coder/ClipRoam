//! Platform-independent task progression. iOS submits each returned request to
//! URLSession; Android runs the same progression with the Rust HTTP client.
use super::{atomic, auth, io, prepare, read, save, Session, Share};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
    pub callback_id: String,
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub body_path: PathBuf,
}

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub struct Task {
    files: PathBuf,
    directory: PathBuf,
    id: String,
}

impl Task {
    pub fn new(files: PathBuf, directory: PathBuf, id: String) -> Self {
        Self {
            files,
            directory,
            id,
        }
    }

    fn session(&self) -> Result<Session> {
        let value = auth::access(&self.files, &self.id, None)?.ok_or("请打开 ClipRoam 重新登录")?;
        serde_json::from_value(value).map_err(io)
    }

    pub fn prepare(&self, share: Value) -> Result<()> {
        if self.directory.join("upload.json").exists() {
            return Ok(());
        }
        let share: Share = serde_json::from_value(share).map_err(io)?;
        prepare(&self.directory, share, &self.session()?)?;
        Ok(())
    }

    /// Start from content and cached login JSON, an encrypted login file path,
    /// or the task's saved login snapshot when no explicit source is supplied.
    pub fn start_share(&self, share: Value, login: Option<Value>) -> Result<Option<HttpRequest>> {
        if self.directory.join("upload.json").exists()
            && read(&self.directory)?.status == "complete"
        {
            return self.end_share();
        }
        fs::create_dir_all(&self.directory).map_err(io)?;
        if let Some(source) = login {
            let login = match source {
                Value::String(path) => {
                    auth::read_file(Path::new(&path))?.ok_or("登录配置文件不存在")?
                }
                Value::Object(_) => source,
                _ => return Err("登录配置必须是对象或加密配置文件路径".into()),
            };
            auth::access(&self.files, &self.id, Some(Some(&login)))?;
        } else if auth::access(&self.files, &self.id, None)?.is_none() {
            let login =
                auth::access(&self.files, "current", None)?.ok_or("请打开 ClipRoam 登录")?;
            auth::access(&self.files, &self.id, Some(Some(&login)))?;
        }
        let mut share = share;
        if !share.is_object() {
            return Err("分享内容必须是对象".into());
        }
        share["ownerScope"] = self.session()?.scope.into();
        atomic(
            &self.directory.join("manifest.json"),
            &serde_json::to_vec(&share).map_err(io)?,
        )?;
        self.start_auth()
    }

    pub fn start_auth(&self) -> Result<Option<HttpRequest>> {
        let session = self.session()?;
        if session.username.is_empty() || !session.device.is_object() || session.token.is_empty() {
            return Err("缓存的登录或设备信息不完整，请打开 ClipRoam 重新登录".into());
        }
        let body_path = self.directory.join("request.body");
        atomic(
            &body_path,
            &serde_json::to_vec(&json!({
                "username":session.username,"sessionToken":session.token,"device":session.device
            }))
            .map_err(io)?,
        )?;
        self.http_request(
            HttpRequest {
                callback_id: String::new(),
                url: format!("{}/auth/verify", session.http_url.trim_end_matches('/')),
                method: "POST".into(),
                headers: BTreeMap::from([("Content-Type".into(), "application/json".into())]),
                body_path,
            },
            "auth",
        )
        .map(Some)
    }

    pub fn end_auth(&self, login: Value) -> Result<Option<HttpRequest>> {
        let token = login["sessionToken"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("服务器登录响应缺少会话凭据")?;
        let username = login["user"]["username"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("服务器登录响应缺少账号")?;
        login["expiresAt"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("服务器登录响应缺少会话有效期")?;
        let mut cached = auth::access(&self.files, &self.id, None)?.ok_or("分享登录配置缺失")?;
        cached["token"] = token.into();
        cached["username"] = username.into();
        if let Some(limit) = login["settings"]["maxStoredFileMb"]
            .as_u64()
            .and_then(|n| n.checked_mul(1024 * 1024))
        {
            if let Some(previous) = cached["fileLimit"].as_u64() {
                cached["fileLimit"] = previous.min(limit).into();
            }
        }
        if let Some(limit) = login["settings"]["maxCaptureFileCount"]
            .as_u64()
            .filter(|n| *n > 0)
        {
            if let Some(previous) = cached["maxCount"].as_u64() {
                cached["maxCount"] = previous.min(limit).into();
            }
        }
        auth::access(&self.files, &self.id, Some(Some(&cached)))?;
        let _ = fs::remove_file(self.directory.join("request.body"));
        // Auth is finished. A preparation/limit error now falls back to the
        // main app, while an interrupted auth request remains resumable.
        self.save_callback("upload")?;
        let share =
            serde_json::from_slice(&fs::read(self.directory.join("manifest.json")).map_err(io)?)
                .map_err(io)?;
        self.prepare(share)?;
        self.start_upload()
    }

    pub fn start_upload(&self) -> Result<Option<HttpRequest>> {
        let session = self.session()?;
        let request = super::next(&self.directory, &session)?;
        if request["complete"] == true {
            return self.end_share();
        }
        self.http_request(
            HttpRequest {
                callback_id: String::new(),
                url: request["url"].as_str().ok_or("请求地址缺失")?.into(),
                method: request["method"].as_str().ok_or("请求方法缺失")?.into(),
                headers: BTreeMap::from([
                    ("Authorization".into(), format!("Bearer {}", session.token)),
                    (
                        "Content-Type".into(),
                        request["contentType"]
                            .as_str()
                            .ok_or("请求类型缺失")?
                            .into(),
                    ),
                ]),
                body_path: request["bodyPath"].as_str().ok_or("请求文件缺失")?.into(),
            },
            "upload",
        )
        .map(Some)
    }

    fn save_callback(&self, callback_id: &str) -> Result<()> {
        atomic(
            &self.directory.join("callback.json"),
            &serde_json::to_vec(callback_id).map_err(io)?,
        )
    }

    pub fn http_request(&self, mut info: HttpRequest, callback_id: &str) -> Result<HttpRequest> {
        if !matches!(callback_id, "auth" | "upload") {
            return Err("未知 HTTP callbackId".into());
        }
        let url = url::Url::parse(&info.url).map_err(io)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("分享服务器地址无效".into());
        }
        self.save_callback(callback_id)?;
        info.callback_id = callback_id.into();
        Ok(info)
    }

    pub fn http_response(
        &self,
        callback_id: &str,
        response: HttpResponse,
    ) -> Result<Option<HttpRequest>> {
        if self.directory.join("upload.json").exists()
            && read(&self.directory)?.status == "complete"
        {
            return self.end_share();
        }
        let expected: String =
            serde_json::from_slice(&fs::read(self.directory.join("callback.json")).map_err(io)?)
                .map_err(io)?;
        if expected != callback_id {
            return Err("HTTP callbackId 与当前任务阶段不一致".into());
        }
        if response.body.len() > MAX_RESPONSE_BYTES {
            return Err("服务器响应过大".into());
        }
        match callback_id {
            "auth" => {
                if !(200..300).contains(&response.status) {
                    return Err(if response.status == 401 {
                        "登录已失效，请打开 ClipRoam 重新登录".into()
                    } else {
                        format!("分享鉴权失败（HTTP {}）", response.status)
                    });
                }
                self.end_auth(serde_json::from_str(&response.body).map_err(io)?)
            }
            "upload" => self.end_upload(response),
            _ => Err("未知 HTTP callbackId".into()),
        }
    }

    pub fn end_upload(&self, response: HttpResponse) -> Result<Option<HttpRequest>> {
        super::advance(&self.directory, response.status.into(), &response.body)?;
        self.start_upload()
    }

    pub fn end_share(&self) -> Result<Option<HttpRequest>> {
        auth::access(&self.files, &self.id, Some(None))?;
        let _ = fs::remove_file(self.directory.join("request.body"));
        match fs::remove_file(self.directory.join("callback.json")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(error)),
        }
        Ok(None)
    }

    pub fn fail(&self, error: &str) -> Result<()> {
        let _ = fs::remove_file(self.directory.join("request.body"));
        if self.directory.join("upload.json").exists() {
            let mut plan = read(&self.directory)?;
            plan.status = "failed".into();
            plan.error = Some(error.into());
            save(&self.directory, &plan)
        } else {
            let callback: Option<String> = fs::read(self.directory.join("callback.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            if callback.as_deref() != Some("auth") {
                auth::access(&self.files, &self.id, Some(None))?;
                let _ = fs::remove_file(self.directory.join("native"));
            }
            Ok(())
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
    Retry,
    Failed,
}

#[cfg(not(target_os = "ios"))]
impl Task {
    /// Runs synchronously on the Android Worker thread. All HTTP and response
    /// processing stay in Rust, with progress saved after every response.
    pub fn run(&self, share: Value, login: Option<Value>, attempt: u64) -> Outcome {
        let mut retry = false;
        let result = (|| -> Result<()> {
            let client = reqwest::blocking::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .timeout(std::time::Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(io)?;
            let mut request = self.start_share(share, login)?;
            while let Some(current) = request {
                let callback_id = current.callback_id.clone();
                let response = match execute(&client, current) {
                    Ok(response) => response,
                    Err(error) => {
                        retry = true;
                        return Err(error);
                    }
                };
                retry = response.status >= 500 || response.status == 429;
                request = self.http_response(&callback_id, response)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => Outcome::Success,
            Err(error) => {
                let _ = self.fail(&error);
                if retry && attempt < 3 {
                    Outcome::Retry
                } else {
                    Outcome::Failed
                }
            }
        }
    }
}

#[cfg(not(target_os = "ios"))]
fn execute(client: &reqwest::blocking::Client, request: HttpRequest) -> Result<HttpResponse> {
    use std::io::Read;
    let mut builder = client.request(
        reqwest::Method::from_bytes(request.method.as_bytes()).map_err(io)?,
        &request.url,
    );
    for (name, value) in request.headers {
        builder = builder.header(&name, value);
    }
    let response = builder
        .body(std::fs::File::open(request.body_path).map_err(io)?)
        .send()
        .map_err(io)?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("服务器响应过大".into());
    }
    Ok(HttpResponse {
        status,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}
