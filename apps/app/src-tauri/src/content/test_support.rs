use std::path::PathBuf;

pub(crate) struct TemporaryDirectory(pub PathBuf);

impl TemporaryDirectory {
    pub(crate) fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("cliproam-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
