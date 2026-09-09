use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, PluginApi, PluginHandle, TauriPlugin},
    AppHandle, Manager, Runtime,
};

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
    pub mime_type: String,
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
#[serde(rename_all = "camelCase")]
struct AcknowledgeRequest<'a> {
    id: &'a str,
}

const PLUGIN_IDENTIFIER: &str = "com.while.cliproam.share";

fn mobile_init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<ShareReceiver<R>> {
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "ShareReceiverPlugin")?;
    Ok(ShareReceiver(handle))
}

pub struct ShareReceiver<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> ShareReceiver<R> {
    pub fn pending(&self) -> Result<Vec<PendingShare>> {
        self.0.run_mobile_plugin("pending", ()).map_err(Into::into)
    }

    pub fn acknowledge(&self, id: &str) -> Result<()> {
        self.0
            .run_mobile_plugin("acknowledge", AcknowledgeRequest { id })
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
