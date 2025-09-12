use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct PackageInstance {
    pub name: String,
    pub version: String,
    /// dependency name -> range (for now) – we resolve to concrete version via instance map later
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct Linker {
    pub virtual_dir_name: String, // e.g. ".pacm"
}

impl Linker {
    pub fn new() -> Self {
        Self {
            virtual_dir_name: ".pacm".into(),
        }
    }

    pub fn link_project(
        &self,
        project_root: &Path,
        instances: &HashMap<String, PackageInstance>,
        _root_deps: &BTreeMap<String, String>,
    ) -> Result<()> {
        let node_modules = project_root.join("node_modules");
        fs::create_dir_all(&node_modules)?;

        // Install all packages flat into node_modules by copying from global cache
        for inst in instances.values() {
            let cache_pkg_dir = crate::store::cache_package_path(&inst.name, &inst.version);
            let mut dest = node_modules.clone();
            for part in inst.name.split('/') {
                dest = dest.join(part);
            }
            if dest.exists() {
                continue;
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            materialize_package_dir(&cache_pkg_dir, &dest)?;
        }

        Ok(())
    }
}


pub fn copy_dir_recursive(from: &Path, to: &Path) -> Result<()> {
    if !to.exists() {
        fs::create_dir_all(to)?;
    }
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let p = entry.path();
        let meta = entry.metadata()?;
        let dst = to.join(entry.file_name());
        if meta.is_dir() {
            copy_dir_recursive(&p, &dst)?;
        } else {
            // Prefer hard links to keep content de-duplicated and paths local to project
            if let Err(_e) = std::fs::hard_link(&p, &dst) {
                std::fs::copy(&p, &dst)?;
            }
        }
    }
    Ok(())
}

fn materialize_package_dir(from: &Path, to: &Path) -> Result<()> {
    if to.exists() || std::fs::symlink_metadata(to).is_ok() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    copy_dir_recursive(from, to)
}
