use crate::colors::*;
use crate::lockfile;
use anyhow::Result;
use std::path::PathBuf;

pub fn cmd_list() -> Result<()> {
    let lock_path = PathBuf::from("pacm.lockb");
    let lock = if lock_path.exists() {
        lockfile::load(&lock_path)?
    } else {
        let legacy = PathBuf::from("pacm-lock.json");
        if legacy.exists() {
            let lf = lockfile::load_json_compat(&legacy)?;
            println!("{C_GRAY}[pacm]{C_RESET} {C_YELLOW}note{C_RESET}: reading legacy pacm-lock.json (run 'pacm install' to migrate)");
            lf
        } else {
            println!(
                "{C_GRAY}[pacm]{C_RESET} {C_RED}error{C_RESET} no lockfile. Run 'pacm install'."
            );
            return Ok(());
        }
    };

    println!(
        "{gray}[pacm]{reset} packages ({count} entries):",
        gray = C_GRAY,
        reset = C_RESET,
        count = lock.packages.len()
    );
    for (key, entry) in &lock.packages {
        println!(
            "{gray}[pacm]{reset}  {dim}-{reset} {name} => {version}",
            gray = C_GRAY,
            dim = C_DIM,
            reset = C_RESET,
            name = key,
            version = entry.version.as_deref().unwrap_or("(unresolved)")
        );
    }
    Ok(())
}
