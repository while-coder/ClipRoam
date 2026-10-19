//! Platform-independent task progression. iOS submits each returned request to
//! URLSession; Android runs the same progression with the Rust HTTP client.
use super::{auth, io, prepare, read, save, Session, Share};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
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

    pub fn next_request(&self) -> Result<Option<HttpRequest>> {
        if read(&self.directory)?.status == "complete" {
            auth::access(&self.files, &self.id, Some(None))?;
            return Ok(None);
        }
        let session = self.session()?;
        let request = super::next(&self.directory, &session)?;
        if request["complete"] == true {
            auth::access(&self.files, &self.id, Some(None))?;
            return Ok(None);
        }
        Ok(Some(HttpRequest {
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
        }))
    }

    pub fn on_response(&self, response: HttpResponse) -> Result<Option<HttpRequest>> {
        if response.body.len() > MAX_RESPONSE_BYTES {
            return Err("服务器响应过大".into());
        }
        super::advance(&self.directory, response.status.into(), &response.body)?;
        self.next_request()
    }

    pub fn fail(&self, error: &str) -> Result<()> {
        if self.directory.join("upload.json").exists() {
            let mut plan = read(&self.directory)?;
            plan.status = "failed".into();
            plan.error = Some(error.into());
            save(&self.directory, &plan)
        } else {
            auth::access(&self.files, &self.id, Some(None))?;
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
    pub fn run(&self, share: Value, attempt: u64) -> Outcome {
        let mut retry = false;
        let result = (|| -> Result<()> {
            self.prepare(share)?;
            let client = reqwest::blocking::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .timeout(std::time::Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(io)?;
            let mut request = self.next_request()?;
            while let Some(current) = request {
                let response = match execute(&client, current) {
                    Ok(response) => response,
                    Err(error) => {
                        retry = true;
                        return Err(error);
                    }
                };
                retry = response.status >= 500 || response.status == 429;
                request = self.on_response(response)?;
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
