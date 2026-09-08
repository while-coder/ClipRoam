//! Writing entries back to the OS clipboard, including the paste strategy
//! that decides when remote contents must be materialized first.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};
use tauri::{AppHandle, State};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use tauri::Manager;

use crate::content::{file_signature, readable_path, rebuild_tree, ClipboardEntry};
use crate::store::refresh_entry_summary;
use crate::history::entry_contents_of;
use crate::{active_cache_dir, save_active_history, AppState};
use crate::transfer::save::MissingFile;

use super::capture::{decode_image_as_bmp, image_signature, rich_text_signature, safe_file_name, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilePasteStrategy {
    VirtualStream,
    MaterializedPaths,
}

impl FilePasteStrategy {
    pub(crate) fn for_entry(entry: &ClipboardEntry) -> Self {
        #[cfg(target_os = "windows")]
        if entry.kind == "files" && crate::clipboard::virtual_files::supports_entry(entry) {
            return Self::VirtualStream;
        }
        let _ = entry;
        Self::MaterializedPaths
    }

    pub(crate) fn requires_complete_content(self, kind: &str) -> bool {
        kind == "image" || (kind == "files" && self == Self::MaterializedPaths)
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn synthesize_paste() -> Result<(), String> {
    use std::mem::size_of;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_V,
    };
    fn key(vk: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    let inputs = [
        key(VK_CONTROL, 0),
        key(VK_V, 0),
        key(VK_V, KEYEVENTF_KEYUP),
        key(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(format!("SendInput inserted {sent} of {} events", inputs.len()))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn synthesize_paste() -> Result<(), String> {
    crate::clipboard::platform_clipboard::synthesize_paste()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub(crate) fn synthesize_paste() -> Result<(), String> {
    Ok(())
}

pub(crate) enum ClipboardPayload {
    Text(RichText),
    Files(Vec<String>),
    #[cfg(target_os = "windows")]
    VirtualFiles(Box<ClipboardEntry>),
    Image(Vec<u8>),
}

#[cfg(target_os = "windows")]
fn write_clipboard_files(_app: &AppHandle, paths: &[String]) -> Result<(), String> {
    use clipboard_win::{formats::FileList, Clipboard, Setter};

    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    FileList
        .write_clipboard(paths)
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_clipboard_files(app: &AppHandle, paths: &[String]) -> Result<(), String> {
    app.state::<AppState>().platform_clipboard.write_files(paths)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn write_clipboard_files(_app: &AppHandle, _paths: &[String]) -> Result<(), String> {
    Err("当前平台暂不支持文件粘贴".to_string())
}

#[cfg(target_os = "windows")]
fn write_clipboard_image(_app: &AppHandle, image: &[u8]) -> Result<(), String> {
    use clipboard_win::{options::DoClear, raw, Clipboard};

    let bitmap = decode_image_as_bmp(image)?;
    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    // clipboard-win 5.x keeps existing formats when setting a bitmap. Clear
    // them explicitly so a text-only target cannot paste stale Unicode text
    // from the previous clipboard value when it rejects the image format.
    raw::set_bitmap_with(&bitmap, DoClear)
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_clipboard_image(app: &AppHandle, image: &[u8]) -> Result<(), String> {
    app.state::<AppState>().platform_clipboard.write_image(image)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn write_clipboard_image(_app: &AppHandle, _image: &[u8]) -> Result<(), String> {
    Err("当前平台暂不支持图片粘贴".to_string())
}

pub(crate) fn write_clipboard_text(_app: &AppHandle, rich_text: &RichText) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use clipboard_win::{
            formats::{Html, Unicode},
            raw, Clipboard, Setter,
        };

        let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
        Unicode
            .write_clipboard(&rich_text.text)
            .map_err(|error| error.to_string())?;
        if let Some(html) = &rich_text.html {
            if let Some(format) = Html::new() {
                format
                    .write_clipboard(html)
                    .map_err(|error| error.to_string())?;
            }
        }
        if let Some(rtf) = &rich_text.rtf {
            let format = raw::register_format("Rich Text Format")
                .ok_or_else(|| "无法注册 RTF 剪贴板格式".to_string())?;
            let mut rtf = rtf.clone().into_bytes();
            rtf.push(0);
            raw::set_without_clear(format.get(), &rtf).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        _app
            .state::<AppState>()
            .platform_clipboard
            .write_text(&rich_text.text, rich_text.html.as_deref())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        _app.clipboard()
            .write_text(&rich_text.text)
            .map_err(|error| error.to_string())
    }
}

/// A snapshot taken under the history lock so file dialogs and disk work never
/// block the clipboard monitor.
pub(crate) struct EntrySnapshot {
    pub entry: ClipboardEntry,
    cached: HashSet<String>,
    pub cache_dir: PathBuf,
}

pub(crate) fn snapshot_entry(state: &AppState, entry_id: &str) -> Result<EntrySnapshot, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(state, &history);
    let entry = history
        .find(entry_id)
        .cloned()
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    Ok(EntrySnapshot {
        entry,
        cached: history.cached_files.clone(),
        cache_dir,
    })
}

impl EntrySnapshot {
    pub(crate) fn resolve(&self, file_id: &str) -> Option<PathBuf> {
        readable_path(&self.cache_dir, &self.cached, &self.entry, file_id)
    }
}

pub(crate) fn missing_files(snapshot: &EntrySnapshot) -> Vec<MissingFile> {
    entry_contents_of(&snapshot.entry)
        .into_iter()
        .filter(|(file_id, _)| snapshot.resolve(file_id).is_none())
        .map(|(file_id, size)| MissingFile {
            file_id,
            size,
            source_device_id: snapshot.entry.source_device_id.clone(),
        })
        .collect()
}

pub(crate) fn refresh_snapshot_summary(state: &AppState, snapshot: &EntrySnapshot, entry_id: &str) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    refresh_entry_summary(&mut history, entry_id, &snapshot.cache_dir);
    Ok(())
}

/// Writes a live clipboard activation received from another device without
/// synthesizing Paste. File-list entries are deliberately excluded: they stay
/// in history until the user explicitly chooses where to paste or save them.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn activate_remote_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let payload = match snapshot.entry.kind.as_str() {
        "files" => return Err("文件和文件夹不会自动写入漫游剪贴板".to_string()),
        "image" => {
            let file_id = snapshot
                .entry
                .image_info
                .as_ref()
                .map(|image| image.file_id.clone())
                .ok_or_else(|| "图片内容不可用".to_string())?;
            let path = snapshot
                .resolve(&file_id)
                .ok_or_else(|| "图片内容不可用".to_string())?;
            ClipboardPayload::Image(fs::read(path).map_err(|error| error.to_string())?)
        }
        _ => ClipboardPayload::Text(RichText {
            text: snapshot.entry.content.clone(),
            html: snapshot.entry.html.clone(),
            rtf: snapshot.entry.rtf.clone(),
        }),
    };

    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        match &payload {
            ClipboardPayload::Image(image) => {
                history.last_image_signature = image_signature(image);
                history.last_clipboard.clear();
                history.last_file_signature.clear();
            }
            ClipboardPayload::Text(rich_text) => {
                history.last_clipboard = rich_text_signature(rich_text);
                history.last_file_signature.clear();
                history.last_image_signature.clear();
            }
            ClipboardPayload::Files(_) => unreachable!("file activations are rejected above"),
            #[cfg(target_os = "windows")]
            ClipboardPayload::VirtualFiles(_) => unreachable!("file activations are rejected above"),
        }
        save_active_history(&state, &history)?;
    }

    match payload {
        ClipboardPayload::Text(rich_text) => write_clipboard_text(&app, &rich_text),
        ClipboardPayload::Image(image) => write_clipboard_image(&app, &image),
        ClipboardPayload::Files(_) => unreachable!("file activations are rejected above"),
        #[cfg(target_os = "windows")]
        ClipboardPayload::VirtualFiles(_) => unreachable!("file activations are rejected above"),
    }
}

pub(crate) fn apply_clipboard_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    synthesize: bool,
) -> Result<(), String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let payload = match snapshot.entry.kind.as_str() {
        "files" => {
            let file_info = snapshot
                .entry
                .file_info
                .as_ref()
                .ok_or_else(|| "该记录不包含文件".to_string())?;
            let roots = &snapshot.entry.sources.roots;
            // Copying and pasting on the same machine should not duplicate a
            // single byte, so the original paths are reused when still intact.
            let intact = !roots.is_empty()
                && roots.len() == file_info.len()
                && roots.iter().all(|path| Path::new(path).exists());
            if intact {
                ClipboardPayload::Files(roots.clone())
            } else {
                match FilePasteStrategy::for_entry(&snapshot.entry) {
                    FilePasteStrategy::VirtualStream => {
                        #[cfg(target_os = "windows")]
                        {
                            ClipboardPayload::VirtualFiles(Box::new(snapshot.entry.clone()))
                        }
                        #[cfg(not(target_os = "windows"))]
                        unreachable!("virtual file paste is only available on Windows")
                    }
                    FilePasteStrategy::MaterializedPaths => {
                        let view = snapshot
                            .cache_dir
                            .join("views")
                            .join(safe_file_name(&snapshot.entry.id));
                        let _ = fs::remove_dir_all(&view);
                        rebuild_tree(&view, file_info, &|file_id| snapshot.resolve(file_id), true)?;
                        let paths = file_info
                            .keys()
                            .map(|root| view.join(root).display().to_string())
                            .collect();
                        ClipboardPayload::Files(paths)
                    }
                }
            }
        }
        "image" => {
            let file_id = snapshot
                .entry
                .image_info
                .as_ref()
                .map(|image| image.file_id.clone())
                .ok_or_else(|| "图片内容不可用".to_string())?;
            let path = snapshot
                .resolve(&file_id)
                .ok_or_else(|| "图片内容不可用".to_string())?;
            ClipboardPayload::Image(fs::read(path).map_err(|error| error.to_string())?)
        }
        _ => ClipboardPayload::Text(RichText {
            text: snapshot.entry.content.clone(),
            html: snapshot.entry.html.clone(),
            rtf: snapshot.entry.rtf.clone(),
        }),
    };

    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        match &payload {
            ClipboardPayload::Files(paths) => {
                let paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
                history.last_file_signature = file_signature(&paths);
                history.last_clipboard.clear();
                history.last_image_signature.clear();
            }
            #[cfg(target_os = "windows")]
            ClipboardPayload::VirtualFiles(_) => {
                history.last_file_signature.clear();
                history.last_clipboard.clear();
                history.last_image_signature.clear();
            }
            ClipboardPayload::Image(image) => {
                history.last_image_signature = image_signature(image);
                history.last_clipboard.clear();
                history.last_file_signature.clear();
            }
            ClipboardPayload::Text(rich_text) => {
                history.last_clipboard = rich_text_signature(rich_text);
                history.last_file_signature.clear();
                history.last_image_signature.clear();
            }
        }
        save_active_history(&state, &history)?;
    }

    match payload {
        ClipboardPayload::Text(rich_text) => write_clipboard_text(&app, &rich_text)?,
        ClipboardPayload::Files(paths) => write_clipboard_files(&app, &paths)?,
        #[cfg(target_os = "windows")]
        ClipboardPayload::VirtualFiles(entry) => {
            crate::clipboard::virtual_files::set_clipboard(&app, window.label(), *entry)?
        }
        ClipboardPayload::Image(image) => write_clipboard_image(&app, &image)?,
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        // Mobile operating systems do not let a normal app inject a paste into
        // another app. Keep ClipRoam visible and treat activation as Copy.
        let _ = window;
        let _ = synthesize;
        return Ok(());
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        if !synthesize {
            return Ok(());
        }
        window.hide().map_err(|error| error.to_string())?;
        thread::sleep(Duration::from_millis(90));
        if let Err(error) = synthesize_paste() {
            // The clipboard content is still valid, but the user needs to see why
            // automatic delivery failed (for example missing Linux helpers or
            // macOS Accessibility permission).
            let _ = window.show();
            return Err(error);
        }
        Ok(())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn copy_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    apply_clipboard_entry(window, app, state, entry_id, false)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn paste_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    if window.label() != "paste" {
        return Err("只有快捷粘贴窗口可以执行自动粘贴".to_string());
    }

    apply_clipboard_entry(window, app, state, entry_id, true)
}

#[cfg(test)]
mod tests {
    use super::FilePasteStrategy;

    #[test]
    fn paste_strategy_owns_platform_materialization_policy() {
        let virtual_stream = FilePasteStrategy::VirtualStream;
        let materialized = FilePasteStrategy::MaterializedPaths;

        assert!(!virtual_stream.requires_complete_content("files"));
        assert!(virtual_stream.requires_complete_content("image"));
        assert!(materialized.requires_complete_content("files"));
        assert!(materialized.requires_complete_content("image"));
        assert!(!materialized.requires_complete_content("text"));
    }
}
