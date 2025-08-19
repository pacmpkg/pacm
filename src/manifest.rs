use serde::{Serialize, Deserialize};
use std::{collections::BTreeMap, path::Path, fs};
use crate::error::Result;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

impl Manifest {
    pub fn new(name: String, version: String) -> Self { Self { name, version, dependencies: BTreeMap::new() } }
}

pub fn load(path: &Path) -> Result<Manifest> {
    let data = fs::read_to_string(path)?;
    let m: Manifest = serde_json::from_str(&data)?;
    if m.name.is_empty() { anyhow::bail!("name empty"); }
    Ok(m)
}

pub fn write(manifest: &Manifest, path: &Path) -> Result<()> {
    let data = serde_json::to_string_pretty(manifest)?;
    fs::write(path, data)?;
    Ok(())
}
