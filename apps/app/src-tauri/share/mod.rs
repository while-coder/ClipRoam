//! Sharing entry points, native adapters and inbox handoff.
use serde::Serialize;
use tauri::AppHandle;

#[cfg(mobile)]
mod inbox;

#[derive(Serialize)]
pub(crate) struct NativeShare {
    pub(crate) id: String,
    pub(crate) text: Option<String>,
    pub(crate) html: Option<String>,
    pub(crate) paths: Vec<String>,
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
