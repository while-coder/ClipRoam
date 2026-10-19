//! Shared mobile inbox import and acknowledgement.
use crate::files::{ensure_parent_dir, scope_directory, NativeShare};
use std::{fs, path::Path};
use tauri::AppHandle;

pub(crate) fn pending_shares(app: AppHandle, scope: String) -> Result<Vec<NativeShare>, String> {
    use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
    let root = scope_directory(&app, &scope)?.join("share");
    app.share_receiver()
        .pending()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|share| {
            uuid::Uuid::parse_str(&share.id).map_err(|_| "分享标识不合法")?;
            let dir = root.join(&share.id);
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let mut images = Vec::new();
            let mut paths = Vec::new();
            let image = share.items.len() == 1 && share.items[0].mime_type.starts_with("image/");
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
                if image {
                    images.push(target.to_string_lossy().into_owned());
                } else {
                    paths.push(target.to_string_lossy().into_owned());
                }
            }
            Ok(NativeShare {
                id: share.id,
                text: share.text,
                html: share.html,
                images,
                paths,
            })
        })
        .collect()
}

pub(crate) fn ack_share(app: AppHandle, id: String) -> Result<(), String> {
    use tauri_plugin_cliproam_share_receiver::ShareReceiverExt;
    app.share_receiver()
        .acknowledge(&id)
        .map_err(|e| e.to_string())
}
