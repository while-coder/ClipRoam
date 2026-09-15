//! session_token 的持久化：优先系统凭据库，不可用则回落配置文件。
//!
//! sync-config.json 的其余字段是低敏感元数据，保持明文便于排障；只有
//! token 挪进钥匙串（Windows 凭据管理器 / macOS·iOS 钥匙串 / Linux
//! Secret Service）。Android 无 keyring 支持且应用沙箱已隔离数据目录，
//! 直接回落文件存储——调用方按返回值决定文件里是否保留 token。

/// token 写入系统凭据库；空 token 表示登出，顺带清掉凭据库条目。
/// 返回 false 表示凭据库不可用（如无 secret service 的 Linux），调用方
/// 应把 token 留在配置文件里（行为与回落存储一致）。
pub(crate) fn store_session_token(token: &str) -> bool {
    imp::store(token)
}

/// 从系统凭据库读取 token；不存在或凭据库不可用时返回 `None`。
pub(crate) fn load_session_token() -> Option<String> {
    imp::load()
}

// keyring 依赖的目标平台清单，须与 Cargo.toml 里 keyring 的目标段一致。
#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "linux"
))]
mod imp {
    use keyring::Entry;

    const SERVICE: &str = "com.whilecode.cliproam";
    const USER: &str = "sync-session-token";

    fn entry() -> Option<Entry> {
        Entry::new(SERVICE, USER).ok()
    }

    pub(super) fn store(token: &str) -> bool {
        let Some(entry) = entry() else {
            return false;
        };
        if token.is_empty() {
            // 登出清凭证：条目本就不存在时报错无碍，同样视为已清空。
            let _ = entry.delete_credential();
            true
        } else {
            entry.set_password(token).is_ok()
        }
    }

    pub(super) fn load() -> Option<String> {
        entry()?
            .get_password()
            .ok()
            .filter(|token| !token.is_empty())
    }
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "linux"
)))]
mod imp {
    pub(super) fn store(token: &str) -> bool {
        token.is_empty()
    }

    pub(super) fn load() -> Option<String> {
        None
    }
}
