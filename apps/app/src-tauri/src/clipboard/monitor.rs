//! Polling loop that turns the current OS clipboard into history entries.

use std::thread;
use std::time::Duration;
use tauri::AppHandle;

use super::capture::{capture_files, capture_image, capture_text, read_clipboard_files, read_clipboard_image, read_clipboard_text};

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn start_clipboard_monitor(app: AppHandle) {
    thread::spawn(move || {
        // Every pass reads the full clipboard and re-decodes its contents for
        // the signatures (a bitmap can be tens of megabytes), so Windows gates
        // each pass on the clipboard sequence number: an unchanged clipboard —
        // a screenshot sitting idle for hours, for example — is skipped without
        // even opening it.
        #[cfg(target_os = "windows")]
        let mut last_clipboard_sequence = 0u32;
        loop {
            #[cfg(target_os = "windows")]
            {
                let sequence = unsafe {
                    windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
                };
                if sequence != 0 && sequence == last_clipboard_sequence {
                    thread::sleep(Duration::from_millis(350));
                    continue;
                }
                last_clipboard_sequence = sequence;
            }
            if let Some(paths) = read_clipboard_files(&app).filter(|paths| !paths.is_empty()) {
                let _ = capture_files(&app, paths);
            } else if let Some(rich_text) = read_clipboard_text(&app) {
                let _ = capture_text(&app, rich_text);
            } else if let Some(image) = read_clipboard_image(&app) {
                let _ = capture_image(&app, image);
            }
            thread::sleep(Duration::from_millis(350));
        }
    });
}
