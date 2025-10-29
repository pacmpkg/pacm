use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub peer_dependencies: BTreeMap<String, String>,
}

impl Manifest {
    pub fn new(name: String, version: String) -> Self {
        Self {
            name,
            version,
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            optional_dependencies: BTreeMap::new(),
            peer_dependencies: BTreeMap::new(),
        }
    }
}

pub fn load(path: &Path) -> Result<Manifest> {
    let data = fs::read_to_string(path)?;
    let m: Manifest = serde_json::from_str(&data)?;
    if m.name.is_empty() {
        anyhow::bail!("name empty");
    }
    Ok(m)
}

pub fn write(manifest: &Manifest, path: &Path) -> Result<()> {
    let data = serde_json::to_string_pretty(manifest)?;
    fs::write(path, data)?;
    Ok(())
}
