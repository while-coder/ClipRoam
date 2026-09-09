//! macOS/Linux 剪贴板集成（arboard）。
//!
//! 这些平台通过本地物化路径传输文件：macOS 发布 NSURL 对象，Linux 通过
//! arboard 的原生 X11/Wayland 后端发布 `text/uri-list`。图片在内存中以
//! RGBA 表示，在现有的 WebP/BMP 存储边界完成转换。

#![cfg_attr(all(test, target_os = "windows"), allow(dead_code))]

use arboard::{Clipboard, ImageData};
use image::{DynamicImage, ImageFormat, RgbaImage};
use std::{borrow::Cow, io::Cursor, path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Manager};

use crate::clipboard::capture::RichText;

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

pub(crate) fn read_clipboard_files(app: &AppHandle) -> Option<Vec<PathBuf>> {
    app.state::<PlatformClipboard>().read_files()
}

pub(crate) fn read_clipboard_image(app: &AppHandle) -> Option<Vec<u8>> {
    app.state::<PlatformClipboard>().read_image_as_bmp()
}

pub(crate) fn read_clipboard_text(app: &AppHandle) -> Option<RichText> {
    let clipboard = app.state::<PlatformClipboard>();
    clipboard.read_text().map(|text| RichText {
        html: clipboard.read_html(),
        text,
        rtf: None,
    })
}

pub(crate) fn write_clipboard_text(app: &AppHandle, rich_text: &RichText) -> Result<(), String> {
    app.state::<PlatformClipboard>()
        .write_text(&rich_text.text, rich_text.html.as_deref())
}

pub(crate) fn write_clipboard_files(app: &AppHandle, paths: &[String]) -> Result<(), String> {
    app.state::<PlatformClipboard>().write_files(paths)
}

pub(crate) fn write_clipboard_image(app: &AppHandle, image: &[u8]) -> Result<(), String> {
    app.state::<PlatformClipboard>().write_image(image)
}
