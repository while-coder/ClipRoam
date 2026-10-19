//! Sharing entry points, native adapters and inbox handoff.
use serde::Serialize;
use tauri::AppHandle;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub(crate) use android::attach;
#[cfg(mobile)]
mod inbox;

#[cfg(target_os = "ios")]
#[used]
static CORE_CALL: unsafe extern "C" fn(*const std::ffi::c_char) -> *mut std::ffi::c_char =
    cliproam_share_upload::cliproam_share_call;
#[cfg(target_os = "ios")]
#[used]
static CORE_FREE: unsafe extern "C" fn(*mut std::ffi::c_char) =
    cliproam_share_upload::cliproam_share_free;

#[derive(Serialize)]
pub(crate) struct NativeShare {
    pub(crate) id: String,
    pub(crate) text: Option<String>,
    pub(crate) html: Option<String>,
    pub(crate) images: Vec<String>,
    pub(crate) paths: Vec<String>,
}

#[tauri::command]
pub(crate) fn native_configure_share_session(
    app: AppHandle,
    session: Option<serde_json::Value>,
) -> Result<(), String> {
    #[cfg(mobile)]
    {
        use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
        let session = session
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| e.to_string())?;
        app.share_receiver()
            .configure_session(session)
            .map_err(|e| e.to_string())
    }
    #[cfg(desktop)]
    {
        let _ = (app, session);
        Ok(())
    }
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_pending_shares(
    app: AppHandle,
    scope: String,
) -> Result<Vec<NativeShare>, String> {
    #[cfg(mobile)]
    {
        inbox::pending_shares(app, scope)
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
        inbox::ack_share(app, id)
    }
    #[cfg(desktop)]
    {
        let _ = (app, id);
        Ok(())
    }
}
