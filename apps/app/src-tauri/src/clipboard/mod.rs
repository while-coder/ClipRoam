pub(crate) mod capture;
pub(crate) mod hashing;
pub(crate) mod monitor;
pub(crate) mod output;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) mod platform_clipboard;
pub(crate) mod share;
#[cfg(target_os = "windows")]
pub(crate) mod virtual_files;
