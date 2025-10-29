use crate::colors::*;
use crate::manifest::{self, Manifest};
use anyhow::{bail, Result};
use std::path::PathBuf;

pub fn cmd_init(name: Option<String>, version: Option<String>) -> Result<()> {
    let path = PathBuf::from("package.json");
    if path.exists() {
        bail!("package.json already exists");
    }
    let manifest = Manifest::new(
        name.unwrap_or_else(|| "my-app".into()),
        version.unwrap_or_else(|| "0.1.0".into()),
    );
    manifest::write(&manifest, &path)?;
    println!(
        "{gray}[pacm]{reset} {green}init{reset} created {name}@{ver}",
        gray = C_GRAY,
        reset = C_RESET,
        green = C_GREEN,
        name = manifest.name,
        ver = manifest.version
    );
    Ok(())
}
