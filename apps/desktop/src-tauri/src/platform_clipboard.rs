//! macOS/Linux clipboard integration.
//!
//! These platforms receive materialized local paths. macOS publishes NSURL
//! objects and Linux publishes `text/uri-list` through arboard's native
//! X11/Wayland backends. Images use RGBA in memory and are converted at the
//! existing WebP/BMP storage boundary.

#![cfg_attr(all(test, target_os = "windows"), allow(dead_code))]

use arboard::{Clipboard, ImageData};
use image::{DynamicImage, ImageFormat, RgbaImage};
use std::{borrow::Cow, io::Cursor, path::PathBuf, process::Command, sync::Mutex};

pub struct PlatformClipboard {
    inner: Mutex<Clipboard>,
}

impl PlatformClipboard {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            inner: Mutex::new(Clipboard::new().map_err(|error| error.to_string())?),
        })
    }

    pub fn read_files(&self) -> Option<Vec<PathBuf>> {
        self.inner
            .lock()
            .ok()?
            .get()
            .file_list()
            .ok()
            .filter(|paths| !paths.is_empty())
    }

    /// The existing capture pipeline accepts a BMP clipboard payload.
    pub fn read_image_as_bmp(&self) -> Option<Vec<u8>> {
        let image = self.inner.lock().ok()?.get_image().ok()?;
        let width = u32::try_from(image.width).ok()?;
        let height = u32::try_from(image.height).ok()?;
        let rgba = RgbaImage::from_raw(width, height, image.bytes.into_owned())?;
        let mut output = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut output, ImageFormat::Bmp)
            .ok()?;
        Some(output.into_inner())
    }

    pub fn read_text(&self) -> Option<String> {
        self.inner
            .lock()
            .ok()?
            .get_text()
            .ok()
            .filter(|text| !text.trim().is_empty())
    }

    pub fn read_html(&self) -> Option<String> {
        self.inner
            .lock()
            .ok()?
            .get()
            .html()
            .ok()
            .filter(|html| !html.is_empty())
    }

    pub fn write_files(&self, paths: &[String]) -> Result<(), String> {
        let paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
        self.inner
            .lock()
            .map_err(|error| error.to_string())?
            .set()
            .file_list(&paths)
            .map_err(|error| error.to_string())
    }

    pub fn write_image(&self, encoded: &[u8]) -> Result<(), String> {
        let image = image::load_from_memory(encoded)
            .map_err(|error| error.to_string())?
            .into_rgba8();
        let (width, height) = image.dimensions();
        self.inner
            .lock()
            .map_err(|error| error.to_string())?
            .set_image(ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Owned(image.into_raw()),
            })
            .map_err(|error| error.to_string())
    }

    pub fn write_text(&self, text: &str, html: Option<&str>) -> Result<(), String> {
        let mut clipboard = self.inner.lock().map_err(|error| error.to_string())?;
        if let Some(html) = html {
            clipboard
                .set_html(html, Some(text))
                .map_err(|error| error.to_string())
        } else {
            clipboard.set_text(text).map_err(|error| error.to_string())
        }
    }
}

fn run_paste_command(program: &str, arguments: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if message.is_empty() {
        format!("{program} 退出码：{}", output.status)
    } else {
        format!("{program}: {message}")
    })
}

#[cfg(target_os = "macos")]
pub fn synthesize_paste() -> Result<(), String> {
    run_paste_command(
        "osascript",
        &[
            "-e",
            "tell application \"System Events\" to keystroke \"v\" using command down",
        ],
    )
    .map_err(|error| {
        format!("无法模拟 Command+V，请在系统设置中允许 ClipRoam 使用辅助功能：{error}")
    })
}

#[cfg(any(target_os = "linux", all(test, target_os = "windows")))]
pub fn synthesize_paste() -> Result<(), String> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let attempts: &[(&str, &[&str])] = if wayland {
        &[
            ("wtype", &["-M", "ctrl", "-k", "v", "-m", "ctrl"]),
            ("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"]),
            ("xdotool", &["key", "--clearmodifiers", "ctrl+v"]),
        ]
    } else {
        &[
            ("xdotool", &["key", "--clearmodifiers", "ctrl+v"]),
            ("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"]),
        ]
    };
    let mut errors = Vec::new();
    for (program, arguments) in attempts {
        match run_paste_command(program, arguments) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(error),
        }
    }
    Err(format!(
        "剪贴板已写入，但无法模拟 Ctrl+V；请安装 {}。{}",
        if wayland {
            "wtype 或 ydotool"
        } else {
            "xdotool"
        },
        errors.join("；")
    ))
}
