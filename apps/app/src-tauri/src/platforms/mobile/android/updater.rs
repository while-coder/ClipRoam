// 应用内更新的 Android 实现：查询 GitHub Release、下载 APK、唤起系统安装器。
// 安装依赖「安装未知应用」权限（REQUEST_INSTALL_PACKAGES），权限检测与安装
// 通过 JNI 调用 Updater.kt 完成；前端约定「无权限时不自检，先引导授权」。

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::updater::UpdateInfo;

pub(crate) const UPDATE_PROGRESS_EVENT: &str = "cliproam://update-progress";
// 与 tauri.conf.json updater endpoints 保持同一份清单（updater release）。
// Android 更新项由 CI（release.yml 的 publish-android-manifest 任务）在发版时
// 写入 android 字段，客户端只读静态文件，不请求 GitHub API，避免匿名限流。
const UPDATER_MANIFEST_URL: &str =
    "https://github.com/while-coder/ClipRoam/releases/download/updater/latest.json";
const UPDATER_CLASS: &str = "com/whilecode/cliproam/Updater";
/// 下载进度节流间隔，避免大文件下载把事件总线刷爆。
const PROGRESS_EMIT_INTERVAL: u64 = 256 * 1024;

#[derive(Debug, Deserialize)]
struct UpdaterManifest {
    version: String,
    notes: Option<String>,
    android: Option<ManifestAndroidEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestAndroidEntry {
    url: String,
}

// ---------------------------------------------------------------------------
// 权限与安装（JNI -> Updater.kt）
// ---------------------------------------------------------------------------

/// 使用 Wry 管理的 Activity 和 JNI 环境，不依赖未初始化的 ndk-context 全局状态。
fn with_jni_env<T: Send + 'static>(
    f: impl FnOnce(&mut jni::JNIEnv, &jni::objects::JObject, &jni::objects::JClass) -> Result<T, String>
        + Send
        + 'static,
) -> Result<T, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    tauri::wry::prelude::dispatch(move |env, activity, _webview| {
        let result = tauri::wry::prelude::find_class(env, activity, UPDATER_CLASS.to_string())
            .map_err(|error| format!("查找 Updater 类失败: {error}"))
            .and_then(|class| f(env, activity, &class));
        // JNI 错误转为命令错误后，不能把待处理的 Java 异常留在 UI 线程上。
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        let _ = sender.send(result);
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|error| format!("等待 Android 更新操作失败: {error}"))?
}

pub(crate) fn can_install_packages() -> Result<bool, String> {
    with_jni_env(|env, context, class| {
        env.call_static_method(
            class,
            "canInstallPackages",
            "(Landroid/content/Context;)Z",
            &[jni::objects::JValue::Object(context)],
        )
        .map_err(|error| format!("canInstallPackages 调用失败: {error}"))?
        .z()
        .map_err(|error| error.to_string())
    })
}

pub(crate) fn request_install_permission() -> Result<(), String> {
    with_jni_env(|env, context, class| {
        env.call_static_method(
            class,
            "requestInstallPermission",
            "(Landroid/content/Context;)V",
            &[jni::objects::JValue::Object(context)],
        )
        .map_err(|error| format!("requestInstallPermission 调用失败: {error}"))?;
        Ok(())
    })
}

pub(crate) fn install_apk(path: &str) -> Result<(), String> {
    let path = path.to_string();
    with_jni_env(move |env, context, class| {
        let java_path = env
            .new_string(&path)
            .map_err(|error| error.to_string())?;
        env.call_static_method(
            class,
            "installApk",
            "(Landroid/content/Context;Ljava/lang/String;)V",
            &[
                jni::objects::JValue::Object(context),
                jni::objects::JValue::Object(&java_path),
            ],
        )
        .map_err(|error| format!("installApk 调用失败: {error}"))?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// 版本查询与下载
// ---------------------------------------------------------------------------

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("ClipRoam-Updater")
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())
}

/// 清单里的版本 tag 形如 v0.2.6；与 package.json 派生的应用版本做数值比较。
fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse = |value: &str| -> Vec<u64> {
        value
            .split('.')
            .map(|part| part.trim().parse().unwrap_or(0))
            .collect()
    };
    let (latest, current) = (parse(latest), parse(current));
    for index in 0..latest.len().max(current.len()) {
        match (latest.get(index), current.get(index)) {
            (Some(l), Some(c)) if l == c => continue,
            (Some(l), Some(c)) => return l > c,
            // 缺失的段按 0 处理：0.2.6.0 与 0.2.6 等价。
            (Some(_), None) => return true,
            _ => return false,
        }
    }
    false
}

pub(crate) async fn check_update(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let current = app.package_info().version.to_string();
    let response = http_client()?
        .get(UPDATER_MANIFEST_URL)
        .send()
        .await
        .map_err(|error| format!("获取最新版本失败: {error}"))?
        .error_for_status()
        .map_err(|error| format!("获取最新版本失败: {error}"))?;
    let manifest: UpdaterManifest = response
        .json()
        .await
        .map_err(|error| format!("解析最新版本信息失败: {error}"))?;
    // 清单还没有 android 字段（旧版 CI 产物）时视为无更新。
    let Some(entry) = manifest.android else {
        return Ok(None);
    };
    let latest = manifest.version.trim().trim_start_matches('v').trim();
    if latest.is_empty() || !is_newer_version(latest, &current) {
        return Ok(None);
    }
    Ok(Some(UpdateInfo {
        version: latest.to_string(),
        notes: manifest.notes.unwrap_or_default().trim().to_string(),
        url: entry.url,
    }))
}

pub(crate) async fn download_update(
    app: &AppHandle,
    url: String,
    version: String,
) -> Result<String, String> {
    let client = http_client()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("下载更新失败: {error}"))?
        .error_for_status()
        .map_err(|error| format!("下载更新失败: {error}"))?;
    let total = response.content_length();

    // 只保留当前版本的安装包，清掉历史下载残留。
    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("updates");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let destination = directory.join(format!("ClipRoam_{version}.apk"));

    let mut file = std::fs::File::create(&destination).map_err(|error| error.to_string())?;
    let mut downloaded: u64 = 0;
    let mut emitted: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("下载更新失败: {error}"))?
    {
        std::io::Write::write_all(&mut file, &chunk).map_err(|error| error.to_string())?;
        downloaded += chunk.len() as u64;
        if downloaded - emitted >= PROGRESS_EMIT_INTERVAL {
            emitted = downloaded;
            app.emit(
                UPDATE_PROGRESS_EVENT,
                UpdateProgressPayload { downloaded, total },
            )
            .map_err(|error| error.to_string())?;
        }
    }
    app.emit(
        UPDATE_PROGRESS_EVENT,
        UpdateProgressPayload { downloaded, total },
    )
    .map_err(|error| error.to_string())?;
    Ok(destination.to_string_lossy().into_owned())
}

#[derive(Clone, serde::Serialize)]
struct UpdateProgressPayload {
    downloaded: u64,
    total: Option<u64>,
}
