use std::{env, mem, slice};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::{
        RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ,
    },
};

use crate::platforms::AUTOSTART_APP_IDENTIFIER as APP_IDENTIFIER;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn as_bytes(value: &[u16]) -> &[u8] {
    unsafe { slice::from_raw_parts(value.as_ptr().cast(), mem::size_of_val(value)) }
}

fn win_err(action: &str, code: windows_sys::Win32::Foundation::WIN32_ERROR) -> String {
    format!("{action}失败: Windows error {}", code)
}

fn run_command() -> Result<String, String> {
    let exe = env::current_exe().map_err(|e| format!("获取当前程序路径失败: {e}"))?;
    Ok(format!("\"{}\"", exe.display()))
}

fn open_run_key_for_read() -> Result<Option<RegKey>, String> {
    let subkey = to_wide(RUN_KEY);
    let mut key = std::ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut key) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(win_err("打开开机启动注册表", status));
    }
    Ok(Some(RegKey(key)))
}

fn open_or_create_run_key_for_write() -> Result<RegKey, String> {
    let subkey = to_wide(RUN_KEY);
    let mut key = std::ptr::null_mut();
    let status = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, subkey.as_ptr(), &mut key) };
    if status != ERROR_SUCCESS {
        return Err(win_err("创建开机启动注册表", status));
    }
    Ok(RegKey(key))
}

pub(crate) fn autostart_is_enabled() -> Result<bool, String> {
    let Some(key) = open_run_key_for_read()? else {
        return Ok(false);
    };
    let name = to_wide(APP_IDENTIFIER);
    let mut size = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if status != ERROR_SUCCESS {
        return Err(win_err("读取开机启动状态", status));
    }
    Ok(size > 0)
}

pub(crate) fn autostart_set_enabled(enabled: bool) -> Result<(), String> {
    let key = open_or_create_run_key_for_write()?;
    let name = to_wide(APP_IDENTIFIER);
    let status = if enabled {
        let command = to_wide(&run_command()?);
        unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_SZ,
                as_bytes(&command).as_ptr(),
                mem::size_of_val(command.as_slice()) as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(key.0, name.as_ptr()) }
    };

    if !enabled && status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if status != ERROR_SUCCESS {
        return Err(win_err("更新开机启动状态", status));
    }
    Ok(())
}
