use crate::colors::*;
use crate::fsutil;
use anyhow::Result;
use std::fs;

pub fn cmd_cache_path() -> Result<()> {
    let path = fsutil::cache_root();
    println!(
        "{gray}[pacm]{reset} cache: {path}",
        gray = C_GRAY,
        reset = C_RESET,
        path = path.display()
    );
    Ok(())
}

pub fn cmd_cache_clean() -> Result<()> {
    let root = fsutil::cache_root();
    if root.exists() {
        fs::remove_dir_all(&root).ok();
    }
    fs::create_dir_all(&root)?;
    println!(
        "{gray}[pacm]{reset} {green}cache cleaned{reset} at {path}",
        gray = C_GRAY,
        green = C_GREEN,
        reset = C_RESET,
        path = root.display()
    );
    Ok(())
}
