pub(crate) fn is_chinese() -> bool {
    tauri_plugin_os::locale()
        .map(|locale| {
            locale
                .split(['-', '_', '.'])
                .next()
                .unwrap_or("")
                .eq_ignore_ascii_case("zh")
        })
        .unwrap_or(false)
}

pub(crate) fn app_name() -> &'static str {
    if is_chinese() {
        "随心贴"
    } else {
        "ClipRoam"
    }
}

pub(crate) fn quick_paste_title() -> &'static str {
    if is_chinese() {
        "随心贴 - 快速粘贴"
    } else {
        "ClipRoam - Quick Paste"
    }
}
