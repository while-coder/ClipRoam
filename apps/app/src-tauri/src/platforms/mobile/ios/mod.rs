//! iOS 平台实现：除插件注册外与 Android 共享全部移动端桩。

pub(crate) use super::*;

pub(crate) fn register_plugins(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder
}
