//! Shared encrypted session files. Platforms only supply their files directory.
//! The fixed key prevents plaintext storage; it is extractable from the binary.
use super::{atomic, io, Result};
use aes_gcm::{
    aead::{Aead, AeadCore, OsRng},
    Aes256Gcm, KeyInit, Nonce,
};
use serde_json::Value;
use std::{fs, io::ErrorKind, path::Path};

const KEY: &[u8; 32] = b"cliproam-share-auth-key-v1-00001";
const HEADER: &[u8] = b"CRSA\x01";

pub fn access(files: &Path, id: &str, value: Option<Option<&Value>>) -> Result<Option<Value>> {
    if id != "current"
        && (id.len() != 36 || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'))
    {
        return Err("分享标识无效".into());
    }
    let directory = files.join("share-auth");
    let path = directory.join(id);
    match value {
        Some(Some(value)) => {
            fs::create_dir_all(directory).map_err(io)?;
            let cipher = Aes256Gcm::new_from_slice(KEY).map_err(|_| "分享配置密钥无效")?;
            let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
            let plaintext = serde_json::to_vec(value).map_err(io)?;
            let ciphertext = cipher
                .encrypt(&nonce, plaintext.as_slice())
                .map_err(|_| "分享配置加密失败")?;
            let mut bytes = HEADER.to_vec();
            bytes.extend_from_slice(&nonce);
            bytes.extend_from_slice(&ciphertext);
            atomic(&path, &bytes)?;
            Ok(None)
        }
        Some(None) => match fs::remove_file(path) {
            Ok(()) => Ok(None),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io(error)),
        },
        None => read_file(&path),
    }
}

/// Read cached credentials supplied as an encrypted file path to start_share.
pub fn read_file(path: &Path) -> Result<Option<Value>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(error)),
    };
    if !bytes.starts_with(HEADER) || bytes.len() < HEADER.len() + 12 + 16 {
        return Err("分享配置格式已变更或损坏，请打开 ClipRoam 重新登录".into());
    }
    let cipher = Aes256Gcm::new_from_slice(KEY).map_err(|_| "分享配置密钥无效")?;
    let nonce = Nonce::from_slice(&bytes[HEADER.len()..HEADER.len() + 12]);
    let plaintext = cipher
        .decrypt(nonce, &bytes[HEADER.len() + 12..])
        .map_err(|_| "分享配置无法解密，请打开 ClipRoam 重新登录")?;
    serde_json::from_slice(&plaintext).map(Some).map_err(io)
}
