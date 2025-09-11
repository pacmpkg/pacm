use dirs::data_local_dir;
use std::path::{Path, PathBuf};

pub fn store_root() -> PathBuf {
    let mut root = data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    root.push("pacm");
    root.push("store");
    root.push("v1");
    root
}

pub fn ensure_dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(p)
}

pub fn safe_join(base: &Path, rel: &str) -> Option<PathBuf> {
    if rel.contains("..") {
        return None;
    }
    let mut p = base.to_path_buf();
    p.push(rel);
    Some(p)
}
