//! 开机启动的 Tauri 命令入口，系统实现通过 platform 统一转发。
#[tauri::command]
pub fn autostart_is_enabled() -> Result<bool, String> {
    crate::platform::autostart_is_enabled()
}

#[tauri::command]
pub fn autostart_set_enabled(enabled: bool) -> Result<(), String> {
    crate::platform::autostart_set_enabled(enabled)
}
