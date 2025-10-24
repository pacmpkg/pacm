use anyhow::{Context, Result};
use pacm::cli::PacmCli;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if let Err(e) = real_main() {
        eprintln!("pacm error: {:#}", e);
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    // Shim mode: if current exe has a sidecar .shim file, treat this as a bin shim
    if let Ok(exe_path) = std::env::current_exe() {
        let sidecar = PathBuf::from(format!("{}.shim", exe_path.to_string_lossy()));
        if sidecar.exists() {
            let target = fs::read_to_string(&sidecar).with_context(|| "read .shim file")?;
            let base = exe_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let target_path = base.join(target.trim());
            let mut cmd = Command::new("node");
            cmd.arg(target_path);
            // Pass through all CLI args
            for arg in std::env::args().skip(1) {
                cmd.arg(arg);
            }
            let status = cmd.status().with_context(|| "spawn node for bin shim")?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }
    let cli = PacmCli::parse();
    cli.run()
}
