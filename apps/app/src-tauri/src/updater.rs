// 应用内更新的命令层。桌面端更新完全由 tauri-plugin-updater 的 JS API 处理，
// 这组命令只服务于 Android：检测/申请安装权限、查询 GitHub Release、下载 APK
// 并唤起系统安装器（实现见 platforms/mobile/android/updater.rs 与 Updater.kt）。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub url: String,
}

#[cfg(target_os = "android")]
use crate::platforms::implementation::updater as android_updater;

#[tauri::command]
pub(crate) fn native_update_permission_status() -> Result<bool, String> {
    #[cfg(target_os = "android")]
    return android_updater::can_install_packages();
    #[cfg(not(target_os = "android"))]
    Err("当前平台不支持应用内更新".to_string())
}

#[tauri::command]
pub(crate) fn native_update_request_permission() -> Result<(), String> {
    #[cfg(target_os = "android")]
    return android_updater::request_install_permission();
    #[cfg(not(target_os = "android"))]
    Err("当前平台不支持应用内更新".to_string())
}

#[tauri::command]
pub(crate) async fn native_update_check(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    #[cfg(target_os = "android")]
    return android_updater::check_update(&app).await;
    #[cfg(not(target_os = "android"))]
    {
        let _ = &app;
        Err("当前平台不支持应用内更新".to_string())
    }
}

#[tauri::command]
pub(crate) async fn native_update_download(
    app: tauri::AppHandle,
    url: String,
    version: String,
) -> Result<String, String> {
    #[cfg(target_os = "android")]
    return android_updater::download_update(&app, url, version).await;
    #[cfg(not(target_os = "android"))]
    {
        let _ = (&app, &url, &version);
        Err("当前平台不支持应用内更新".to_string())
    }
}

#[tauri::command]
pub(crate) fn native_update_install(path: String) -> Result<(), String> {
    #[cfg(target_os = "android")]
    return android_updater::install_apk(&path);
    #[cfg(not(target_os = "android"))]
    {
        let _ = &path;
        Err("当前平台不支持应用内更新".to_string())
    }
}
