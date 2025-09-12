use crate::colors::*;
use crate::fetch::Fetcher;
use crate::installer::{Installer, PackageInstance};
use crate::lockfile::{self, Lockfile, PackageEntry};
use crate::manifest::{self, Manifest};
use crate::resolver::{map_versions, Resolver};
use anyhow::Context;
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    name = "pacm",
    version,
    about = "Fast, cache-first JavaScript/TypeScript package manager",
    long_about = "pacm — a blazing fast, cache-first package manager.\n\nExamples:\n  pacm init --name my-app\n  pacm install\n  pacm add axios\n  pacm cache path\n  pacm cache clean"
)]
pub struct PacmCli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Create a new package.json (interactive later)
    Init {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        version: Option<String>,
    },
    /// Remove one or more dependencies
    Remove {
        /// Packages to remove (by name)
        packages: Vec<String>,
    },
    /// Install all deps or add specific packages
    Install {
        /// Packages to add (name[@range])
        packages: Vec<String>,
        /// Save to devDependencies
        #[arg(long, short = 'D')]
        dev: bool,
        /// Save as optionalDependency
        #[arg(long)]
        optional: bool,
        /// Do not write package.json (no-save)
        #[arg(long = "no-save")]
        no_save: bool,
        /// Install exact version (no range coercion)
        #[arg(long)]
        exact: bool,
        /// Do not hit the network; fail if package/version is missing in cache
        #[arg(long)]
        prefer_offline: bool,
        /// Disable progress rendering
        #[arg(long)]
        no_progress: bool,
    },
    /// Add a dependency (alias for install <pkg>)
    Add {
        /// Package to add (name[@range])
        package: String,
        /// Save to devDependencies
        #[arg(long, short = 'D')]
        dev: bool,
        /// Save as optionalDependency
        #[arg(long)]
        optional: bool,
        /// Do not write package.json (no-save)
        #[arg(long = "no-save")]
        no_save: bool,
        /// Install exact version
        #[arg(long)]
        exact: bool,
    },
    /// List packages from lockfile
    List,
    /// Manage the global cache
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
    /// Package manager utilities (similar to npm/pnpm/bun pm)
    Pm {
        #[command(subcommand)]
        cmd: PmCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheCmd {
    /// Show the cache path on this machine
    Path,
    /// Clean the cache (remove all cached packages)
    Clean,
}

#[derive(Subcommand, Debug)]
pub enum PmCmd {
    /// Export the lockfile in human-readable form
    Lockfile {
        /// Output format: json or yaml
        #[arg(long, short = 'f', default_value = "json")]
        format: String,
        /// Save to a file in the current directory instead of only printing
        #[arg(long, short = 's')]
        save: bool,
    },
    /// Prune transitive dependencies no longer referenced by roots
    Prune,
    /// List lockfile entries (alias of list)
    Ls,
}

impl PacmCli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }

    pub fn run(&self) -> Result<()> {
        match &self.command {
            None => {
                self.print_help();
                Ok(())
            }
            Some(Commands::Init { name, version }) => cmd_init(name.clone(), version.clone()),
            Some(Commands::Install {
                packages,
                dev,
                optional,
                no_save,
                exact,
                prefer_offline,
                no_progress,
            }) => cmd_install(
                packages.clone(),
                *dev,
                *optional,
                *no_save,
                *exact,
                *prefer_offline,
                *no_progress,
            ),
            Some(Commands::Add {
                package,
                dev,
                optional,
                no_save,
                exact,
            }) => cmd_install(
                vec![package.clone()],
                *dev,
                *optional,
                *no_save,
                *exact,
                false,
                false,
            ),
            Some(Commands::Remove { packages }) => cmd_remove(packages.clone()),
            Some(Commands::List) => cmd_list(),
            Some(Commands::Cache { cmd }) => match cmd {
                CacheCmd::Path => cmd_cache_path(),
                CacheCmd::Clean => cmd_cache_clean(),
            },
            Some(Commands::Pm { cmd }) => match cmd {
                PmCmd::Lockfile { format, save } => cmd_pm_lockfile(format.clone(), *save),
                PmCmd::Prune => cmd_pm_prune(),
                PmCmd::Ls => cmd_list(),
            },
        }
    }

    fn print_help(&self) {
    println!("pacm - Fast, cache-first package manager\n");
    println!("Commands:\n  init [--name --version]\n  install [pkg..] [--dev|--optional] [--no-save] [--prefer-offline] [--no-progress]\n  add <pkg> [--dev|--optional] [--no-save]\n  remove <pkg..>\n  list\n  cache <path|clean>\n  pm <lockfile|prune|ls> [options]");
    }
}

fn cmd_init(name: Option<String>, version: Option<String>) -> Result<()> {
    let path = PathBuf::from("package.json");
    if path.exists() {
        bail!("package.json already exists");
    }
    let manifest = Manifest::new(
        name.unwrap_or_else(|| "my-app".into()),
        version.unwrap_or_else(|| "0.1.0".into()),
    );
    manifest::write(&manifest, &path)?;
    println!(
        "{gray}[pacm]{reset} {green}init{reset} created {name}@{ver}",
        gray = C_GRAY,
        reset = C_RESET,
        green = C_GREEN,
        name = manifest.name,
        ver = manifest.version
    );
    Ok(())
}

pub(crate) struct ProgressRenderer {
    last_len: usize,
    last_status: String,
}
impl ProgressRenderer {
    fn new() -> Self {
        Self {
            last_len: 0,
            last_status: String::new(),
        }
    }
}

// Progress handle helpers removed in cache-first refactor.

fn render_status(pr: &mut ProgressRenderer, raw: impl Into<String>) {
    let raw = raw.into();
    pr.last_status = raw.clone();
    let mut out = io::stdout();
    let pad = if pr.last_len > raw.len() {
        pr.last_len - raw.len()
    } else {
        0
    };
    write!(out, "\r{}{}", raw, " ".repeat(pad)).ok();
    out.flush().ok();
    pr.last_len = raw.len();
}

fn format_status(kind: &str, detail: &str) -> String {
    let (color, action) = match kind {
        "resolving" => (C_CYAN, "resolving"),
        "downloading" => (C_CYAN, "downloading"),
        "extracting" => (C_MAGENTA, "extracting"),
        "linking" => (C_GREEN, "linking"),
        "fast" => (C_GREEN, "fast"),
        _ => (C_DIM, kind),
    };
    format!(
        "{gray}[pacm]{reset} {color}{action}{reset} {detail}",
        gray = C_GRAY,
        reset = C_RESET,
        color = color,
        action = action,
        detail = detail
    )
}

fn finish_progress(pr: &mut ProgressRenderer) {
    println!();
    pr.last_len = 0;
}

// Represents an in-flight download progress (for large packages >10MB)
#[derive(Clone, Debug)]
struct DownloadProgress {
    name: String,
    version: String,
    downloaded: u64,
    total: Option<u64>,
    done: bool,
    started_at: Instant,
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0usize;
    while v > 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{}{}", bytes, UNITS[i])
    } else {
        format!("{:.1}{}", v, UNITS[i])
    }
}

fn cmd_install(
    specs: Vec<String>,
    dev: bool,
    optional: bool,
    no_save: bool,
    _exact: bool,
    prefer_offline: bool,
    no_progress: bool,
) -> Result<()> {
    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() {
        println!(
            "{gray}[pacm]{reset} {red}error{reset} no package.json found. Run 'pacm init' first.",
            gray = C_GRAY,
            red = C_RED,
            reset = C_RESET
        );
        return Ok(());
    }
    let mut manifest = manifest::load(&manifest_path)?;
    // If user provided package specs on the command line, resolve/persist only those names
    if !specs.is_empty() {
        // Resolve provided specs to concrete versions for package.json (if not --no-save)
        for spec in &specs {
            // Parse package spec allowing scoped names. Strategy:
            // - If spec contains '@' but starts with '@' (scoped) we look for the last '@' to split version, e.g. @scope/name@1.2.3
            let (name, req) = if spec.starts_with('@') {
                if let Some(idx) = spec.rfind('@') {
                    if idx == 0 {
                        (spec.to_string(), "*".to_string())
                    } else {
                        let (n, r) = spec.split_at(idx);
                        (n.to_string(), r[1..].to_string())
                    }
                } else {
                    (spec.to_string(), "*".to_string())
                }
            } else {
                if let Some((n, r)) = spec.split_once('@') {
                    (
                        n.to_string(),
                        if r.is_empty() {
                            "*".to_string()
                        } else {
                            r.to_string()
                        },
                    )
                } else {
                    (spec.to_string(), "*".to_string())
                }
            };

            // If no-save is false, resolve a concrete version (prefer cache) to persist; otherwise just keep the range
            let resolved_version = if no_save { req.clone() } else {
                // Prefer cache: find highest cached version satisfying the req (or latest if '*')
                let req_str = req.as_str();
                let candidates = crate::cache::cached_versions(&name);
                let maybe_cached_pick = if req_str == "*" {
                    candidates.first().cloned()
                } else {
                    let _keep = crate::resolver::map_versions; // keep import referenced
                    let canon = crate::resolver::canonicalize_npm_range(req_str);
                    let parsed = semver::VersionReq::parse(&canon).ok();
                    if let Some(rq) = parsed {
                        candidates.into_iter().find(|v| rq.matches(v))
                    } else { None }
                };
                if let Some(v) = maybe_cached_pick { v.to_string() } else {
                    // fallback to registry latest/tag range without resolving full workspace
                    let fetcher = Fetcher::new(None)?;
                    let meta = fetcher
                        .package_metadata(&name)
                        .with_context(|| format!("fetch metadata for {}", name))?;
                    if req == "*" || req.eq_ignore_ascii_case("latest") {
                        if let Some(tags) = &meta.dist_tags {
                            if let Some(ver) = tags.get("latest") { ver.clone() } else { req.clone() }
                        } else { req.clone() }
                    } else if let Some(tags) = &meta.dist_tags {
                        if let Some(ver) = tags.get(&req) { ver.clone() } else { req.clone() }
                    } else { req.clone() }
                }
            };

            if !no_save {
                add_spec_with_version(&mut manifest, &name, &resolved_version, dev, optional)?;
            }
        }
        if !no_save {
            manifest::write(&manifest, &manifest_path)?;
        }
    }
    // Lock handling prior to potential fast path
    let lock_path = PathBuf::from("pacm.lockb");
    let mut lock = if lock_path.exists() {
        Lockfile::load_or_default(lock_path.clone())?
    } else {
        let legacy = PathBuf::from("pacm-lock.json");
        if legacy.exists() {
            let lf = lockfile::load_json_compat(&legacy)?;
            // Write migrated binary file
            lockfile::write(&lf, lock_path.clone())?;
            println!(
                "{gray}[pacm]{reset} migrated lockfile to binary: pacm.lockb",
                gray = C_GRAY,
                reset = C_RESET
            );
            lf
        } else {
            Lockfile::default()
        }
    };
    // Copy for diffing + no-op detection
    let original_lock = lock.clone();

    // Capture old root dependency map (pre-sync)
    let old_root_deps: std::collections::BTreeMap<String, String> = original_lock
        .packages
        .get("")
        .map(|p| {
            let mut m = BTreeMap::new();
            m.extend(p.dependencies.clone());
            m.extend(p.dev_dependencies.clone());
            m.extend(p.optional_dependencies.clone());
            m
        })
        .unwrap_or_default();

    // Sync lock root + placeholder entries for new deps
    lock.sync_from_manifest(&manifest);
    let new_root_deps: std::collections::BTreeMap<String, String> = lock
        .packages
        .get("")
        .map(|p| {
            let mut m = BTreeMap::new();
            m.extend(p.dependencies.clone());
            m.extend(p.dev_dependencies.clone());
            m.extend(p.optional_dependencies.clone());
            m
        })
        .unwrap_or_default();

    // Determine added / removed top-level dependencies (by name)
    use std::collections::BTreeSet;
    let old_names: BTreeSet<_> = old_root_deps.keys().cloned().collect();
    let new_names: BTreeSet<_> = new_root_deps.keys().cloned().collect();
    let added_root: Vec<String> = new_names.difference(&old_names).cloned().collect();
    let removed_root: Vec<String> = old_names.difference(&new_names).cloned().collect();

    // If lockfile didn't change and node_modules/.pacm appear intact, behave like pnpm's no-op
    if lock == original_lock
        && added_root.is_empty()
        && removed_root.is_empty()
        && node_modules_intact(&manifest)
    {
        println!(
            "{gray}[pacm]{reset} {dim}no dependency changes{reset}",
            gray = C_GRAY,
            dim = C_DIM,
            reset = C_RESET
        );
        println!(
            "{gray}[pacm]{reset} {dim}0 added, 0 removed{reset}",
            gray = C_GRAY,
            dim = C_DIM,
            reset = C_RESET
        );
        println!(
            "{gray}[pacm]{reset} {green}already up to date{reset}",
            gray = C_GRAY,
            green = C_GREEN,
            reset = C_RESET
        );
        return Ok(());
    }

    // Try fast path if user did not request adding packages (specs empty)
    if specs.is_empty() {
        // Fast path only if no newly added dependencies (they require resolution). Removals can still fast-path.
        if added_root.is_empty() {
            if let Some(instances) = build_fast_instances(&manifest, &lock) {
                // Prune removed dependencies from lock + filesystem before linking
                if !removed_root.is_empty() {
                    prune_removed_from_lock(&mut lock, &removed_root);
                    remove_dirs(&removed_root);
                    // Also drop any unreachable stale lock entries (after removal)
                    let trans_removed = prune_unreachable(&mut lock, &instances);
                    if !trans_removed.is_empty() {
                        remove_dirs(&trans_removed);
                    }
                }
                let start = Instant::now();
                let mut merged_root_deps: BTreeMap<String, String> = BTreeMap::new();
                for (n, r) in &manifest.dependencies {
                    merged_root_deps.insert(n.clone(), r.clone());
                }
                for (n, r) in &manifest.dev_dependencies {
                    merged_root_deps.insert(n.clone(), r.clone());
                }
                for (n, r) in &manifest.optional_dependencies {
                    merged_root_deps.insert(n.clone(), r.clone());
                }
                // Setup progress renderer so warnings appear above status
                let progress = Arc::new(Mutex::new(ProgressRenderer::new()));
                {
                    let mut pr = progress.lock().unwrap();
                    render_status(
                        &mut pr,
                        format_status("fast", "link: using cached store; skipping resolution"),
                    );
                }
                let installer = Installer::new();
                let as_hash: std::collections::HashMap<String, PackageInstance> =
                    instances.into_iter().collect();
                installer.install(&std::env::current_dir()?, &as_hash, &merged_root_deps)?;
                // Finish status line
                {
                    let mut pr = progress.lock().unwrap();
                    finish_progress(&mut pr);
                }
                // progress handle helpers removed
                // Write updated (pruned) lockfile and cleanup if now empty
                lockfile::write(&lock, lock_path.clone())?;
                if lockfile_has_no_packages(&lock) {
                    let _ = std::fs::remove_file(&lock_path);
                }
                cleanup_empty_node_modules_dir();
                let dur = start.elapsed();
                // Summary lines
                if added_root.is_empty() && removed_root.is_empty() {
                    println!(
                        "{gray}[pacm]{reset} {dim}no dependency changes{reset}",
                        gray = C_GRAY,
                        dim = C_DIM,
                        reset = C_RESET
                    );
                }
                for r in &removed_root {
                    if let Some(ver) = original_lock
                        .packages
                        .get(&format!("node_modules/{}", r))
                        .and_then(|e| e.version.as_ref())
                    {
                        println!(
                            "{gray}[pacm]{reset} {red}-{reset} {name}@{ver}",
                            gray = C_GRAY,
                            red = C_RED,
                            reset = C_RESET,
                            name = r,
                            ver = ver
                        );
                    } else {
                        println!(
                            "{gray}[pacm]{reset} {red}-{reset} {name}",
                            gray = C_GRAY,
                            red = C_RED,
                            reset = C_RESET,
                            name = r
                        );
                    }
                }
                let total = as_hash.len();
                println!(
                    "{gray}[pacm]{reset} summary: {green}0 added{reset}, {red}{rm} removed{reset}",
                    gray = C_GRAY,
                    green = C_GREEN,
                    red = C_RED,
                    rm = removed_root.len(),
                    reset = C_RESET
                );
                println!("{gray}[pacm]{reset} {green}linked{reset} {total} packages (all cached) in {secs:.2?}", gray=C_GRAY, green=C_GREEN, reset=C_RESET, total=total, secs=dur);
                return Ok(());
            }
        }
    }

    // Proceed with resolution path; if specs provided, limit to those packages only
    let fetcher = Fetcher::new(None)?;
    let resolver = Resolver::new();
    #[derive(Clone)]
    struct Task {
        name: String,
        range: String,
    }
    let mut queue: VecDeque<Task> = VecDeque::new();
    if specs.is_empty() {
        for (n, r) in &manifest.dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone() });
        }
        for (n, r) in &manifest.dev_dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone() });
        }
        for (n, r) in &manifest.optional_dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone() });
        }
    } else {
        // Seed only the provided specs
        for spec in &specs {
            let (name, req) = if spec.starts_with('@') {
                if let Some(idx) = spec.rfind('@') { if idx == 0 { (spec.to_string(), "*".to_string()) } else { let (n, r) = spec.split_at(idx); (n.to_string(), r[1..].to_string()) } } else { (spec.to_string(), "*".to_string()) }
            } else {
                if let Some((n, r)) = spec.split_once('@') { (n.to_string(), if r.is_empty() { "*".to_string() } else { r.to_string() }) } else { (spec.to_string(), "*".to_string()) }
            };
            queue.push_back(Task { name, range: req });
        }
    }
    let mut visited_name_version: HashSet<(String, String)> = HashSet::new();

    let start = Instant::now();
    let mut installed_count = 0usize;
    use std::sync::atomic::{AtomicBool, Ordering};
    let progress = Arc::new(Mutex::new(ProgressRenderer::new()));
    // progress handle helpers removed
    let downloads: Arc<Mutex<Vec<DownloadProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let progress_clone = progress.clone();
    let downloads_clone = downloads.clone();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_thread = stop_flag.clone();

    let painter = if no_progress {
        None
    } else {
        Some(thread::spawn(move || {
            let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]; // braille spinner
            let mut tick: usize = 0;
            loop {
                thread::sleep(Duration::from_millis(100));
                let mut pr = progress_clone.lock().unwrap();
                let dl = downloads_clone.lock().unwrap();
                let mut active_lines = Vec::new();
                for d in dl.iter() {
                    if d.done { continue; }
                    let frame = spinner_frames[tick % spinner_frames.len()];
                    let total = d.total.unwrap_or(0);
                    let pct = if total > 0 { (d.downloaded as f64 / total as f64) * 100.0 } else { 0.0 };
                    let bar = if total > 0 {
                        let width = 18usize;
                        let filled = ((d.downloaded as f64 / total as f64) * width as f64) as usize;
                        let mut s = String::new();
                        s.push('[');
                        s.push_str(&"#".repeat(filled.min(width)));
                        s.push_str(&"-".repeat(width.saturating_sub(filled)));
                        s.push(']');
                        s
                    } else {
                        "[.................]".to_string()
                    };
                    let elapsed = d.started_at.elapsed().as_secs_f64();
                    let speed = if elapsed > 0.0 { d.downloaded as f64 / elapsed } else { 0.0 };
                    let eta = if total > 0 && speed > 0.0 {
                        let remain = (total.saturating_sub(d.downloaded)) as f64 / speed;
                        format!("{:.1}s", remain)
                    } else { "?s".to_string() };
                    active_lines.push(format!(
                        "{frame} {name}@{ver} {bar} {pct:.0}% {done}/{total} {spd}/s ETA {eta}",
                        frame=frame,
                        name=d.name,
                        ver=d.version,
                        bar=bar,
                        pct=pct,
                        done=human_size(d.downloaded),
                        total=if total>0 { human_size(total) } else { "?".into() },
                        spd=human_size(speed as u64),
                        eta=eta
                    ));
                }
                tick = tick.wrapping_add(1);
                if active_lines.is_empty() {
                    if stop_flag_thread.load(Ordering::SeqCst) { break; }
                    continue;
                }
                render_status(&mut pr, active_lines.join(" | "));
                if dl.iter().all(|d| d.done) && stop_flag_thread.load(Ordering::SeqCst) { break; }
            }
        }))
    };

    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();

    while let Some(Task { name, range }) = queue.pop_front() {
        if visited_name_version.iter().any(|(n, _)| n == &name) {
            continue;
        }
        if !no_progress {
            let mut pr = progress.lock().unwrap();
            render_status(
                &mut pr,
                format_status("resolving", &format!("{}@{}", name, range)),
            );
        }
        // Cache-first pick: try cached versions satisfying the range, with dist-tag support
        let (picked_ver, tarball_url) = {
            let cached = crate::cache::cached_versions(&name);
            let canon = crate::resolver::canonicalize_npm_range(&range);
            let parsed_req = semver::VersionReq::parse(&canon).ok();
            let is_tag_spec = parsed_req.is_none() && canon != "*" && !range.eq_ignore_ascii_case("latest");
            if is_tag_spec {
                if prefer_offline {
                    bail!("cannot resolve dist-tag '{}' for {} offline", range, name);
                }
                // Force registry fetch to resolve tag
                let meta = fetcher
                    .package_metadata(&name)
                    .with_context(|| format!("fetch metadata for {}", name))?;
                if let Some(tags) = &meta.dist_tags {
                    if let Some(ver_s) = tags.get(&range) {
                        let ver = semver::Version::parse(ver_s)
                            .with_context(|| format!("invalid version '{}' for tag '{}'", ver_s, range))?;
                        let tar = meta
                            .versions
                            .get(ver_s)
                            .map(|v| v.dist.tarball.clone())
                            .unwrap_or_default();
                        (ver, tar)
                    } else {
                        bail!("unknown dist-tag '{}' for {}", range, name);
                    }
                } else {
                    bail!("no dist-tags available for {}", name);
                }
            } else {
                let req = if canon == "*" { semver::VersionReq::STAR } else { parsed_req.unwrap_or(semver::VersionReq::STAR) };
                if let Some(v) = cached.into_iter().find(|v| req.matches(v)) {
                    let pv = v.clone();
                    (pv.clone(), String::new())
                } else {
                    let meta = fetcher
                        .package_metadata(&name)
                        .with_context(|| format!("fetch metadata for {}", name))?;
                    // If user asked for 'latest', honor dist-tag directly
                    if range.eq_ignore_ascii_case("latest") {
                        if let Some(tags) = &meta.dist_tags {
                            if let Some(ver_s) = tags.get("latest") {
                                let ver = semver::Version::parse(ver_s)?;
                                let tar = meta
                                    .versions
                                    .get(ver_s)
                                    .map(|v| v.dist.tarball.clone())
                                    .unwrap_or_default();
                                (ver, tar)
                            } else {
                                let version_map = map_versions(&meta);
                                resolver.pick_version(&version_map, "*")?
                            }
                        } else {
                            let version_map = map_versions(&meta);
                            resolver.pick_version(&version_map, "*")?
                        }
                    } else {
                        let version_map = map_versions(&meta);
                        resolver.pick_version(&version_map, &range)?
                    }
                }
            }
        };
        if visited_name_version.contains(&(name.clone(), picked_ver.to_string())) {
            continue;
        }
        // If we picked from cache, load manifest from cache; otherwise use registry metadata
        let (integrity_owned, dep_map, resolved_url): (Option<String>, BTreeMap<String, String>, Option<String>) = if tarball_url.is_empty() {
            let cached_mf = crate::cache::read_cached_manifest(&name, &picked_ver.to_string())?;
            (None, cached_mf.dependencies.into_iter().collect(), None)
        } else {
            let meta2 = fetcher
                .package_metadata(&name)
                .with_context(|| format!("fetch metadata for {}", name))?;
            let version_meta = meta2
                .versions
                .get(&picked_ver.to_string())
                .expect("version meta");
            let integrity_owned = version_meta.dist.integrity.clone();
            let mut dm = BTreeMap::new();
            for (dn, dr) in &version_meta.dependencies { dm.insert(dn.clone(), dr.clone()); }
            (integrity_owned, dm, Some(version_meta.dist.tarball.clone()))
        };
        let mut reused = false;
        let cached = crate::cache::cache_package_path(&name, &picked_ver.to_string()).exists();
        let integrity = if cached {
            reused = true;
            integrity_owned.as_deref().unwrap_or("").to_string()
        } else {
            if prefer_offline {
                bail!("{name}@{ver} not in cache and --prefer-offline is set", name=name, ver=picked_ver);
            }
            {
                if !no_progress {
                    let mut pr = progress.lock().unwrap();
                    render_status(
                        &mut pr,
                        format_status("downloading", &format!("{}@{}", name, picked_ver)),
                    );
                }
            }
            let url = resolved_url
                .as_deref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| tarball_url.clone());
            let bytes = perform_download(&fetcher, &name, &picked_ver.to_string(), &url, &downloads)?;
            {
                if !no_progress {
                    let mut pr = progress.lock().unwrap();
                    render_status(
                        &mut pr,
                        format_status("extracting", &format!("{}@{}", name, picked_ver)),
                    );
                }
            }
            crate::cache::ensure_cached_package(&name, &picked_ver.to_string(), &bytes, integrity_owned.as_deref())?
        };
        // Defer linking – build instance map
        let key = format!("node_modules/{}", name);
        let entry = lock.packages.entry(key).or_insert(PackageEntry {
            version: None,
            integrity: None,
            resolved: None,
            dependencies: Default::default(),
            dev_dependencies: Default::default(),
            optional_dependencies: Default::default(),
            peer_dependencies: Default::default(),
        });
        entry.version = Some(picked_ver.to_string());
        if !integrity.is_empty() { entry.integrity = Some(integrity.clone()); }
        let resolved_for_lock = resolved_url.clone().or_else(|| if !tarball_url.is_empty() { Some(tarball_url.clone()) } else { None });
        entry.resolved = resolved_for_lock;
        entry.dependencies = dep_map.clone();
        instances.insert(
            name.clone(),
            PackageInstance {
                name: name.clone(),
                version: picked_ver.to_string(),
                dependencies: dep_map.clone(),
            },
        );
        visited_name_version.insert((name.clone(), picked_ver.to_string()));
        if !reused {
            installed_count += 1;
        }
        for (dn, dr) in dep_map {
            queue.push_back(Task {
                name: dn,
                range: dr,
            });
        }
    }
    // Before linking phase: clear dynamic line so upcoming warnings stack above future linking status
    if !no_progress {
        let mut pr = progress.lock().unwrap();
        if pr.last_len > 0 {
            print!("\r{}\r", " ".repeat(pr.last_len));
            io::stdout().flush().ok();
        }
        pr.last_status.clear();
        pr.last_len = 0;
    }
    // Before linking: prune unreachable entries only when doing a full install
    if specs.is_empty() {
        let trans_removed = prune_unreachable(&mut lock, &instances);
        if !trans_removed.is_empty() {
            remove_dirs(&trans_removed);
        }
    }
    // Perform linking phase (virtual store + facade)
    let installer = Installer::new();
    // root_deps should include dependencies, devDependencies and optionalDependencies so added dev/optional packages are linked
    let mut merged_root_deps: BTreeMap<String, String> = BTreeMap::new();
    for (n, r) in &manifest.dependencies {
        merged_root_deps.insert(n.clone(), r.clone());
    }
    for (n, r) in &manifest.dev_dependencies {
        merged_root_deps.insert(n.clone(), r.clone());
    }
    for (n, r) in &manifest.optional_dependencies {
        merged_root_deps.insert(n.clone(), r.clone());
    }
    // peers are not installed directly but we do include them in the lockfile root; skip for linking
    let total_packages_for_summary = instances.len();
    let instances_for_link: std::collections::HashMap<String, PackageInstance> =
        instances.clone().into_iter().collect();
    installer.install(
        &std::env::current_dir()?,
        &instances_for_link,
        &merged_root_deps,
    )?;
    lockfile::write(&lock, lock_path.clone())?;
    if lockfile_has_no_packages(&lock) {
        let _ = std::fs::remove_file(&lock_path);
    }
    cleanup_empty_node_modules_dir();
    let dur = start.elapsed();
    // Move to new line before summary
    // Ensure painter terminates (if active). Give it a moment.
    // Mark painter stop (downloads finished) and show final linking status once *after* warnings.
    stop_flag.store(true, Ordering::SeqCst);
    if let Some(p) = painter { p.join().ok(); }
    if !no_progress {
        let mut pr = progress.lock().unwrap();
        render_status(&mut pr, format_status("linking", "graph"));
        finish_progress(&mut pr);
    }
    // progress handle helpers removed
    // Compute summary data
    let total = total_packages_for_summary;
    let reused = total.saturating_sub(installed_count);
    // Added root packages now have versions resolved (if any)
    if added_root.is_empty() && removed_root.is_empty() {
        println!(
            "{gray}[pacm]{reset} {dim}no dependency changes{reset}",
            gray = C_GRAY,
            dim = C_DIM,
            reset = C_RESET
        );
    }
    for a in &added_root {
        if let Some(inst) = instances.get(a) {
            println!(
                "{gray}[pacm]{reset} {green}+{reset} {name}@{ver}",
                gray = C_GRAY,
                green = C_GREEN,
                reset = C_RESET,
                name = a,
                ver = inst.version
            );
        } else {
            println!(
                "{gray}[pacm]{reset} {green}+{reset} {name}",
                gray = C_GRAY,
                green = C_GREEN,
                reset = C_RESET,
                name = a
            );
        }
    }
    for r in &removed_root {
        if let Some(ver) = original_lock
            .packages
            .get(&format!("node_modules/{}", r))
            .and_then(|e| e.version.as_ref())
        {
            println!(
                "{gray}[pacm]{reset} {red}-{reset} {name}@{ver}",
                gray = C_GRAY,
                red = C_RED,
                reset = C_RESET,
                name = r,
                ver = ver
            );
        } else {
            println!(
                "{gray}[pacm]{reset} {red}-{reset} {name}",
                gray = C_GRAY,
                red = C_RED,
                reset = C_RESET,
                name = r
            );
        }
    }
    println!(
        "{gray}[pacm]{reset} summary: {green}{add} added{reset}, {red}{rm} removed{reset}",
        gray = C_GRAY,
        green = C_GREEN,
        red = C_RED,
        add = added_root.len(),
        rm = removed_root.len(),
        reset = C_RESET
    );
    println!("{gray}[pacm]{reset} {green}installed{reset} {total} packages ({green}{dl} downloaded{reset}, {dim}{re} reused{reset}) in {secs:.2?}", gray=C_GRAY, green=C_GREEN, dim=C_DIM, reset=C_RESET, total=total, dl=installed_count, re=reused, secs=dur);
    Ok(())
}

fn cmd_cache_path() -> Result<()> {
    let p = crate::fsutil::cache_root();
    println!("{gray}[pacm]{reset} cache: {p}", gray=C_GRAY, reset=C_RESET, p=p.display());
    Ok(())
}

fn cmd_cache_clean() -> Result<()> {
    use std::fs;
    let root = crate::fsutil::cache_root();
    if root.exists() {
        fs::remove_dir_all(&root).ok();
    }
    fs::create_dir_all(&root)?;
    println!("{gray}[pacm]{reset} {green}cache cleaned{reset} at {p}", gray=C_GRAY, green=C_GREEN, reset=C_RESET, p=root.display());
    Ok(())
}

fn cmd_pm_lockfile(format: String, save: bool) -> Result<()> {
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
    let fmt = format.to_ascii_lowercase();
    let (out, ext) = match fmt.as_str() {
        "json" => {
            (serde_json::to_string_pretty(&lock)?, "json")
        }
        "yaml" | "yml" => {
            (serde_yaml::to_string(&lock)?, "yaml")
        }
        other => {
            bail!("unsupported format '{}', use 'json' or 'yaml'", other);
        }
    };
    if save {
        let file = format!("pacm-lock.readable.{ext}");
        std::fs::write(&file, &out)?;
        println!(
            "{gray}[pacm]{reset} wrote {file}",
            gray = C_GRAY,
            reset = C_RESET,
            file = file
        );
    } else {
        println!("{}", out);
    }
    Ok(())
}

fn cmd_pm_prune() -> Result<()> {
    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() {
        bail!("no package.json found");
    }
    let manifest = manifest::load(&manifest_path)?;
    let lock_path = PathBuf::from("pacm.lockb");
    let mut lock = if lock_path.exists() {
        lockfile::load(&lock_path)?
    } else {
        bail!("no lockfile found to prune");
    };
    if let Some(instances) = build_fast_instances(&manifest, &lock) {
        let removed = prune_unreachable(&mut lock, &instances);
        if !removed.is_empty() {
            remove_dirs(&removed);
            lockfile::write(&lock, lock_path.clone())?;
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
            println!("{gray}[pacm]{reset} nothing to prune", gray = C_GRAY, reset = C_RESET);
        }
    } else {
        println!(
            "{gray}[pacm]{reset} {yellow}note{reset}: prune requires existing cached instances; run 'pacm install'",
            gray = C_GRAY,
            yellow = C_YELLOW,
            reset = C_RESET
        );
    }
    Ok(())
}

fn cmd_remove(packages: Vec<String>) -> Result<()> {
    let start = Instant::now();
    if packages.is_empty() {
        bail!("no packages specified to remove");
    }
    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() {
        bail!("no package.json found");
    }
    let mut manifest = manifest::load(&manifest_path)?;
    // Track which actually existed
    let mut actually_removed: Vec<String> = Vec::new();
    for name in &packages {
        if manifest.dependencies.remove(name).is_some() {
            actually_removed.push(name.clone());
        }
        if manifest.dev_dependencies.remove(name).is_some() {
            if !actually_removed.contains(name) { actually_removed.push(name.clone()); }
        }
        if manifest.optional_dependencies.remove(name).is_some() {
            if !actually_removed.contains(name) { actually_removed.push(name.clone()); }
        }
    }
    if actually_removed.is_empty() {
        println!("{gray}[pacm]{reset} {dim}no matching dependencies to remove{reset}", gray=C_GRAY, dim=C_DIM, reset=C_RESET);
        return Ok(());
    }
    // Persist updated manifest
    manifest::write(&manifest, &manifest_path)?;

    // Load or create lockfile
    let lock_path = PathBuf::from("pacm.lockb");
    let mut lock = if lock_path.exists() { lockfile::load(&lock_path)? } else { Lockfile::default() };

    // Remove root lock entries for removed packages
    prune_removed_from_lock(&mut lock, &actually_removed);

    // Build reachable set from updated root using lock entries
    let mut roots: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for (n, _) in &manifest.dependencies { roots.push_back(n.clone()); }
    for (n, _) in &manifest.dev_dependencies { roots.push_back(n.clone()); }
    for (n, _) in &manifest.optional_dependencies { roots.push_back(n.clone()); }
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();
    while let Some(name) = roots.pop_front() {
        if !seen.insert(name.clone()) { continue; }
        let key = format!("node_modules/{}", name);
        if let Some(entry) = lock.packages.get(&key) {
            // Record instance (version may be None; use empty string)
            let ver = entry.version.clone().unwrap_or_default();
            let deps = entry.dependencies.clone();
            instances.insert(name.clone(), PackageInstance { name: name.clone(), version: ver, dependencies: deps.clone() });
            for (dn, _) in deps { roots.push_back(dn); }
        }
    }

    // Prune any unreachable lock entries and delete directories
    let trans_removed = prune_unreachable(&mut lock, &instances);
    let mut to_delete = actually_removed.clone();
    to_delete.extend(trans_removed.into_iter());
    if !to_delete.is_empty() {
        remove_dirs(&to_delete);
    }

    // Write lockfile or delete if empty
    lockfile::write(&lock, lock_path.clone())?;
    if lockfile_has_no_packages(&lock) {
        let _ = std::fs::remove_file(&lock_path);
    }
    cleanup_empty_node_modules_dir();

    // Output summary
    for n in &actually_removed {
        if let Some(ver) = lock.packages.get(&format!("node_modules/{}", n)).and_then(|e| e.version.clone()) {
            println!("{gray}[pacm]{reset} {red}-{reset} {name}@{ver}", gray=C_GRAY, red=C_RED, reset=C_RESET, name=n, ver=ver);
        } else {
            println!("{gray}[pacm]{reset} {red}-{reset} {name}", gray=C_GRAY, red=C_RED, reset=C_RESET, name=n);
        }
    }
    let dur = start.elapsed();
    println!(
        "{gray}[pacm]{reset} summary: {red}{rm} removed{reset} in {secs:.2?}",
        gray = C_GRAY,
        red = C_RED,
        rm = to_delete.len(),
        secs = dur,
        reset = C_RESET
    );
    Ok(())
}

fn build_fast_instances(
    manifest: &Manifest,
    lock: &Lockfile,
) -> Option<BTreeMap<String, PackageInstance>> {
    use std::collections::{BTreeMap, HashSet, VecDeque};
    let mut needed: HashSet<String> = HashSet::new();
    for (n, _) in &manifest.dependencies {
        needed.insert(n.clone());
    }
    for (n, _) in &manifest.dev_dependencies {
        needed.insert(n.clone());
    }
    for (n, _) in &manifest.optional_dependencies {
        needed.insert(n.clone());
    }
    if needed.is_empty() {
        return Some(BTreeMap::new());
    }
    let mut queue: VecDeque<String> = needed.iter().cloned().collect();
    // Collect closure of dependencies from lockfile without resolution.
    while let Some(name) = queue.pop_front() {
        let key = format!("node_modules/{}", name);
        if let Some(entry) = lock.packages.get(&key) {
            for dep in entry.dependencies.keys() {
                if needed.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        } else {
            return None;
        }
    }
    // Build instance map ensuring store availability
    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();
    for name in needed.iter() {
        let key = format!("node_modules/{}", name);
        let entry = lock.packages.get(&key)?; // missing -> cannot fast path
    let version = entry.version.clone()?;
    let _ = entry.integrity.as_ref()?;
        if !crate::cache::cache_package_path(&name, &version).exists() {
            return None;
        }
        instances.insert(
            name.clone(),
            PackageInstance {
                name: name.clone(),
                version: version.clone(),
                dependencies: entry.dependencies.clone(),
            },
        );
    }
    Some(instances)
}

fn prune_removed_from_lock(lock: &mut Lockfile, removed: &[String]) {
    for name in removed {
        let key = format!("node_modules/{}", name);
        lock.packages.remove(&key);
    }
    // Also update root dependencies already changed by sync_from_manifest
}

fn prune_unreachable(
    lock: &mut Lockfile,
    instances: &BTreeMap<String, PackageInstance>,
) -> Vec<String> {
    let keep: std::collections::HashSet<String> = instances.keys().cloned().collect();
    let mut to_remove_keys: Vec<String> = Vec::new();
    let mut removed_names: Vec<String> = Vec::new();
    for k in lock.packages.keys() {
        if k.is_empty() {
            continue;
        }
        if let Some(stripped) = k.strip_prefix("node_modules/") {
            if !keep.contains(stripped) {
                to_remove_keys.push(k.clone());
                removed_names.push(stripped.to_string());
            }
        }
    }
    for k in to_remove_keys {
        lock.packages.remove(&k);
    }
    removed_names
}

fn remove_dirs(names: &[String]) {
    use std::fs;
    for n in names {
        let mut p = PathBuf::from("node_modules");
        // Support scoped packages by splitting on '/'
        for part in n.split('/') {
            p = p.join(part);
        }
        if p.exists() {
            let _ = fs::remove_dir_all(&p);
            // If this was a scoped package, try to remove now-empty scope folder
            if let Some(scope_dir) = p.parent() {
                if let Ok(mut rd) = fs::read_dir(scope_dir) {
                    if rd.next().is_none() {
                        let _ = fs::remove_dir(scope_dir);
                    }
                }
            }
        }
    }
    // If node_modules is now empty (and only possibly .pacm remains later), we'll not remove it here.
}

fn cleanup_empty_node_modules_dir() {
    use std::fs;
    let nm = PathBuf::from("node_modules");
    if nm.exists() {
        if let Ok(mut rd) = fs::read_dir(&nm) {
            if rd.next().is_none() {
                let _ = fs::remove_dir(&nm);
            }
        }
    }
}

fn lockfile_has_no_packages(lock: &Lockfile) -> bool {
    if let Some(root) = lock.packages.get("") {
        let only_root = lock.packages.len() == 1;
        let no_deps = root.dependencies.is_empty()
            && root.dev_dependencies.is_empty()
            && root.optional_dependencies.is_empty()
            && root.peer_dependencies.is_empty();
        only_root && no_deps
    } else {
        // No root entry and no packages -> treat as empty
        lock.packages.is_empty()
    }
}

fn perform_download(
    fetcher: &Fetcher,
    name: &str,
    version: &str,
    url: &str,
    downloads: &Arc<Mutex<Vec<DownloadProgress>>>,
) -> Result<Vec<u8>> {
    // First attempt HEAD/metadata—reqwest blocking doesn't easily expose HEAD reused connection, skip and stream directly.
    // We'll stream and if size threshold exceeded show progress entry.
    let entry_index: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let dl_vec_clone = downloads.clone();
    let name_s = name.to_string();
    let ver_s = version.to_string();
    let bytes = fetcher.download_tarball_stream(url, |downloaded, total| {
        // Determine if we should create a progress entry
        if total.unwrap_or(0) > 10 * 1024 * 1024 {
            // >10MB
            let mut idx_lock = entry_index.lock().unwrap();
            if idx_lock.is_none() {
                let mut dls = dl_vec_clone.lock().unwrap();
                dls.push(DownloadProgress {
                    name: name_s.clone(),
                    version: ver_s.clone(),
                    downloaded,
                    total,
                    done: false,
                    started_at: Instant::now(),
                });
                *idx_lock = Some(dls.len() - 1);
            } else {
                let i = idx_lock.unwrap();
                let mut dls = dl_vec_clone.lock().unwrap();
                if let Some(entry) = dls.get_mut(i) {
                    entry.downloaded = downloaded;
                    entry.total = total;
                    if total.map(|t| downloaded >= t).unwrap_or(false) {
                        entry.done = true;
                    }
                }
            }
        }
    })?;
    // Mark done if we had an entry but total unknown until end and large
    if let Some(i) = *entry_index.lock().unwrap() {
        let mut dls = downloads.lock().unwrap();
        if let Some(entry) = dls.get_mut(i) {
            entry.downloaded = entry.total.unwrap_or(entry.downloaded);
            entry.done = true;
        }
    }
    Ok(bytes)
}

fn add_spec_with_version(
    manifest: &mut Manifest,
    name: &str,
    version: &str,
    dev: bool,
    optional: bool,
) -> Result<()> {
    // avoid writing empty keys
    if name.is_empty() {
        anyhow::bail!("empty package name")
    }
    if dev {
        manifest
            .dev_dependencies
            .insert(name.to_string(), version.to_string());
    } else if optional {
        manifest
            .optional_dependencies
            .insert(name.to_string(), version.to_string());
    } else {
        manifest
            .dependencies
            .insert(name.to_string(), version.to_string());
    }
    Ok(())
}

fn node_modules_intact(manifest: &Manifest) -> bool {
    use std::path::PathBuf;
    let node_modules = PathBuf::from("node_modules");
    if !node_modules.exists() {
        return false;
    }
    let pacm_dir = node_modules.join(".pacm");
    if !pacm_dir.exists() {
        return false;
    }
    for (name, _) in &manifest.dependencies {
        if !node_modules.join(name).exists() {
            return false;
        }
    }
    for (name, _) in &manifest.dev_dependencies {
        if !node_modules.join(name).exists() {
            return false;
        }
    }
    for (name, _) in &manifest.optional_dependencies {
        if !node_modules.join(name).exists() {
            return false;
        }
    }
    true
}

fn cmd_list() -> Result<()> {
    let lock_path = PathBuf::from("pacm.lockb");
    let lock = if lock_path.exists() {
        lockfile::load(&lock_path)?
    } else {
        let legacy = PathBuf::from("pacm-lock.json");
        if legacy.exists() {
            let lf = lockfile::load_json_compat(&legacy)?;
            println!("{gray}[pacm]{reset} {yellow}note{reset}: reading legacy pacm-lock.json (run 'pacm install' to migrate)", gray=C_GRAY, yellow=C_YELLOW, reset=C_RESET);
            lf
        } else {
            println!(
                "{gray}[pacm]{reset} {red}error{reset} no lockfile. Run 'pacm install'.",
                gray = C_GRAY,
                red = C_RED,
                reset = C_RESET
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
    for (k, v) in &lock.packages {
        println!(
            "{gray}[pacm]{reset}  {dim}-{reset} {name} => {ver}",
            gray = C_GRAY,
            dim = C_DIM,
            reset = C_RESET,
            name = k,
            ver = v.version.as_deref().unwrap_or("(unresolved)")
        );
    }
    Ok(())
}
