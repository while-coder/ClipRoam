//! Sharing entry points, native adapters and inbox handoff.
use serde::Serialize;
use tauri::AppHandle;

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

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_pending_shares(
    app: AppHandle,
    scope: String,
) -> Result<Vec<NativeShare>, String> {
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
