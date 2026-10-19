//! Shared inbox, credential snapshots and task commands. Native callers pass
//! their directories and staged text/file paths through the single JSON entry.
use super::{atomic, auth, io, upload, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn task_id(input: &Value) -> Result<&str> {
    let id = input["id"].as_str().ok_or("分享标识缺失")?;
    if id.len() != 36 || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
        return Err("分享标识无效".into());
    }
    Ok(id)
}
fn manifest(directory: &Path) -> Result<Value> {
    let path = if directory.join("manifest.json").exists() {
        directory.join("manifest.json")
    } else {
        directory.join("share.json")
    };
    serde_json::from_slice(&fs::read(path).map_err(io)?).map_err(io)
}
fn resume(files: &Path, inbox: &Path, session: &Value) -> Result<Value> {
    let mut jobs = Vec::new();
    for entry in fs::read_dir(inbox).map_err(io)? {
        let directory = entry.map_err(io)?.path();
        if !directory.is_dir() {
            continue;
        }
        let id = directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        task_id(&json!({"id":id}))?;
        let plan: Option<Value> = fs::read(directory.join("upload.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let share = manifest(&directory).ok();
        let scope = plan
            .as_ref()
            .map(|p| &p["scope"])
            .or_else(|| share.as_ref().map(|s| &s["ownerScope"]));
        if scope != Some(&session["scope"]) {
            continue;
        }
        if plan.as_ref().is_some_and(|p| p["status"] == "complete") {
            fs::remove_dir_all(directory).map_err(io)?;
            auth::access(files, &id, Some(None))?;
        } else if plan.is_some() || directory.join("native").exists() {
            auth::access(files, &id, Some(Some(session)))?;
            jobs.push(id);
        }
    }
    Ok(json!({"jobs":jobs}))
}

pub(super) fn dispatch(input: Value) -> Result<Value> {
    let files = PathBuf::from(input["files"].as_str().ok_or("缺少配置目录")?);
    let inbox = PathBuf::from(input["inbox"].as_str().ok_or("缺少分享目录")?);
    fs::create_dir_all(&inbox).map_err(io)?;
    match input["command"].as_str().ok_or("缺少操作")? {
        "configure" => {
            let session = (!input["session"].is_null()).then_some(&input["session"]);
            auth::access(&files, "current", Some(session))?;
            match session {
                Some(session) => resume(&files, &inbox, session),
                None => Ok(json!({"jobs":[]})),
            }
        }
        "resume" => match auth::access(&files, "current", None)? {
            Some(session) => resume(&files, &inbox, &session),
            None => Ok(json!({"jobs":[]})),
        },
        "stage" => {
            let mut share = input["share"].clone();
            let id = task_id(&share)?;
            let directory = inbox.join(id);
            fs::create_dir_all(&directory).map_err(io)?;
            let session = auth::access(&files, "current", None).unwrap_or(None);
            if let Some(session) = &session {
                auth::access(&files, id, Some(Some(session)))?;
                fs::write(directory.join("native"), []).map_err(io)?;
            }
            share["ownerScope"] = session
                .as_ref()
                .map(|s| s["scope"].clone())
                .unwrap_or(Value::Null);
            atomic(
                &directory.join("manifest.json"),
                &serde_json::to_vec(&share).map_err(io)?,
            )?;
            Ok(
                json!({"uploading":session.is_some(),"jobs":if session.is_some(){vec![task_id(&share)?]}else{vec![]},"message":"内容已保存，请打开 ClipRoam 登录后上传。"}),
            )
        }
        "pending" => {
            let mut shares = Vec::new();
            for entry in fs::read_dir(&inbox).map_err(io)? {
                let directory = entry.map_err(io)?.path();
                if directory.join("upload.json").exists()
                    || directory.join("native").exists()
                    || directory.join("preparing").exists()
                {
                    continue;
                }
                if let Ok(share) = manifest(&directory) {
                    if share["ownerScope"].is_null() || share["ownerScope"] == input["scope"] {
                        shares.push(share);
                    }
                }
            }
            Ok(json!(shares))
        }
        command => {
            let id = task_id(&input)?;
            let directory = inbox.join(id);
            let task = upload::Task::new(files.clone(), directory.clone(), id.into());
            match command {
                "ack" => {
                    auth::access(&files, id, Some(None))?;
                    if directory.exists() {
                        fs::remove_dir_all(directory).map_err(io)?;
                    }
                    Ok(json!({}))
                }
                "defer" => {
                    if !directory.join("upload.json").exists() {
                        let _ = fs::remove_file(directory.join("native"));
                        auth::access(&files, id, Some(None))?;
                    }
                    Ok(json!({}))
                }
                "start_share" => {
                    let share = if input["share"].is_null() {
                        manifest(&directory)
                    } else {
                        Ok(input["share"].clone())
                    };
                    let login = input.get("login").filter(|value| !value.is_null()).cloned();
                    #[cfg(target_os = "ios")]
                    {
                        let result = share.and_then(|share| task.start_share(share, login));
                        if let Err(error) = &result {
                            let _ = task.fail(error);
                        }
                        Ok(json!({"request":result?}))
                    }
                    #[cfg(not(target_os = "ios"))]
                    {
                        if !directory.exists() && input["share"].is_null() {
                            return Ok(json!({"status":"success"}));
                        }
                        let outcome = match share {
                            Ok(share) => {
                                task.run(share, login, input["attempt"].as_u64().unwrap_or(0))
                            }
                            Err(error) => {
                                let _ = task.fail(&error);
                                upload::Outcome::Failed
                            }
                        };
                        Ok(json!({"status":outcome}))
                    }
                }
                "http_response" => {
                    let result = (|| {
                        let callback_id =
                            input["callbackId"].as_str().ok_or("HTTP callbackId 缺失")?;
                        if let Some(error) = input["error"].as_str() {
                            return Err(error.to_owned());
                        }
                        task.http_response(
                            callback_id,
                            upload::HttpResponse {
                                status: input["status"]
                                    .as_u64()
                                    .and_then(|n| u16::try_from(n).ok())
                                    .ok_or("HTTP 状态无效")?,
                                body: input["body"].as_str().ok_or("HTTP 响应缺失")?.into(),
                            },
                        )
                    })();
                    if let Err(error) = &result {
                        let _ = task.fail(error);
                    }
                    Ok(json!({"request":result?}))
                }
                "error" => {
                    task.fail(input["error"].as_str().unwrap_or("网络请求失败"))?;
                    Ok(json!({}))
                }
                _ => Err("未知分享操作".into()),
            }
        }
    }
}
