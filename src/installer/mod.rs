use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PackageInstance {
    pub name: String,
    pub version: String,
    pub dependencies: BTreeMap<String, String>,
    #[allow(dead_code)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[allow(dead_code)]
    pub peer_dependencies: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct Installer;

impl Installer {
    pub fn new() -> Self {
        Self
    }

    pub fn install(
        &self,
        project_root: &Path,
        instances: &HashMap<String, PackageInstance>,
        _root_deps: &BTreeMap<String, String>,
    ) -> Result<()> {
        let node_modules = project_root.join("node_modules");
        fs::create_dir_all(&node_modules)?;
        for inst in instances.values() {
            let cache_pkg_dir = crate::cache::cache_package_path(&inst.name, &inst.version);
            let mut dest = node_modules.clone();
            for part in inst.name.split('/') {
                dest = dest.join(part);
            }
            if dest.exists() {
                // Even if package dir exists, ensure bins are created
            } else {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                materialize_package_dir(&cache_pkg_dir, &dest)?;
            }
            // After materializing, create .bin shims if package has bin entries
            create_bin_shims(project_root, &inst.name, &dest)
                .with_context(|| format!("create bin shims for {}", inst.name))?;
        }
        Ok(())
    }
}

impl Default for Installer {
    fn default() -> Self {
        Self::new()
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
        } else if let Err(_e) = std::fs::hard_link(&p, &dst) {
            std::fs::copy(&p, &dst)?;
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

fn create_bin_shims(project_root: &Path, package_name: &str, pkg_dest_dir: &Path) -> Result<()> {
    // Read cached manifest to get bin entries
    // Determine version by reading the installed package.json to avoid relying on the caller
    let manifest_path = pkg_dest_dir.join("package.json");
    if !manifest_path.exists() {
        return Ok(());
    }
    let txt = fs::read_to_string(&manifest_path)?;
    #[derive(serde::Deserialize)]
    struct LocalMf {
        name: Option<String>,
        #[serde(default)]
        bin: Option<crate::cache::BinField>,
    }
    let mf: LocalMf =
        serde_json::from_str(&txt).with_context(|| "parse package.json for bin field")?;
    let bin_field = match mf.bin {
        None => return Ok(()),
        Some(b) => b,
    };
    let nm_dir = project_root.join("node_modules");
    let bin_dir = nm_dir.join(".bin");
    fs::create_dir_all(&bin_dir)?;
    // Build mapping name -> relative js path (within package)
    let entries: Vec<(String, String)> = match bin_field {
        crate::cache::BinField::Single(path) => {
            let name = mf.name.clone().unwrap_or_else(|| package_name.to_string());
            vec![(name, path)]
        }
        crate::cache::BinField::Map(map) => map.into_iter().collect(),
    };
    for (mut bin_name, rel_path) in entries {
        if let Some(idx) = bin_name.rfind('/') {
            bin_name = bin_name[(idx + 1)..].to_string();
        }
        // Absolute JS target path (under node_modules/<pkg>/...)
        let target_js_abs = normalize_pkg_path(pkg_dest_dir, &rel_path);
        if !target_js_abs.exists() {
            continue;
        }
        // Build relative JS path from .bin directory: ../<pkg>/<rel_path>
        let mut rel_from_bin = PathBuf::from("..");
        for part in package_name.split('/') {
            rel_from_bin = rel_from_bin.join(part);
        }
        for part in rel_path.split('/') {
            if part == "." || part.is_empty() {
                continue;
            } else if part == ".." {
                rel_from_bin.pop();
            } else {
                rel_from_bin = rel_from_bin.join(part);
            }
        }

        #[cfg(windows)]
        {
            // Only create .exe and .exe.shim on Windows
            let exe_path = bin_dir.join(format!("{bin_name}.exe"));
            write_windows_exe_shim(&exe_path, &rel_from_bin)?;
        }
        #[cfg(unix)]
        {
            let dest = bin_dir.join(&bin_name);
            write_unix_native_shim(&dest, &rel_from_bin)?;
        }
    }
    Ok(())
}

fn normalize_pkg_path(base: &Path, rel: &str) -> PathBuf {
    let mut p = PathBuf::from(base);
    for part in rel.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            p.pop();
        } else {
            p.push(part);
        }
    }
    p
}

#[cfg(windows)]
fn write_windows_exe_shim(dest_exe: &Path, relative_target: &Path) -> Result<()> {
    // Copy current pacm.exe as a generic shim and write a sidecar with target path.
    let pacm_exe = std::env::current_exe().with_context(|| "locate pacm executable")?;
    if let Some(parent) = dest_exe.parent() {
        fs::create_dir_all(parent)?;
    }
    if dest_exe.exists() {
        let _ = fs::remove_file(dest_exe);
    }
    fs::copy(&pacm_exe, dest_exe)
        .with_context(|| format!("copy pacm exe to {}", dest_exe.display()))?;
    let sidecar = PathBuf::from(format!("{}.shim", dest_exe.to_string_lossy()));
    fs::write(sidecar, relative_target.to_string_lossy().as_ref())?;
    Ok(())
}

#[cfg(unix)]
fn write_unix_native_shim(dest: &Path, relative_target: &Path) -> Result<()> {
    // Copy the pacm-shim binary next to pacm and append marker with relative path
    let mut shim_bin = std::env::current_exe().with_context(|| "locate pacm executable")?;
    shim_bin.set_file_name("pacm-shim");
    if !shim_bin.exists() {
        // Fallback: try alongside in release/debug target structure
        // If not found, error out with context
        anyhow::bail!("pacm-shim binary not found at {}", shim_bin.display());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    fs::copy(&shim_bin, dest)?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(dest)?;
    write!(f, "\nPACM_SHIM:{}\n", relative_target.to_string_lossy())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dest)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(dest, perms)?;
    }
    Ok(())
}
