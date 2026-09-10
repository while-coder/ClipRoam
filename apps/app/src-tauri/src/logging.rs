//! 日志插件配置。

/// 构建日志插件：
/// - 三路输出：stdout（dev 控制台）、webview（前端 devtools console）、文件（系统日志目录）；
/// - dev 记到 Debug，release 记到 Info，并压低三方库噪声；
/// - 本地时区时间戳，单文件超 10 MB 轮转，保留最近 3 份历史归档（外加当前活动文件，最多 4 个），避免无限增长。
///   日志文件位置（Windows）：%LOCALAPPDATA%/<bundle-id>/logs/。
pub(crate) fn logging_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

    let level = if cfg!(debug_assertions) {
        tauri_plugin_log::log::LevelFilter::Debug
    } else {
        tauri_plugin_log::log::LevelFilter::Info
    };

    tauri_plugin_log::Builder::new()
        .level(level)
        // 三方库默认只在出问题时才需要，压到 Warn 减少噪声
        .level_for("tao", tauri_plugin_log::log::LevelFilter::Warn)
        .level_for("reqwest", tauri_plugin_log::log::LevelFilter::Warn)
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::Webview),
            Target::new(TargetKind::LogDir { file_name: None }),
        ])
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .max_file_size(10_000_000)
        .rotation_strategy(RotationStrategy::KeepSome(3))
        .build()
}
