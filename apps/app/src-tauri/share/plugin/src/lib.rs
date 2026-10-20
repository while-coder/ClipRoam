use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, PluginApi, PluginHandle, TauriPlugin},
    AppHandle, Manager, Runtime,
};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_cliproam_share_receiver);

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedItem {
    pub path: String,
    pub name: String,
}

/// Metadata read from the installed app through Kotlin / Swift.
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub android_api_level: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_build_number: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingShare {
    pub id: String,
    pub text: Option<String>,
    pub html: Option<String>,
    #[serde(default)]
    pub items: Vec<SharedItem>,
}

#[derive(Serialize)]
struct PendingRequest<'a> {
    scope: &'a str,
}

#[derive(Serialize)]
struct AcknowledgeRequest<'a> {
    id: &'a str,
}

#[derive(Deserialize)]
pub struct DirectorySelection {
    pub uri: Option<String>,
}

#[derive(Serialize)]
struct CopyFileRequest<'a> {
    source: &'a str,
    parent: &'a str,
    name: &'a str,
}
#[derive(Serialize)]
struct CreateDirectoryRequest<'a> {
    parent: &'a str,
    name: &'a str,
}
#[derive(Serialize)]
struct RemoveDocumentRequest<'a> {
    uri: &'a str,
}
#[derive(Deserialize)]
pub struct DocumentResult {
    pub uri: String,
}

#[derive(Serialize)]
struct CopyDocumentsRequest<'a> {
    uris: &'a [String],
    directory: &'a str,
}

#[derive(Deserialize)]
pub struct ImportedDocuments {
    pub paths: Vec<String>,
}

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.whilecode.cliproam.share";

fn mobile_init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<ShareReceiver<R>> {
    #[cfg(target_os = "android")]
    {
        let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "ShareReceiverPlugin")?;
        Ok(ShareReceiver(handle))
    }
    #[cfg(target_os = "ios")]
    {
        Ok(ShareReceiver(api.register_ios_plugin(
            init_plugin_cliproam_share_receiver,
        )?))
    }
}

pub struct ShareReceiver<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> ShareReceiver<R> {
    pub async fn device_metadata(&self) -> Result<DeviceMetadata> {
        self.0
            .run_mobile_plugin_async("deviceMetadata", ())
            .await
            .map_err(Into::into)
    }

    #[cfg(target_os = "android")]
    pub fn copy_documents(&self, uris: &[String], directory: &str) -> Result<ImportedDocuments> {
        self.0
            .run_mobile_plugin("copyDocuments", CopyDocumentsRequest { uris, directory })
            .map_err(Into::into)
    }
    #[cfg(target_os = "android")]
    pub fn pick_directory(&self) -> Result<DirectorySelection> {
        self.0
            .run_mobile_plugin("pickDirectory", ())
            .map_err(Into::into)
    }

    #[cfg(target_os = "android")]
    pub fn copy_file(&self, source: &str, parent: &str, name: &str) -> Result<DocumentResult> {
        self.0
            .run_mobile_plugin(
                "copyFile",
                CopyFileRequest {
                    source,
                    parent,
                    name,
                },
            )
            .map_err(Into::into)
    }
    #[cfg(target_os = "android")]
    pub fn create_directory(&self, parent: &str, name: &str) -> Result<DocumentResult> {
        self.0
            .run_mobile_plugin("createDirectory", CreateDirectoryRequest { parent, name })
            .map_err(Into::into)
    }
    #[cfg(target_os = "android")]
    pub fn remove_document(&self, uri: &str) -> Result<()> {
        self.0
            .run_mobile_plugin("removeDocument", RemoveDocumentRequest { uri })
            .map_err(Into::into)
    }

    pub fn pending(&self, scope: &str) -> Result<Vec<PendingShare>> {
        self.0
            .run_mobile_plugin("pending", PendingRequest { scope })
            .map_err(Into::into)
    }
    pub fn acknowledge(&self, id: &str) -> Result<()> {
        self.0
            .run_mobile_plugin::<serde_json::Value>("acknowledge", AcknowledgeRequest { id })
            .map(|_| ())
            .map_err(Into::into)
    }
}

pub trait ShareReceiverExt<R: Runtime> {
    fn share_receiver(&self) -> &ShareReceiver<R>;
}

impl<R: Runtime, T: Manager<R>> ShareReceiverExt<R> for T {
    fn share_receiver(&self) -> &ShareReceiver<R> {
        self.state::<ShareReceiver<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("cliproam-share-receiver")
        .setup(|app, api| {
            app.manage(mobile_init(app, api)?);
            Ok(())
        })
        .build()
}
