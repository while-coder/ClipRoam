pub(crate) mod capture;
pub(crate) mod hashing;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) mod platform_clipboard;
pub(crate) mod output;
#[cfg(target_os = "windows")]
pub(crate) mod virtual_files;
