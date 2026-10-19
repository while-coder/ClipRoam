//! iOS 平台实现。
pub(crate) use super::{autostart_is_enabled, autostart_set_enabled};
pub(crate) use super::{
    open_data_directory, read_clipboard_files, read_clipboard_image, read_clipboard_text,
    show_notification, write_clipboard_files, write_clipboard_image, write_clipboard_text,
};

mod files;
pub(crate) use super::{
    begin_window_drag, capture_paste_target, create_windows, deliver_paste, manage_platform_state,
    on_paste_window_focus, on_window_event, register_plugins, should_skip_clipboard_poll,
};
pub(crate) use files::{pick_files, save_dialog};

pub(crate) use crate::platforms::files::{copy_file, create_directory, remove_file};
