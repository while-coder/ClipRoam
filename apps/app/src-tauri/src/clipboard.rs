//! OS clipboard snapshots, change buffering and clipboard writes. No account database or transfer logic.
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{GenericImageView, ImageFormat};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};
const THUMBNAIL_MAX_EDGE: u32 = 64;
const THUMBNAIL_MAX_BYTES: usize = 72 * 1024;
fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes
        .into_iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RichText {
    pub text: String,
    pub html: Option<String>,
    pub rtf: Option<String>,
}

pub(crate) fn rich_text_signature(rich_text: &RichText) -> String {
    // arboard wraps HTML on macOS to force UTF-8 interpretation. Treat that
    // transport wrapper as equivalent to the original fragment so paste does
    // not get captured back as a second history item.
    const MAC_HTML_PREFIX: &str =
        "<html><head><meta http-equiv=\"content-type\" content=\"text/html; charset=utf-8\"></head><body>";
    const MAC_HTML_SUFFIX: &str = "</body></html>";
    let html = rich_text.html.as_deref().map(|html| {
        html.strip_prefix(MAC_HTML_PREFIX)
            .and_then(|html| html.strip_suffix(MAC_HTML_SUFFIX))
            .unwrap_or(html)
    });
    let values = [
        Some(rich_text.text.as_str()),
        html,
        rich_text.rtf.as_deref(),
    ];
    let hash = fnv1a(
        values
            .into_iter()
            .flat_map(|value| value.unwrap_or_default().bytes().chain(std::iter::once(0))),
    );
    format!("{hash:016x}")
}

pub(crate) fn image_signature(image: &[u8]) -> String {
    // Clipboard encodings differ by platform (BMP, PNG, TIFF), while pasted
    // history images are WebP. Hash canonical RGBA pixels so writing an image
    // does not make the monitor capture the same pixels as a new entry.
    let canonical = image::load_from_memory(image).ok().map(|decoded| {
        let mut rgba = decoded.into_rgba8();
        let (width, height) = rgba.dimensions();
        // The 32bpp DIB roundtrip on Windows does not carry a trusted alpha
        // byte (plain BITMAPINFOHEADER has no alpha masks), so the same image
        // can come back opaque after an activation wrote it with alpha. The
        // signature only cares about visible content: hash everything opaque.
        for (_, _, pixel) in rgba.enumerate_pixels_mut() {
            pixel[3] = 255;
        }
        (width, height, rgba.into_raw())
    });
    let (prefix, bytes) = match canonical {
        Some((width, height, pixels)) => (format!("{width}x{height}"), pixels),
        None => (image.len().to_string(), image.to_vec()),
    };
    // FNV-1a is sufficient here: this only suppresses repeated reads of the current clipboard.
    let hash = fnv1a(bytes.iter().copied());
    format!("{prefix}:{hash:016x}")
}

pub(crate) fn encode_image_as_webp(
    image: &[u8],
) -> Result<(Vec<u8>, u32, u32, Option<String>), String> {
    let decoded = image::load_from_memory(image).map_err(|error| error.to_string())?;
    let (width, height) = decoded.dimensions();
    let mut output = Cursor::new(Vec::new());
    decoded
        .write_to(&mut output, ImageFormat::WebP)
        .map_err(|error| error.to_string())?;
    let thumbnail = decoded.thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE);
    let mut thumbnail_output = Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut thumbnail_output, ImageFormat::WebP)
        .map_err(|error| error.to_string())?;
    let thumbnail = thumbnail_output.into_inner();
    Ok((
        output.into_inner(),
        width,
        height,
        (thumbnail.len() <= THUMBNAIL_MAX_BYTES).then(|| BASE64.encode(thumbnail)),
    ))
}

/// Encodes an image as a 32bpp BMP with the 40-byte BITMAPINFOHEADER (V3,
/// BI_RGB). The `image` crate's encoder emits a 108-byte BITMAPV4HEADER with
/// BI_BITFIELDS, and clipboard-win's `set_bitmap_inner` only copies the first
/// 40 header bytes before handing the buffer to CreateDIBitmap — the V4
/// channel masks are dropped, so GDI reads garbage colors (everything pasted
/// or roamed onto Windows came out hue-shifted). V3 + BI_RGB carries no masks
/// and decodes back losslessly; readers treat the unused fourth byte as
/// padding, and `image` forces alpha back to 255 on decode.
pub(crate) fn decode_image_as_bmp(image: &[u8]) -> Result<Vec<u8>, String> {
    let rgba = image::load_from_memory(image)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    let (width, height) = (width as usize, height as usize);
    let pixels = rgba.into_raw();
    let data_size = pixels.len();
    let mut output = Vec::with_capacity(54 + data_size);
    // BITMAPFILEHEADER
    output.extend_from_slice(b"BM");
    output.extend_from_slice(&((54 + data_size) as u32).to_le_bytes());
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER: positive height = bottom-up rows.
    output.extend_from_slice(&40u32.to_le_bytes());
    output.extend_from_slice(&(width as i32).to_le_bytes());
    output.extend_from_slice(&(height as i32).to_le_bytes());
    output.extend_from_slice(&1u16.to_le_bytes());
    output.extend_from_slice(&32u16.to_le_bytes());
    output.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    output.extend_from_slice(&(data_size as u32).to_le_bytes());
    output.extend_from_slice(&[0; 16]); // ppm, color counts
                                        // Pixel rows bottom-up, RGBA -> BGRA.
    for row in pixels.chunks_exact(width * 4).rev() {
        for pixel in row.chunks_exact(4) {
            output.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    Ok(output)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ClipboardData {
    Text {
        text: String,
        html: Option<String>,
        rtf: Option<String>,
    },
    Files {
        paths: Vec<String>,
    },
    Image {
        data: String,
        width: u32,
        height: u32,
        thumbnail: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Capture {
    id: String,
    signature: String,
    data: ClipboardData,
}
#[derive(Clone, Serialize)]
struct ClipboardEvent<'a> {
    scope: &'a str,
    capture: &'a Capture,
}
#[derive(Default)]
pub(crate) struct ClipboardState {
    monitor: Mutex<Option<(String, Arc<AtomicBool>)>>,
    scope: Mutex<String>,
    signature: Mutex<String>,
    io: Mutex<()>,
}

fn scope_dir(app: &AppHandle, scope: &str) -> Result<PathBuf, String> {
    crate::files::validate_component(scope)?;
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("clipboard-inbox")
        .join(scope))
}
pub(crate) fn image_data(bytes: &[u8]) -> Result<ClipboardData, String> {
    let (bytes, width, height, thumbnail) = encode_image_as_webp(bytes)?;
    Ok(ClipboardData::Image {
        data: BASE64.encode(bytes),
        width,
        height,
        thumbnail: thumbnail.unwrap_or_default(),
    })
}
fn read(app: &AppHandle, text_only: bool) -> Result<Option<(ClipboardData, String)>, String> {
    if !text_only {
        if let Some(paths) =
            crate::platform::read_clipboard_files(app).filter(|paths| !paths.is_empty())
        {
            let signature = paths
                .iter()
                .map(|p| {
                    let metadata = fs::metadata(p).ok();
                    format!(
                        "{}:{}:{}",
                        p.display(),
                        metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                        metadata
                            .as_ref()
                            .and_then(crate::files::modified_millis)
                            .unwrap_or(0)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(Some((
                ClipboardData::Files {
                    paths: paths
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                },
                format!("files:{signature}"),
            )));
        }
    }
    if let Some(text) = crate::platform::read_clipboard_text(app) {
        let signature = format!("text:{}", rich_text_signature(&text));
        return Ok(Some((
            ClipboardData::Text {
                text: text.text,
                html: text.html,
                rtf: text.rtf,
            },
            signature,
        )));
    }
    if !text_only {
        if let Some(bytes) = crate::platform::read_clipboard_image(app) {
            return Ok(Some((
                image_data(&bytes)?,
                format!("image:{}", image_signature(&bytes)),
            )));
        }
    }
    Ok(None)
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_read_clipboard(
    app: AppHandle,
    text_only: bool,
) -> Result<Option<ClipboardData>, String> {
    Ok(read(&app, text_only)?.map(|(data, _)| data))
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn native_watch_clipboard(
    app: AppHandle,
    state: State<'_, ClipboardState>,
    scope: String,
    enabled: bool,
) -> Result<(), String> {
    // Mobile clipboard access must follow an explicit user action, even when
    // an existing account still has clipboard monitoring enabled.
    let enabled = enabled && !cfg!(any(target_os = "android", target_os = "ios"));
    let mut monitor = state.monitor.lock().map_err(|e| e.to_string())?;
    if !enabled && monitor.as_ref().is_some_and(|(active, _)| active != &scope) {
        return Ok(());
    }
    let _io = state.io.lock().map_err(|e| e.to_string())?;
    if let Some((_, stop)) = monitor.take() {
        stop.store(true, Ordering::Relaxed);
    }
    if !enabled {
        return Ok(());
    }
    let mut previous_scope = state.scope.lock().map_err(|e| e.to_string())?;
    if *previous_scope != scope {
        state.signature.lock().map_err(|e| e.to_string())?.clear();
        *previous_scope = scope.clone();
    }
    let dir = scope_dir(&app, &scope)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    *monitor = Some((scope.clone(), stop.clone()));
    thread::spawn(move || {
        let mut sequence = 0;
        while !stop.load(Ordering::Relaxed) {
            let skip = crate::platform::should_skip_clipboard_poll(&mut sequence);
            let state = app.state::<ClipboardState>();
            let _io = match state.io.lock() {
                Ok(io) => io,
                Err(_) => break,
            };
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match if skip { Ok(None) } else { read(&app, false) } {
                Ok(Some((data, signature))) => {
                    if let Ok(mut previous) = state.signature.lock() {
                        if *previous != signature {
                            let id = uuid::Uuid::new_v4().to_string();
                            let capture = Capture {
                                id: id.clone(),
                                signature: signature.clone(),
                                data,
                            };
                            let result = serde_json::to_vec(&capture)
                                .map_err(|e| e.to_string())
                                .and_then(|data| {
                                    crate::files::write_file_atomic(
                                        &dir.join(format!("{id}.json")),
                                        &data,
                                    )
                                });
                            if result.is_ok() {
                                *previous = signature;
                            } else if let Err(error) = result {
                                tauri_plugin_log::log::error!(
                                    "Clipboard snapshot persistence failed: {error}"
                                );
                            }
                        }
                    }
                }
                Err(error) => tauri_plugin_log::log::warn!("Clipboard read failed: {error}"),
                _ => {}
            }
            if let Err(error) = deliver_inbox(&app, &scope) {
                tauri_plugin_log::log::warn!("Clipboard delivery failed: {error}");
            }
            drop(_io);
            if !skip {
                thread::sleep(Duration::from_millis(
                    if cfg!(any(target_os = "android", target_os = "ios")) {
                        2000
                    } else {
                        350
                    },
                ));
            }
        }
    });
    Ok(())
}
fn clipboard_inbox(app: &AppHandle, scope: &str) -> Result<Vec<Capture>, String> {
    let dir = scope_dir(app, scope)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
        .collect::<Vec<_>>();
    paths.sort_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());
    paths
        .into_iter()
        .map(|entry| {
            serde_json::from_slice(&fs::read(entry.path()).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        })
        .collect()
}
fn clipboard_ack(app: &AppHandle, scope: &str, id: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(id).map_err(|_| "Invalid capture id")?;
    let path = scope_dir(app, scope)?.join(format!("{id}.json"));
    if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn deliver_inbox(app: &AppHandle, scope: &str) -> Result<(), String> {
    for capture in clipboard_inbox(app, scope)? {
        app.emit_to(
            "main",
            "cliproam://clipboard-changed",
            ClipboardEvent {
                scope,
                capture: &capture,
            },
        )
        .map_err(|error| error.to_string())?;
        clipboard_ack(app, scope, &capture.id)?;
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ClipboardWrite {
    Text {
        text: String,
        html: Option<String>,
        rtf: Option<String>,
    },
    Files {
        paths: Vec<String>,
    },
    Image {
        data: String,
    },
}
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn native_write_clipboard(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, ClipboardState>,
    payload: ClipboardWrite,
    synthesize: bool,
) -> Result<(), String> {
    #[cfg(desktop)]
    if synthesize && (window.label() != "main" || !crate::app::quick_paste_enabled(&app)) {
        return Err("只有快速粘贴模式可以执行自动粘贴".into());
    }
    let _io = state.io.lock().map_err(|e| e.to_string())?;
    match payload {
        ClipboardWrite::Text { text, html, rtf } => {
            crate::platform::write_clipboard_text(&app, &RichText { text, html, rtf })?
        }
        ClipboardWrite::Files { paths } => crate::platform::write_clipboard_files(&app, &paths)?,
        ClipboardWrite::Image { data } => crate::platform::write_clipboard_image(
            &app,
            &BASE64.decode(data).map_err(|e| e.to_string())?,
        )?,
    }
    if let Some((_, signature)) = read(&app, false)? {
        *state.signature.lock().map_err(|e| e.to_string())? = signature;
    }
    drop(_io);
    crate::platform::deliver_paste(&window, synthesize)
}
