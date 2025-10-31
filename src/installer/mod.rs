use crate::cache::StoreEntry;
use crate::lockfile::Lockfile;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Link,
    Copy,
}

#[derive(Debug, Clone)]
pub struct InstallPlanEntry {
    pub package: PackageInstance,
    pub store_entry: StoreEntry,
}

#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub package_name: String,
    pub link_mode: InstallMode,
}

#[derive(Debug)]
pub struct Installer {
    mode: InstallMode,
}

impl Installer {
    pub fn new(mode: InstallMode) -> Self {
        Self { mode }
    }

    pub fn install(
        &self,
        project_root: &Path,
        plan: &HashMap<String, InstallPlanEntry>,
        lock: &mut Lockfile,
    ) -> Result<Vec<InstallOutcome>> {
        let node_modules = project_root.join("node_modules");
        fs::create_dir_all(&node_modules)?;
        let mut names: Vec<String> = plan.keys().cloned().collect();
        names.sort();

        let install_results: Result<Vec<(String, InstallMode)>> = names
            .par_iter()
            .map(|name| -> Result<(String, InstallMode)> {
                let entry =
                    plan.get(name).expect("plan entries should remain stable across iteration");
                let mut dest = node_modules.clone();
                for part in entry.package.name.split('/') {
                    dest.push(part);
                }
                let outcome_mode = self
                    .materialize(&entry.store_entry, &dest)
                    .with_context(|| format!("materialize {} into project", entry.package.name))?;

                create_bin_shims(project_root, &entry.package.name, &dest)
                    .with_context(|| format!("create bin shims for {}", entry.package.name))?;

                Ok((entry.package.name.clone(), outcome_mode))
            })
            .collect();

        let install_results = install_results?;
        let mut outcomes = Vec::with_capacity(install_results.len());
        for (package_name, outcome_mode) in install_results {
            if let Some(entry) = plan.get(&package_name) {
                if let Some(lock_entry) =
                    lock.packages.get_mut(&format!("node_modules/{}", entry.package.name))
                {
                    lock_entry.store_key = Some(entry.store_entry.store_key.clone());
                    lock_entry.content_hash = Some(entry.store_entry.content_hash.clone());
                    lock_entry.link_mode = Some(match outcome_mode {
                        InstallMode::Link => "link".to_string(),
                        InstallMode::Copy => "copy".to_string(),
                    });
                    lock_entry.store_path = Some(entry.store_entry.root_dir.display().to_string());
                }
                outcomes.push(InstallOutcome {
                    package_name: entry.package.name.clone(),
                    link_mode: outcome_mode,
                });
            }
        }
        Ok(outcomes)
    }

    fn materialize(&self, store_entry: &StoreEntry, dest: &Path) -> Result<InstallMode> {
        if dest.exists() || std::fs::symlink_metadata(dest).is_ok() {
            fs::remove_dir_all(dest).or_else(|_| {
                if dest.is_file() {
                    fs::remove_file(dest)
                } else {
                    Err(std::io::Error::other("failed to remove existing destination"))
                }
            })?;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        match self.mode {
            InstallMode::Copy => {
                copy_tree_only(store_entry.package_dir(), dest)?;
                Ok(InstallMode::Copy)
            }
            InstallMode::Link => {
                let linked = link_or_copy_tree(store_entry.package_dir(), dest)?;
                if linked {
                    Ok(InstallMode::Link)
                } else {
                    Ok(InstallMode::Copy)
                }
            }
        }
    }
}

impl Default for Installer {
    fn default() -> Self {
        Self::new(InstallMode::Copy)
    }
}

fn copy_tree_only(from: &Path, to: &Path) -> Result<()> {
    for entry in WalkDir::new(from).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(from)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let dest = to.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(entry.path(), &dest)?;
        let perms = entry.metadata()?.permissions();
        fs::set_permissions(&dest, perms)?;
    }
    Ok(())
}

fn link_or_copy_tree(from: &Path, to: &Path) -> Result<bool> {
    let mut all_linked = true;
    for entry in WalkDir::new(from).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(from)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let dest = to.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::hard_link(entry.path(), &dest) {
            Ok(_) => {}
            Err(_) => {
                fs::copy(entry.path(), &dest)?;
                all_linked = false;
            }
        }
        let perms = entry.metadata()?.permissions();
        fs::set_permissions(&dest, perms)?;
    }
    Ok(all_linked)
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
    // Try to copy the packaged pacm-shim binary next to pacm and append marker with relative path.
    // If the binary isn't available (e.g., CI/coverage builds), fall back to writing a small
    // portable shell wrapper that executes node on the relative target path.
    if let Ok(shim_bin) = locate_unix_pacm_shim() {
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
        return Ok(());
    }

    // Fallback: write a simple shell wrapper that invokes node on the relative target.
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    use std::io::Write;
    let mut f = std::fs::File::create(dest)?;
    // The wrapper resolves the script path relative to the .bin dir using $0.
    // Use a POSIX-compatible sh wrapper which calls node.
    let rel = relative_target.to_string_lossy();
    let script = format!(
        "#!/usr/bin/env sh\n"#
        "basedir=$(dirname \"$0\")\n"#
        "node \"$basedir/{rel}\" \"$@\"\n",
        rel = rel
    );
    f.write_all(script.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dest)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(dest, perms)?;
    }
    Ok(())
}

#[cfg(unix)]
fn locate_unix_pacm_shim() -> Result<PathBuf> {
    fn find_candidate(dir: &Path) -> Option<PathBuf> {
        for name in ["pacm-shim", "pacm_shim"] {
            let direct = dir.join(name);
            if direct.is_file() {
                return Some(direct);
            }
        }

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if (name.starts_with("pacm-shim") || name.starts_with("pacm_shim"))
                    && !name.ends_with(".d")
                    && !name.ends_with(".rlib")
                    && !name.ends_with(".rmeta")
                {
                    return Some(path);
                }
            }
        }
        None
    }

    let current = std::env::current_exe().with_context(|| "locate pacm executable")?;
    let exe_dir = current.parent().with_context(|| "determine pacm executable directory")?;

    if let Some(candidate) = find_candidate(exe_dir) {
        return Ok(candidate);
    }

    for ancestor in exe_dir.ancestors().skip(1) {
        if let Some(candidate) = find_candidate(ancestor) {
            return Ok(candidate);
        }
    }

    anyhow::bail!("pacm-shim binary not found near {}", exe_dir.display());
}
