//! Desktop shortcuts are registered and dispatched without WebView callbacks.
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState as KeyState};

#[derive(Default)]
pub(crate) struct ShortcutState(Mutex<Shortcuts>);

#[derive(Default)]
struct Shortcuts {
    quick_paste: Option<Shortcut>,
    boss_key: Option<Shortcut>,
}

pub(crate) fn set_shortcut(
    app: &AppHandle,
    shortcut: Option<String>,
    boss_key: bool,
) -> Result<(), String> {
    let next = shortcut
        .map(|value| {
            value
                .trim()
                .parse::<Shortcut>()
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let state = app.state::<ShortcutState>();
    let mut state = state.0.lock().map_err(|error| error.to_string())?;
    let other = if boss_key {
        state.quick_paste
    } else {
        state.boss_key
    };
    if next.is_some() && next == other {
        return Err("老板键和快捷粘贴不能使用相同的快捷键".into());
    }
    let current = if boss_key {
        &mut state.boss_key
    } else {
        &mut state.quick_paste
    };
    let shortcuts = app.global_shortcut();
    if *current == next && next.is_none_or(|key| shortcuts.is_registered(key)) {
        return Ok(());
    }
    // Preserve the old shortcut when registration fails; roll back on unregister failure.
    if let Some(key) = next {
        shortcuts
            .on_shortcut(key, move |app, _, event| {
                if event.state != KeyState::Pressed {
                    return;
                }
                if boss_key {
                    toggle_main_window(app);
                } else if let Some(window) = app.get_webview_window("main") {
                    if let Err(error) = window.emit("cliproam://show-paste", ()) {
                        log::warn!("[quick-paste] failed to dispatch shortcut: {error}");
                    }
                }
            })
            .map_err(|error| error.to_string())?;
    }
    if let Some(key) = (*current).filter(|key| Some(*key) != next) {
        if let Err(error) = shortcuts.unregister(key) {
            if let Some(key) = next {
                let _ = shortcuts.unregister(key);
            }
            return Err(error.to_string());
        }
    }
    *current = next;
    Ok(())
}

pub(crate) fn toggle_main_window(app: &AppHandle) {
    let app = app.clone();
    // Window-mode restoration uses blocking native operations, just like the tray.
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| -> Result<(), String> {
            let window = app
                .get_webview_window("main")
                .ok_or_else(|| "main window is unavailable".to_string())?;
            if window.is_visible().map_err(|error| error.to_string())?
                && !window.is_minimized().map_err(|error| error.to_string())?
            {
                window.hide().map_err(|error| error.to_string())
            } else {
                crate::platform::show_main_window(&app)
            }
        })();
        if let Err(error) = result {
            log::warn!("[boss-key] failed to toggle main window: {error}");
        }
    });
}
