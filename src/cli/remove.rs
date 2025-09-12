use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use crate::colors::*;
use crate::installer::PackageInstance;
use crate::lockfile::{self, Lockfile};
use crate::manifest::{self};

use super::util::{prune_removed_from_lock, prune_unreachable, remove_dirs, lockfile_has_no_packages};

pub fn cmd_remove(packages: Vec<String>) -> Result<()> {
    let start = Instant::now();
    if packages.is_empty() { bail!("no packages specified to remove"); }
    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() { bail!("no package.json found"); }
    let mut manifest = manifest::load(&manifest_path)?;

    let mut actually_removed: Vec<String> = Vec::new();
    for name in &packages {
        if manifest.dependencies.remove(name).is_some() { actually_removed.push(name.clone()); }
        if manifest.dev_dependencies.remove(name).is_some() { if !actually_removed.contains(name) { actually_removed.push(name.clone()); } }
        if manifest.optional_dependencies.remove(name).is_some() { if !actually_removed.contains(name) { actually_removed.push(name.clone()); } }
    }
    if actually_removed.is_empty() {
        println!("{gray}[pacm]{reset} {dim}no matching dependencies to remove{reset}", gray=C_GRAY, dim=C_DIM, reset=C_RESET);
        return Ok(());
    }
    manifest::write(&manifest, &manifest_path)?;

    let lock_path = PathBuf::from("pacm.lockb");
    let mut lock = if lock_path.exists() { lockfile::load(&lock_path)? } else { Lockfile::default() };

    prune_removed_from_lock(&mut lock, &actually_removed);

    let mut roots: VecDeque<String> = VecDeque::new();
    for (n, _) in &manifest.dependencies { roots.push_back(n.clone()); }
    for (n, _) in &manifest.dev_dependencies { roots.push_back(n.clone()); }
    for (n, _) in &manifest.optional_dependencies { roots.push_back(n.clone()); }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();
    while let Some(name) = roots.pop_front() {
        if !seen.insert(name.clone()) { continue; }
        let key = format!("node_modules/{}", name);
        if let Some(entry) = lock.packages.get(&key) {
            let ver = entry.version.clone().unwrap_or_default();
            let deps = entry.dependencies.clone();
            instances.insert(name.clone(), PackageInstance { name: name.clone(), version: ver, dependencies: deps.clone() });
            for (dn, _) in deps { roots.push_back(dn); }
        }
    }

    let trans_removed = prune_unreachable(&mut lock, &instances);
    let mut to_delete = actually_removed.clone();
    to_delete.extend(trans_removed.into_iter());
    if !to_delete.is_empty() { remove_dirs(&to_delete); }

    lockfile::write(&lock, lock_path.clone())?;
    if lockfile_has_no_packages(&lock) { let _ = std::fs::remove_file(&lock_path); }

    super::super::cli::cleanup_empty_node_modules_dir();

    for n in &actually_removed {
        println!("{gray}[pacm]{reset} {red}-{reset} {name}", gray=C_GRAY, red=C_RED, reset=C_RESET, name=n);
    }
    let dur = start.elapsed();
    println!("{gray}[pacm]{reset} summary: {red}{rm} removed{reset} in {secs:.2?}", gray=C_GRAY, red=C_RED, rm=to_delete.len(), secs=dur, reset=C_RESET);
    Ok(())
}
