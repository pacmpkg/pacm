use crate::colors::*;
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn path_with_bin_prefix(bin_dir: &Path) -> Option<OsString> {
    if !bin_dir.exists() {
        return None;
    }
    let path_sep = if cfg!(windows) { ";" } else { ":" };
    if let Some(cur) = std::env::var_os("PATH") {
        let mut p = OsString::new();
        p.push(bin_dir.to_string_lossy().as_ref());
        p.push(path_sep);
        p.push(cur);
        Some(p)
    } else if let Some(cur) = std::env::var_os("Path") {
        let mut p = OsString::new();
        p.push(bin_dir.to_string_lossy().as_ref());
        p.push(path_sep);
        p.push(cur);
        Some(p)
    } else {
        let mut p = OsString::new();
        p.push(bin_dir.to_string_lossy().as_ref());
        Some(p)
    }
}

pub(crate) fn quote_arg_for_shell(arg: &str) -> String {
    if cfg!(windows) {
        // Simple Windows quoting: wrap in double quotes if spaces or special chars
        // Quote when containing spaces/quotes or when it's a flag/option (starts with '-') so
        // arguments like `--watch` are preserved when appended to script strings.
        if arg.contains(' ') || arg.contains('"') || arg.starts_with('-') {
            let escaped = arg.replace('"', "\\\"");
            format!("\"{escaped}\"")
        } else {
            arg.to_string()
        }
    } else {
        // POSIX single-quote with escaping of single-quotes
        if arg.is_empty() {
            "''".to_string()
        } else if arg.chars().all(|c| !c.is_whitespace())
            && !arg.contains('"')
            && !arg.contains('\'')
            && !arg.starts_with('-')
        {
            // no whitespace, no quotes, and not a flag -> safe
            // We intentionally quote flags (starting with '-') to preserve them
            // when appended to scripts (matching test expectations).
            arg.to_string()
        } else {
            // escape single quotes by closing, escaping, and reopening
            let mut out = String::new();
            out.push('\'');
            for ch in arg.chars() {
                if ch == '\'' {
                    out.push_str("'\\''");
                } else {
                    out.push(ch);
                }
            }
            out.push('\'');
            out
        }
    }
}

pub(crate) fn build_script_command(script: &str, pass_args: &[String]) -> String {
    if pass_args.is_empty() {
        script.to_string()
    } else {
        let quoted: Vec<String> = pass_args.iter().map(|a| quote_arg_for_shell(a)).collect();
        format!("{} {}", script, quoted.join(" "))
    }
}

pub fn cmd_run(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        println!("Usage: pacm run <script-or-binary> [args...]");
        return Ok(());
    }

    let project_root = std::env::current_dir()?;
    let bin_dir = project_root.join("node_modules").join(".bin");

    // Try load package.json scripts at project root
    let mut root_scripts: Option<serde_json::Map<String, serde_json::Value>> = None;
    let pkg_path = project_root.join("package.json");
    if pkg_path.exists() {
        if let Ok(txt) = std::fs::read_to_string(&pkg_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                if let Some(s) = val.get("scripts") {
                    if let Some(map) = s.as_object() {
                        root_scripts = Some(map.clone());
                    }
                }
            }
        }
    }

    // Interpret `--` separator: everything after `--` is passed as args to script/binary
    let sep_pos = args.iter().position(|s| s == "--");
    let (first, pass_args_vec): (String, Vec<String>) = if let Some(pos) = sep_pos {
        let cmd = args[0].clone();
        let tail = args[(pos + 1)..].to_vec();
        (cmd, tail)
    } else {
        let cmd = args[0].clone();
        let tail = args.iter().skip(1).cloned().collect();
        (cmd, tail)
    };

    // Build PATH prefix with node_modules/.bin if present
    let new_path = path_with_bin_prefix(&bin_dir);

    // If the first arg matches a script name in package.json, run it via shell
    if let Some(scripts) = &root_scripts {
        if let Some(cmd_val) = scripts.get(&first) {
            if let Some(cmd_str) = cmd_val.as_str() {
                let final_cmd = build_script_command(cmd_str, &pass_args_vec);
                println!("{C_GRAY}[pacm]{C_RESET} running script: {first} -> {final_cmd}");
                let mut c = if cfg!(windows) {
                    let mut cc = std::process::Command::new("cmd");
                    cc.arg("/C").arg(&final_cmd);
                    cc
                } else {
                    let mut cc = std::process::Command::new("sh");
                    cc.arg("-c").arg(&final_cmd);
                    cc
                };
                c.current_dir(&project_root);
                if let Some(p) = &new_path {
                    c.env("PATH", p);
                    if cfg!(windows) {
                        c.env("Path", p);
                    }
                }
                let status = c.status().with_context(|| format!("spawn script {first}"))?;
                if !status.success() {
                    anyhow::bail!("script {first} failed");
                }
                return Ok(());
            }
        }
    }

    // Not a package script — try to execute a binary from node_modules/.bin
    if bin_dir.exists() {
        // Candidate names to try (windows: .exe, fallback no-ext; unix: direct)
        let mut candidates: Vec<PathBuf> = Vec::new();
        if cfg!(windows) {
            candidates.push(bin_dir.join(format!("{first}.exe")));
            candidates.push(bin_dir.join(&first));
            candidates.push(bin_dir.join(format!("{first}.cmd")));
        } else {
            candidates.push(bin_dir.join(&first));
        }
        for cand in candidates {
            if cand.exists() {
                // Execute bin directly
                println!("{C_GRAY}[pacm]{C_RESET} running binary: {}", cand.display());
                let mut cmd = std::process::Command::new(cand);
                for a in &pass_args_vec {
                    cmd.arg(a);
                }
                cmd.current_dir(&project_root);
                if let Some(p) = &new_path {
                    cmd.env("PATH", p);
                    if cfg!(windows) {
                        cmd.env("Path", p);
                    }
                }
                let status = cmd.status().with_context(|| format!("spawn binary {first}"))?;
                if !status.success() {
                    anyhow::bail!("binary {first} failed");
                }
                return Ok(());
            }
        }
    }

    // Fallback: run as a shell command (this will use PATH which we've prefixed)
    let joined = args.join(" ");
    println!("{C_GRAY}[pacm]{C_RESET} running shell: {joined}");
    let mut sh = if cfg!(windows) {
        let mut cc = std::process::Command::new("cmd");
        cc.arg("/C").arg(&joined);
        cc
    } else {
        let mut cc = std::process::Command::new("sh");
        cc.arg("-c").arg(&joined);
        cc
    };
    sh.current_dir(&project_root);
    if let Some(p) = &new_path {
        sh.env("PATH", p);
        if cfg!(windows) {
            sh.env("Path", p);
        }
    }
    let status = sh.status().with_context(|| "spawn fallback shell")?;
    if !status.success() {
        anyhow::bail!("command failed");
    }
    Ok(())
}
