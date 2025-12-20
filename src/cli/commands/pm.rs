use crate::cli::commands::install::{
    build_fast_instances, cleanup_empty_node_modules_dir, lockfile_has_no_packages,
    prune_unreachable, remove_dirs,
};
use crate::colors::*;
use crate::lockfile;
use anyhow::{bail, Result};
use std::path::PathBuf;

pub fn cmd_pm_lockfile(format: String, save: bool) -> Result<()> {
    let lock_path = PathBuf::from("pacm.lockb");
    let lock = if lock_path.exists() {
        lockfile::load(&lock_path)?
    } else {
        let legacy = PathBuf::from("pacm-lock.json");
        if legacy.exists() {
            lockfile::load_json_compat(&legacy)?
        } else {
            bail!("no lockfile found (pacm.lockb or pacm-lock.json)");
        }
    };

    let lower = format.to_ascii_lowercase();
    let (output, ext) = match lower.as_str() {
        "json" => (serde_json::to_string_pretty(&lock)?, "json"),
        "yaml" | "yml" => (serde_yaml::to_string(&lock)?, "yaml"),
        other => bail!("unsupported format '{other}', use 'json' or 'yaml'"),
    };

    if save {
        let file = format!("pacm-lock.readable.{ext}");
        std::fs::write(&file, &output)?;
        println!("{C_GRAY}[pacm]{C_RESET} wrote {file}");
    } else {
        println!("{output}");
    }
    Ok(())
}

pub fn cmd_pm_prune() -> Result<()> {
    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() {
        bail!("no package.json found");
    }
    let manifest = crate::manifest::load(&manifest_path)?;
    let lock_path = PathBuf::from("pacm.lockb");
    let mut lock = if lock_path.exists() {
        lockfile::load(&lock_path)?
    } else {
        bail!("no lockfile found to prune");
    };

    if build_fast_instances(&manifest, &lock, &[]).is_some() {
        let removed = prune_unreachable(&mut lock);
        if !removed.is_empty() {
            remove_dirs(&removed);
            crate::lockfile::write(&lock, lock_path.clone())?;
            if lockfile_has_no_packages(&lock) {
                let _ = std::fs::remove_file(&lock_path);
            }
            cleanup_empty_node_modules_dir();
            println!(
                "{gray}[pacm]{reset} pruned {count} unreachable packages",
                gray = C_GRAY,
                reset = C_RESET,
                count = removed.len()
            );
        } else {
            println!("{C_GRAY}[pacm]{C_RESET} nothing to prune");
        }
    } else {
        println!("{C_GRAY}[pacm]{C_RESET} {C_YELLOW}note{C_RESET}: prune requires existing cached instances; run 'pacm install'");
    }
    Ok(())
}
