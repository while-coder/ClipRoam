//! session_token 的持久化：优先系统凭据库，不可用则回落配置文件。
//!
//! sync-config.json 的其余字段是低敏感元数据，保持明文便于排障；只有
//! token 挪进钥匙串（Windows 凭据管理器 / macOS·iOS 钥匙串 / Linux
//! Secret Service）。Android 无 keyring 支持且应用沙箱已隔离数据目录，
//! 直接回落文件存储——调用方按返回值决定文件里是否保留 token。

/// token 写入系统凭据库；空 token 表示登出，顺带清掉凭据库条目。
/// 返回 false 表示凭据库不可用（如无 secret service 的 Linux），调用方
/// 应把 token 留在配置文件里（行为与回落存储一致）。
pub(crate) fn store_session_token(token: &str) -> Result<bool, String> {
    imp::store(token)
}

/// 从系统凭据库读取 token；不存在返回 `None`，访问失败保留错误供用户重试。
pub(crate) fn load_session_token() -> Result<Option<String>, String> {
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
    use keyring::{Entry, Error};

    const SERVICE: &str = "com.whilecode.cliproam";
    const USER: &str = "sync-session-token";

    fn entry() -> Result<Entry, String> {
        Entry::new(SERVICE, USER).map_err(|error| format!("无法访问系统凭据库：{error}"))
    }

    pub(super) fn store(token: &str) -> Result<bool, String> {
        if token.is_empty() {
            delete(&entry()?)?;
            Ok(true)
        } else {
            // 仍保留凭据库不可用时的文件回落；只在成功写入后剥离文件 token。
            Ok(entry().is_ok_and(|entry| entry.set_password(token).is_ok()))
        }
    }

    fn delete(entry: &Entry) -> Result<(), String> {
        match entry.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(error) => Err(format!(
                "无法清除系统保存的登录凭证：{error}。请允许访问凭据库后重试退出登录。"
            )),
        }
    }

    fn read(entry: &Entry) -> Result<Option<String>, String> {
        match entry.get_password() {
            Ok(token) => Ok((!token.is_empty()).then_some(token)),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(format!(
                "无法读取系统保存的登录凭证：{error}。请允许访问凭据库或解锁钥匙串后重试。"
            )),
        }
    }

    pub(super) fn load() -> Result<Option<String>, String> {
        read(&entry()?)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use keyring::mock::MockCredential;

        #[test]
        fn denied_deletion_is_not_reported_as_logout_success() {
            let entry = Entry::new_with_credential(Box::new(MockCredential::default()));
            entry.set_password("old-session").unwrap();
            entry
                .get_credential()
                .downcast_ref::<MockCredential>()
                .unwrap()
                .set_error(Error::Invalid("access".into(), "denied".into()));
            assert!(delete(&entry).is_err());
            assert_eq!(read(&entry).unwrap().as_deref(), Some("old-session"));
            delete(&entry).unwrap();
            assert!(read(&entry).unwrap().is_none());
            delete(&entry).unwrap();
        }

        #[test]
        fn denied_read_can_be_retried_after_authorization() {
            let entry = Entry::new_with_credential(Box::new(MockCredential::default()));
            entry.set_password("saved-session").unwrap();
            entry
                .get_credential()
                .downcast_ref::<MockCredential>()
                .unwrap()
                .set_error(Error::Invalid("access".into(), "denied".into()));
            assert!(read(&entry).is_err());
            assert_eq!(read(&entry).unwrap().as_deref(), Some("saved-session"));
        }
    }
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "linux"
)))]
mod imp {
    pub(super) fn store(token: &str) -> Result<bool, String> {
        Ok(token.is_empty())
    }

    pub(super) fn load() -> Result<Option<String>, String> {
        Ok(None)
    }
}
