//! Native input mode for passive notes. No keyboard shortcuts are consumed.
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

const INPUT_EVENT: &str = "cliproam://sticky-input";

fn apply_layer(window: &WebviewWindow, on_top: bool) -> Result<(), String> {
    window.set_focusable(false).map_err(|e| e.to_string())?;
    // Clear the previous band before enabling the other one: setting bottom
    // after top would otherwise demote a pinned window on Windows.
    if on_top {
        window
            .set_always_on_bottom(false)
            .map_err(|e| e.to_string())?;
        window.set_always_on_top(true).map_err(|e| e.to_string())?;
    } else {
        window.set_always_on_top(false).map_err(|e| e.to_string())?;
        window
            .set_always_on_bottom(true)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn native_sticky_set_on_top(window: WebviewWindow, on_top: bool) -> Result<(), String> {
    if !window.label().starts_with("sticky-") {
        return Err("仅便签窗口可设置便签层级".into());
    }
    apply_layer(&window, on_top)
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InputMode {
    interactive: bool,
    alt_supported: bool,
    manual_edit: bool,
}

#[derive(Default)]
struct Note {
    manual_edit: bool,
    forward_until: Option<Instant>,
    applied: Option<InputMode>,
}

#[derive(Default)]
pub(crate) struct StickyState {
    notes: Mutex<HashMap<String, Note>>,
    alt_supported: AtomicBool,
}

fn apply(
    window: &WebviewWindow,
    note: &mut Note,
    alt: bool,
    supported: bool,
) -> Result<InputMode, String> {
    // A tray override ends as soon as the user starts the normal Alt gesture.
    if alt {
        note.manual_edit = false;
    }
    let forwarding = note
        .forward_until
        .is_some_and(|until| until > Instant::now());
    let mode = InputMode {
        interactive: !forwarding && (alt || note.manual_edit),
        alt_supported: supported,
        manual_edit: note.manual_edit,
    };
    if note.applied != Some(mode) {
        window
            .set_ignore_cursor_events(!mode.interactive)
            .map_err(|e| e.to_string())?;
        window.emit(INPUT_EVENT, mode).map_err(|e| e.to_string())?;
        note.applied = Some(mode);
    }
    Ok(mode)
}

pub(crate) fn start(app: &AppHandle) {
    app.manage(StickyState::default());
    let app = app.clone();
    std::thread::spawn(move || {
        let mut keyboard = Keyboard::new();
        let state = app.state::<StickyState>();
        state
            .alt_supported
            .store(keyboard.supported(), Ordering::Relaxed);
        loop {
            let alt = keyboard.pressed();
            if let Ok(mut notes) = state.notes.lock() {
                notes.retain(|label, note| {
                    let Some(window) = app.get_webview_window(label) else {
                        return false;
                    };
                    if let Err(error) = apply(&window, note, alt, keyboard.supported()) {
                        log::warn!("[sticky] failed to update input mode: {error}");
                    }
                    true
                });
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    });
}

#[tauri::command]
pub(crate) fn native_sticky_input_mode(window: WebviewWindow) -> Result<InputMode, String> {
    if !window.label().starts_with("sticky-") {
        return Err("仅便签窗口可设置便签输入模式".into());
    }
    apply_layer(
        &window,
        window.is_always_on_top().map_err(|e| e.to_string())?,
    )?;
    let app = window.app_handle();
    let state = app.state::<StickyState>();
    let mut notes = state.notes.lock().map_err(|e| e.to_string())?;
    // The worker applies the current global Alt state on its next tick.
    let note = notes.entry(window.label().to_string()).or_default();
    note.applied = None;
    apply(
        &window,
        note,
        false,
        state.alt_supported.load(Ordering::Relaxed),
    )
}

pub(crate) fn edit_all(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<StickyState>();
    let mut notes = state.notes.lock().map_err(|e| e.to_string())?;
    for (label, window) in app.webview_windows() {
        if !label.starts_with("sticky-") {
            continue;
        }
        // Uninitialized webviews are still waiting for their content. Only
        // the content-ready frontend may reveal them for the first time.
        let Some(note) = notes.get_mut(&label) else {
            continue;
        };
        note.manual_edit = true;
        note.forward_until = None;
        apply(
            &window,
            note,
            false,
            state.alt_supported.load(Ordering::Relaxed),
        )?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn native_edit_sticky_notes(app: AppHandle) -> Result<(), String> {
    edit_all(&app)
}

#[tauri::command]
pub(crate) fn native_sticky_finish_edit(window: WebviewWindow) -> Result<(), String> {
    if !window.label().starts_with("sticky-") {
        return Err("仅便签窗口可结束编辑".into());
    }
    let state = window.app_handle().state::<StickyState>();
    let mut notes = state.notes.lock().map_err(|e| e.to_string())?;
    let note = notes.entry(window.label().to_string()).or_default();
    note.manual_edit = false;
    apply(
        &window,
        note,
        false,
        state.alt_supported.load(Ordering::Relaxed),
    )?;
    Ok(())
}

#[tauri::command]
pub(crate) fn native_sticky_forward_right_click(window: WebviewWindow) -> Result<(), String> {
    if !window.label().starts_with("sticky-") {
        return Err("仅便签窗口可转发右键".into());
    }
    let state = window.app_handle().state::<StickyState>();
    let mut notes = state.notes.lock().map_err(|e| e.to_string())?;
    let note = notes.entry(window.label().to_string()).or_default();
    // contextmenu arrives after button-up. Ignore the note before replaying
    // the click, so the system hit-tests the underlying application instead.
    note.forward_until = Some(Instant::now() + Duration::from_millis(150));
    apply(
        &window,
        note,
        false,
        state.alt_supported.load(Ordering::Relaxed),
    )?;
    drop(notes);
    forward_right_click()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
struct Keyboard;
#[cfg(any(target_os = "windows", target_os = "macos"))]
impl Keyboard {
    fn new() -> Self {
        Self
    }
    fn supported(&self) -> bool {
        true
    }
    fn pressed(&mut self) -> bool {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LMENU};
            // Left Alt only: right Alt/AltGr remains available for text input.
            GetAsyncKeyState(VK_LMENU as i32) < 0
        }
        #[cfg(target_os = "macos")]
        unsafe {
            #[link(name = "CoreGraphics", kind = "framework")]
            extern "C" {
                fn CGEventSourceFlagsState(state: i32) -> u64;
            }
            CGEventSourceFlagsState(0) & 0x00080000 != 0
        }
    }
}

#[cfg(target_os = "windows")]
fn forward_right_click() -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEINPUT,
    };
    let inputs = [MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP].map(|flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err("系统拒绝向下方窗口转发右键".into())
    }
}

#[cfg(target_os = "macos")]
fn forward_right_click() -> Result<(), String> {
    use core_graphics::{
        event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton},
        event_source::{CGEventSource, CGEventSourceStateID},
    };
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| "无法创建鼠标事件源")?;
    let point = CGEvent::new(source.clone())
        .map_err(|_| "无法读取鼠标位置")?
        .location();
    for kind in [CGEventType::RightMouseDown, CGEventType::RightMouseUp] {
        CGEvent::new_mouse_event(source.clone(), kind, point, CGMouseButton::Right)
            .map_err(|_| "无法转发右键")?
            .post(CGEventTapLocation::HID);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
struct Keyboard {
    xlib: Option<x11_dl::xlib::Xlib>,
    display: *mut x11_dl::xlib::Display,
    alt: u8,
}
#[cfg(target_os = "linux")]
impl Keyboard {
    fn new() -> Self {
        let xlib = x11_dl::xlib::Xlib::open().ok();
        // Xwayland cannot observe global keys in native Wayland applications.
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let display = if wayland {
            std::ptr::null_mut()
        } else {
            xlib.as_ref().map_or(std::ptr::null_mut(), |x| unsafe {
                (x.XOpenDisplay)(std::ptr::null())
            })
        };
        let alt = if display.is_null() {
            0
        } else {
            unsafe { (xlib.as_ref().unwrap().XKeysymToKeycode)(display, 0xffe9) }
        };
        Self { xlib, display, alt }
    }
    fn supported(&self) -> bool {
        !self.display.is_null() && self.alt != 0
    }
    fn pressed(&mut self) -> bool {
        if !self.supported() {
            return false;
        }
        let mut keys = [0 as std::os::raw::c_char; 32];
        unsafe {
            (self.xlib.as_ref().unwrap().XQueryKeymap)(self.display, keys.as_mut_ptr());
        }
        keys[(self.alt / 8) as usize] as u8 & (1 << (self.alt % 8)) != 0
    }
}
#[cfg(target_os = "linux")]
impl Drop for Keyboard {
    fn drop(&mut self) {
        if !self.display.is_null() {
            unsafe {
                (self.xlib.as_ref().unwrap().XCloseDisplay)(self.display);
            }
        }
    }
}
#[cfg(target_os = "linux")]
fn forward_right_click() -> Result<(), String> {
    let keyboard = Keyboard::new();
    if !keyboard.supported() {
        return Err("当前桌面环境不支持右键转发，请松开 Alt 后右键操作".into());
    }
    let xtest = x11_dl::xtest::Xf86vmode::open().map_err(|e| e.to_string())?;
    let (down, up) = unsafe {
        let down = (xtest.XTestFakeButtonEvent)(keyboard.display, 3, 1, 0);
        let up = (xtest.XTestFakeButtonEvent)(keyboard.display, 3, 0, 0);
        (keyboard.xlib.as_ref().unwrap().XSync)(keyboard.display, 0);
        (down, up)
    };
    if down != 0 && up != 0 {
        Ok(())
    } else {
        Err("X11 拒绝转发右键".into())
    }
}
