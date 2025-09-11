use crate::error::Result;
use crate::manifest::Manifest;
use bincode::config::standard;
use bincode::serde::{decode_from_slice, encode_to_vec};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PackageEntry {
    pub version: Option<String>,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub resolved: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub format: u32,
    #[serde(default)]
    pub packages: BTreeMap<String, PackageEntry>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            format: 1,
            packages: BTreeMap::new(),
        }
    }
}

impl Lockfile {
    pub fn load_or_default(path: PathBuf) -> Result<Self> {
        if path.exists() {
            load(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn sync_from_manifest(&mut self, manifest: &Manifest) {
        let root = self.packages.entry("".into()).or_insert(PackageEntry {
            version: Some(manifest.version.clone()),
            integrity: None,
            resolved: None,
            dependencies: BTreeMap::new(),
        });
        root.version = Some(manifest.version.clone());
        // Merge all dependency sections into the lockfile root dependencies so dev/optional are recorded
        let mut merged: BTreeMap<String, String> = BTreeMap::new();
        for (n, r) in &manifest.dependencies {
            merged.insert(n.clone(), r.clone());
        }
        for (n, r) in &manifest.dev_dependencies {
            merged.insert(n.clone(), r.clone());
        }
        for (n, r) in &manifest.optional_dependencies {
            merged.insert(n.clone(), r.clone());
        }
        root.dependencies = merged.clone();

        // Ensure an entry exists for every declared package (including scoped names)
        for (name, _range) in merged {
            let key = format!("node_modules/{}", name);
            self.packages.entry(key).or_insert(PackageEntry {
                version: None,
                integrity: None,
                resolved: None,
                dependencies: BTreeMap::new(),
            });
        }
    }
}

pub fn load(path: &PathBuf) -> Result<Lockfile> {
    let data = fs::read(path)?;
    // Try bincode first; if that fails and looks like JSON, fallback
    let lf: Lockfile = match decode_from_slice::<Lockfile, _>(&data, standard()) {
        Ok((v, _)) => v,
        Err(_) => {
            if let Ok(txt) = std::str::from_utf8(&data) {
                let trimmed = txt.trim_start();
                if trimmed.starts_with('{') || trimmed.starts_with('[') {
                    serde_json::from_str(trimmed)?
                } else {
                    anyhow::bail!("unsupported lockfile format")
                }
            } else {
                anyhow::bail!("unsupported lockfile format")
            }
        }
    };
    if lf.format == 0 {
        anyhow::bail!("invalid lockfile format");
    }
    Ok(lf)
}

pub fn write(lf: &Lockfile, path: PathBuf) -> Result<()> {
    let data = encode_to_vec(lf, standard())?;
    fs::write(path, data)?;
    Ok(())
}

/// Load a legacy JSON lockfile directly (compat migration helper)
pub fn load_json_compat(path: &PathBuf) -> Result<Lockfile> {
    let txt = fs::read_to_string(path)?;
    let lf: Lockfile = serde_json::from_str(&txt)?;
    if lf.format == 0 {
        anyhow::bail!("invalid lockfile format");
    }
    Ok(lf)
}
