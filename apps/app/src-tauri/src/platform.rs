//! 跨平台原生能力的统一函数声明与转发。系统实现只放在 platforms/。
use crate::files::{NativeShare, SaveTarget};
use crate::{app::NotificationPayload, clipboard::RichText, platforms::implementation};
use std::path::PathBuf;
use tauri::AppHandle;

pub(crate) fn autostart_is_enabled() -> Result<bool, String> {
    implementation::autostart_is_enabled()
}

pub(crate) fn autostart_set_enabled(enabled: bool) -> Result<(), String> {
    implementation::autostart_set_enabled(enabled)
}

pub(crate) fn read_clipboard_text(app: &AppHandle) -> Option<RichText> {
    implementation::read_clipboard_text(app)
}

pub(crate) fn read_clipboard_files(app: &AppHandle) -> Option<Vec<PathBuf>> {
    implementation::read_clipboard_files(app)
}

pub(crate) fn read_clipboard_image(app: &AppHandle) -> Option<Vec<u8>> {
    implementation::read_clipboard_image(app)
}

pub(crate) fn write_clipboard_text(app: &AppHandle, rich_text: &RichText) -> Result<(), String> {
    implementation::write_clipboard_text(app, rich_text)
}

pub(crate) fn write_clipboard_files(app: &AppHandle, paths: &[String]) -> Result<(), String> {
    implementation::write_clipboard_files(app, paths)
}

pub(crate) fn write_clipboard_image(app: &AppHandle, image: &[u8]) -> Result<(), String> {
    implementation::write_clipboard_image(app, image)
}

pub(crate) fn open_data_directory(app: &AppHandle) -> Result<(), String> {
    implementation::open_data_directory(app)
}

pub(crate) fn show_notification(
    app: &AppHandle,
    payload: NotificationPayload,
) -> Result<(), String> {
    implementation::show_notification(app, payload)
}

pub(crate) fn begin_window_drag(window: &tauri::WebviewWindow) -> Result<(), String> {
    implementation::begin_window_drag(window)
}

pub(crate) fn capture_paste_target(app: &AppHandle) -> Result<(), String> {
    implementation::capture_paste_target(app)
}

#[cfg(target_os = "macos")]
pub(crate) fn position_quick_paste_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    implementation::position_quick_paste_window(window)
}

pub(crate) fn deliver_paste(window: &tauri::WebviewWindow, synthesize: bool) -> Result<(), String> {
    implementation::deliver_paste(window, synthesize)
}

pub(crate) fn should_skip_clipboard_poll(sequence: &mut u32) -> bool {
    implementation::should_skip_clipboard_poll(sequence)
}

pub(crate) fn on_paste_window_focus(app: AppHandle, focused: bool) {
    implementation::on_paste_window_focus(app, focused)
}

pub(crate) fn register_plugins(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    implementation::register_plugins(builder)
}

pub(crate) fn manage_platform_state(app: &AppHandle) -> Result<(), String> {
    implementation::manage_platform_state(app)
}

pub(crate) fn create_windows(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    implementation::create_windows(app)
}

#[cfg(desktop)]
pub(crate) fn setup_desktop_shell(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    implementation::setup_desktop_shell(app)
}

#[cfg(desktop)]
pub(crate) fn show_main_window(app: &AppHandle) -> Result<(), String> {
    implementation::show_main_window(app)
}

pub(crate) fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    implementation::on_window_event(window, event)
}

#[cfg(desktop)]
pub(crate) fn synthesize_paste() -> Result<(), String> {
    implementation::synthesize_paste()
}

#[cfg(desktop)]
pub(crate) fn should_ignore_paste_focus_loss(app: &AppHandle) -> bool {
    implementation::should_ignore_paste_focus_loss(app)
}

pub(crate) async fn save_dialog(
    app: AppHandle,
    single: bool,
    file_name: String,
) -> Result<Option<SaveTarget>, String> {
    implementation::save_dialog(app, single, file_name).await
}

pub(crate) fn pick_files(
    app: AppHandle,
    mode: String,
    scope: String,
) -> Result<Vec<String>, String> {
    implementation::pick_files(app, mode, scope)
}

pub(crate) fn pending_shares(app: AppHandle, scope: String) -> Result<Vec<NativeShare>, String> {
    implementation::pending_shares(app, scope)
}

pub(crate) fn ack_share(app: AppHandle, id: String) -> Result<(), String> {
    implementation::ack_share(app, id)
}

/// name 为 Some 时，destination 是父目录；返回实际创建的路径或 URI。
pub(crate) fn copy_file(
    app: &AppHandle,
    source: &str,
    destination: &str,
    name: Option<&str>,
) -> Result<String, String> {
    implementation::copy_file(app, source, destination, name)
}
pub(crate) fn create_directory(
    app: &AppHandle,
    path: &str,
    name: Option<&str>,
) -> Result<String, String> {
    implementation::create_directory(app, path, name)
}
pub(crate) fn remove_file(app: &AppHandle, path: &str) -> Result<(), String> {
    implementation::remove_file(app, path)
}
