use std::{env, fs, path::PathBuf};

use crate::platforms::AUTOSTART_APP_IDENTIFIER as APP_IDENTIFIER;

fn config_home() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".config"))
        .ok_or_else(|| "无法获取 HOME 目录".to_string())
}

fn desktop_path() -> Result<PathBuf, String> {
    Ok(config_home()?
        .join("autostart")
        .join(format!("{APP_IDENTIFIER}.desktop")))
}

fn escape_desktop_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
}

fn desktop_content() -> Result<String, String> {
    let exe = env::current_exe().map_err(|e| format!("获取当前程序路径失败: {e}"))?;
    let exe = escape_desktop_value(&exe.display().to_string());
    Ok(format!(
        r#"[Desktop Entry]
Type=Application
Name=ClipRoam
Exec="{exe}"
Terminal=false
X-GNOME-Autostart-enabled=true
"#
    ))
}

pub(crate) fn autostart_is_enabled() -> Result<bool, String> {
    Ok(desktop_path()?.is_file())
}

pub(crate) fn autostart_set_enabled(enabled: bool) -> Result<(), String> {
    let path = desktop_path()?;
    if enabled {
        let parent = path
            .parent()
            .ok_or_else(|| "开机启动配置路径无效".to_string())?;
        fs::create_dir_all(parent).map_err(|e| format!("创建 autostart 目录失败: {e}"))?;
        fs::write(&path, desktop_content()?)
            .map_err(|e| format!("写入 autostart desktop 文件失败: {e}"))?;
    } else if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("删除 autostart desktop 文件失败: {e}"))?;
    }
    Ok(())
}
