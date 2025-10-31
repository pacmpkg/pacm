use crate::colors::*;
use crate::lockfile::Lockfile;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn cmd_scripts_run(
    packages: Vec<String>,
    all: bool,
    ignore_scripts: bool,
    yes: bool,
    per_package: bool,
) -> Result<()> {
    if ignore_scripts {
        println!("{C_GRAY}[pacm]{C_RESET} scripts are ignored by flag");
        return Ok(());
    }

    let project_root = std::env::current_dir()?;
    let lock_path = project_root.join("pacm.lockb");
    let lock = Lockfile::load_or_default(lock_path)?;

    // gather candidate packages
    let mut candidates: Vec<String> = Vec::new();
    if all {
        for k in lock.packages.keys() {
            if k.starts_with("node_modules/") {
                if let Some(name) = k.strip_prefix("node_modules/") {
                    candidates.push(name.to_string());
                }
            }
        }
    } else if !packages.is_empty() {
        for p in packages {
            candidates.push(p);
        }
    } else {
        // prompt user to choose (simple stdin prompt)
        println!("Which package do you want to run scripts for? (comma-separated) ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        for part in input.split(',') {
            let t = part.trim();
            if !t.is_empty() {
                candidates.push(t.to_string());
            }
        }
    }

    // also optionally include root
    let local_pkg = project_root.join("package.json");
    let mut root_scripts = None;
    if local_pkg.exists() {
        if let Ok(txt) = std::fs::read_to_string(&local_pkg) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                if let Some(s) = val.get("scripts") {
                    root_scripts = Some(s.clone());
                }
            }
        }
    }

    // For each candidate, determine script commands from store metadata (metadata.json) under store_path
    for pkg in &candidates {
        let key = format!("node_modules/{pkg}");
        if let Some(entry) = lock.packages.get(&key) {
            if let Some(store_path) = &entry.store_path {
                let metadata_path = PathBuf::from(store_path).join("metadata.json");
                if metadata_path.exists() {
                    if let Ok(txt) = std::fs::read_to_string(&metadata_path) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                            if let Some(scripts) = val.get("scripts") {
                                // Confirmation handling
                                if !yes && per_package {
                                    println!(
                                            "{C_GRAY}[pacm]{C_RESET} run scripts for package '{pkg}'? [y/N]"
                                        );
                                    let mut input = String::new();
                                    std::io::stdin().read_line(&mut input)?;
                                    if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
                                    {
                                        println!(
                                            "{C_GRAY}[pacm]{C_RESET} skipping scripts for {pkg}"
                                        );
                                        continue;
                                    }
                                }
                                run_lifecycle_for_package(
                                    pkg,
                                    &project_root.join("node_modules").join(pkg),
                                    scripts,
                                )?;
                            }
                        }
                    }
                }
            }
        }
    }

    // If root selected or all, run root lifecycle scripts at end
    if let Some(scripts) = root_scripts {
        if !yes {
            println!("{C_GRAY}[pacm]{C_RESET} run scripts for project root? [y/N]");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                println!("{C_GRAY}[pacm]{C_RESET} skipping root scripts");
                return Ok(());
            }
        }
        // per the requested order: root preinstall before deps already not applicable since install refused to run scripts.
        run_lifecycle_for_package("<root>", &project_root, &scripts)?;
    }

    Ok(())
}

fn run_lifecycle_for_package(
    name: &str,
    pkg_dir: &PathBuf,
    scripts: &serde_json::Value,
) -> Result<()> {
    use std::process::Command;
    // execute preinstall -> install -> postinstall if present
    for phase in ["preinstall", "install", "postinstall"] {
        if let Some(cmd_val) = scripts.get(phase) {
            if let Some(cmd_str) = cmd_val.as_str() {
                println!("{C_GRAY}[pacm]{C_RESET} running {phase} for {name}: {cmd_str}");
                let mut c = if cfg!(windows) {
                    let mut cc = Command::new("cmd");
                    cc.arg("/C").arg(cmd_str);
                    cc
                } else {
                    let mut cc = Command::new("sh");
                    cc.arg("-c").arg(cmd_str);
                    cc
                };
                c.current_dir(pkg_dir);
                // inherit env
                let status = c.status().with_context(|| format!("spawn {phase} for {name}"))?;
                if !status.success() {
                    anyhow::bail!("script {phase} failed for {name}");
                }
            }
        }
    }
    Ok(())
}
