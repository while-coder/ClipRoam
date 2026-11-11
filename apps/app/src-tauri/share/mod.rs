//! Sharing entry points, native adapters and inbox handoff.
use serde::Serialize;
use tauri::AppHandle;

#[cfg(target_os = "ios")]
static WIDGET_ENTRY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(target_os = "ios")]
pub(crate) fn request_widget_preview(app: &AppHandle, id: String) {
    use tauri::Emitter;
    *WIDGET_ENTRY.lock().unwrap() = Some(id);
    let _ = app.emit("cliproam://widget-open", ());
}

#[tauri::command]
pub(crate) async fn native_pending_widget_entry(
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "android")]
    {
        return app
            .share_receiver()
            .pending_widget_entry()
            .await
            .map_err(|e| e.to_string());
    }
    #[cfg(target_os = "ios")]
    {
        let _ = app;
        return Ok(serde_json::json!({ "id": WIDGET_ENTRY.lock().unwrap().take() }));
    }
    #[cfg(desktop)]
    {
        let _ = app;
        Ok(serde_json::json!({ "id": null }))
    }
}

#[cfg(target_os = "ios")]
static SHARE_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "ios")]
pub(crate) fn request_import(app: &AppHandle) {
    use tauri::Emitter;
    // Keep the request until login/initialization allows the frontend to import.
    // This also covers a cold launch before its event listener is registered.
    SHARE_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Err(error) = app.emit("cliproam://share-received", ()) {
        log::warn!("Failed to notify share receiver: {error}");
    }
}

#[cfg(mobile)]
use crate::files::{ensure_parent_dir, scope_directory};
#[cfg(mobile)]
use std::{fs, path::Path};
#[cfg(mobile)]
use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;

#[derive(Serialize)]
pub(crate) struct NativeShare {
    pub(crate) id: String,
    pub(crate) text: Option<String>,
    pub(crate) html: Option<String>,
    pub(crate) paths: Vec<String>,
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn native_configure_android_sms(
    app: AppHandle,
    server_address: Option<String>,
    session_token: Option<String>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.share_receiver()
            .configure_sms(server_address.as_deref(), session_token.as_deref())
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, server_address, session_token);
        Err("当前平台不支持 Android 短信同步".to_string())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn native_configure_android_widget(
    app: AppHandle,
    server_address: Option<String>,
    session_token: Option<String>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.share_receiver()
            .configure_widget(server_address.as_deref(), session_token.as_deref())
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, server_address, session_token);
        Err("当前平台不支持 Android 小组件".to_string())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn native_configure_ios_widget(
    app: AppHandle,
    server_address: Option<String>,
    session_token: Option<String>,
) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        app.share_receiver()
            .configure_widget(server_address.as_deref(), session_token.as_deref())
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, server_address, session_token);
        Err("当前平台不支持 iOS 小组件".to_string())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn native_configure_ios_shortcuts(
    app: AppHandle,
    server_address: Option<String>,
    session_token: Option<String>,
) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        app.share_receiver()
            .configure_shortcuts(server_address.as_deref(), session_token.as_deref())
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, server_address, session_token);
        Err("当前平台不支持 iOS 快捷指令".to_string())
    }
}

#[tauri::command]
pub(crate) async fn native_add_ios_sms_shortcut(app: AppHandle, url: String) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        app.share_receiver()
            .add_sms_shortcut(&url)
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, url);
        Err("快速添加短信自动同步需要 iOS 27 或更新版本".to_string())
    }
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_pending_shares(
    app: AppHandle,
    scope: String,
) -> Result<Vec<NativeShare>, String> {
    // Ordinary launches and account/focus changes must not access App Groups.
    // Consume before reading so a failed receipt is not retried on every launch.
    #[cfg(target_os = "ios")]
    if !SHARE_REQUESTED.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    #[cfg(mobile)]
    {
        let root = scope_directory(&app, &scope)?.join("share");
        app.share_receiver()
            .pending(&scope)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|share| {
                uuid::Uuid::parse_str(&share.id).map_err(|_| "分享标识不合法")?;
                let dir = root.join(&share.id);
                fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let mut paths = Vec::new();
                for (index, item) in share.items.iter().enumerate() {
                    let name = Path::new(&item.name)
                        .file_name()
                        .filter(|name| !name.is_empty())
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| format!("shared-{index}"));
                    // Each OS share item gets its own directory; duplicate names never overwrite.
                    let target = dir.join(index.to_string()).join(name);
                    ensure_parent_dir(&target)?;
                    if !target.exists() {
                        fs::copy(&item.path, &target).map_err(|e| e.to_string())?;
                    }
                    paths.push(target.to_string_lossy().into_owned());
                }
                Ok(NativeShare {
                    id: share.id,
                    text: share.text,
                    html: share.html,
                    paths,
                })
            })
            .collect()
    }
    #[cfg(desktop)]
    {
        let _ = (app, scope);
        Ok(Vec::new())
    }
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_ack_share(app: AppHandle, id: String) -> Result<(), String> {
    #[cfg(mobile)]
    {
        app.share_receiver()
            .acknowledge(&id)
            .map_err(|e| e.to_string())
    }
    #[cfg(desktop)]
    {
        let _ = (app, id);
        Ok(())
    }
}
