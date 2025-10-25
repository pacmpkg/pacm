use crate::cli::commands::install::{
    cleanup_empty_node_modules_dir, lockfile_has_no_packages, prune_removed_from_lock,
    prune_unreachable, remove_dirs,
};
use crate::colors::*;
use crate::lockfile::{self, Lockfile};
use crate::manifest;
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::time::Instant;

pub fn cmd_remove(packages: Vec<String>) -> Result<()> {
    let start = Instant::now();
    if packages.is_empty() {
        bail!("no packages specified to remove");
    }

    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() {
        bail!("no package.json found");
    }
    let mut manifest = manifest::load(&manifest_path)?;

    let mut actually_removed = Vec::new();
    for name in &packages {
        if manifest.dependencies.remove(name).is_some()
            || manifest.dev_dependencies.remove(name).is_some()
            || manifest.optional_dependencies.remove(name).is_some()
        {
            if !actually_removed.contains(name) {
                actually_removed.push(name.clone());
            }
        }
    }

    if actually_removed.is_empty() {
        println!(
            "{gray}[pacm]{reset} {dim}no matching dependencies to remove{reset}",
            gray = C_GRAY,
            dim = C_DIM,
            reset = C_RESET
        );
        return Ok(());
    }

    manifest::write(&manifest, &manifest_path)?;

    let lock_path = PathBuf::from("pacm.lockb");
    let mut lock = if lock_path.exists() {
        lockfile::load(&lock_path)?
    } else {
        Lockfile::default()
    };

    prune_removed_from_lock(&mut lock, &actually_removed);
    let trans_removed = prune_unreachable(&mut lock);
    let mut to_delete = actually_removed.clone();
    to_delete.extend(trans_removed.into_iter());
    if !to_delete.is_empty() {
        remove_dirs(&to_delete);
    }

    lockfile::write(&lock, lock_path.clone())?;
    if lockfile_has_no_packages(&lock) {
        let _ = std::fs::remove_file(&lock_path);
    }
    cleanup_empty_node_modules_dir();

    for name in &actually_removed {
        if let Some(version) = lock
            .packages
            .get(&format!("node_modules/{}", name))
            .and_then(|entry| entry.version.clone())
        {
            println!(
                "{gray}[pacm]{reset} {red}-{reset} {name}@{version}",
                gray = C_GRAY,
                red = C_RED,
                reset = C_RESET,
                name = name,
                version = version
            );
        } else {
            println!(
                "{gray}[pacm]{reset} {red}-{reset} {name}",
                gray = C_GRAY,
                red = C_RED,
                reset = C_RESET,
                name = name
            );
        }
    }

    let duration = start.elapsed();
    println!(
        "{gray}[pacm]{reset} summary: {red}{removed} removed{reset} in {time:.2?}",
        gray = C_GRAY,
        red = C_RED,
        reset = C_RESET,
        removed = to_delete.len(),
        time = duration
    );
    Ok(())
}
