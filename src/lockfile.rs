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
    #[serde(
        default,
        rename = "devDependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(
        default,
        rename = "optionalDependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(
        default,
        rename = "peerDependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_dependencies: BTreeMap<String, String>,
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
            version: None,
            integrity: None,
            resolved: None,
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            optional_dependencies: BTreeMap::new(),
            peer_dependencies: BTreeMap::new(),
        });
        root.version = Some(manifest.version.clone());
        // Persist each root section separately
        root.dependencies = manifest.dependencies.clone();
        root.dev_dependencies = manifest.dev_dependencies.clone();
        root.optional_dependencies = manifest.optional_dependencies.clone();
        root.peer_dependencies = manifest.peer_dependencies.clone();

        // Collect declared root installable packages (exclude peers) into a vector to avoid borrow conflicts
        let declared: Vec<String> = {
            let r = self.packages.get("").expect("root exists");
            r.dependencies
                .keys()
                .chain(r.dev_dependencies.keys())
                .chain(r.optional_dependencies.keys())
                .cloned()
                .collect()
        };
        // Ensure an entry exists for every declared package (dependencies, dev, optional)
        for name in declared {
            let key = format!("node_modules/{}", name);
            self.packages.entry(key).or_insert(PackageEntry {
                version: None,
                integrity: None,
                resolved: None,
                dependencies: BTreeMap::new(),
                dev_dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                peer_dependencies: BTreeMap::new(),
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
            // Try legacy bincode schema
            if let Ok((legacy, _)) = decode_from_slice::<LegacyLockfile, _>(&data, standard()) {
                legacy.into()
            } else if let Ok(txt) = std::str::from_utf8(&data) {
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

// Legacy compat structs for older bincode lockfiles (before dev/optional/peer fields)
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct LegacyPackageEntry {
    pub version: Option<String>,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub resolved: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct LegacyLockfile {
    pub format: u32,
    #[serde(default)]
    pub packages: BTreeMap<String, LegacyPackageEntry>,
}

impl From<LegacyLockfile> for Lockfile {
    fn from(old: LegacyLockfile) -> Self {
        let packages = old
            .packages
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    PackageEntry {
                        version: v.version,
                        integrity: v.integrity,
                        resolved: v.resolved,
                        dependencies: v.dependencies,
                        dev_dependencies: BTreeMap::new(),
                        optional_dependencies: BTreeMap::new(),
                        peer_dependencies: BTreeMap::new(),
                    },
                )
            })
            .collect();
        Lockfile {
            format: old.format,
            packages,
        }
    }
}
