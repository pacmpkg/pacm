use std::fs;
use std::io::{BufRead, BufReader};
use std::process::Command;

fn main() {
    if let Err(e) = real_main() {
        eprintln!("pacm-shim error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    // Read our own binary tail to find a line starting with PACM_SHIM:
    // Simpler: read entire file as text is unsafe for binaries; instead, require installer to append a textual line.
    // We'll open and read last few KB for the marker.
    let file = fs::File::open(&exe)?;
    let reader = BufReader::new(file);
    let mut target_rel: Option<String> = None;
    for line in reader.lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("PACM_SHIM:") {
            target_rel = Some(rest.trim().to_string());
        }
    }
    let target_rel =
        target_rel.ok_or_else(|| anyhow::anyhow!("no PACM_SHIM marker in shim binary"))?;
    let base = exe.parent().unwrap_or_else(|| std::path::Path::new("."));
    let target_path = base.join(target_rel);
    let mut cmd = Command::new("node");
    cmd.arg(target_path);
    for arg in std::env::args().skip(1) {
        cmd.arg(arg);
    }
    let status = cmd.status()?;
    std::process::exit(status.code().unwrap_or(1));
}
