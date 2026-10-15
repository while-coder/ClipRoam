use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::platforms::AUTOSTART_APP_IDENTIFIER as APP_IDENTIFIER;

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "无法获取 HOME 目录".to_string())
}

fn plist_path() -> Result<PathBuf, String> {
    Ok(home_dir()?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{APP_IDENTIFIER}.plist")))
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn plist_content(exe: &Path) -> String {
    let exe = escape_xml(&exe.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{APP_IDENTIFIER}</string>
  <key>ProgramArguments</key>
  <array>
<string>{exe}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#
    )
}

pub(crate) fn autostart_is_enabled() -> Result<bool, String> {
    Ok(plist_path()?.is_file())
}

pub(crate) fn autostart_set_enabled(enabled: bool) -> Result<(), String> {
    let path = plist_path()?;
    if enabled {
        let exe = env::current_exe().map_err(|e| format!("获取当前程序路径失败: {e}"))?;
        let parent = path
            .parent()
            .ok_or_else(|| "开机启动配置路径无效".to_string())?;
        fs::create_dir_all(parent).map_err(|e| format!("创建 LaunchAgents 目录失败: {e}"))?;
        fs::write(&path, plist_content(&exe)).map_err(|e| format!("写入 LaunchAgent 失败: {e}"))?;
    } else if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("删除 LaunchAgent 失败: {e}"))?;
    }
    Ok(())
}
