use crate::manifest::{self, Manifest};
use crate::lockfile::{self, Lockfile, PackageEntry};
use crate::fetch::Fetcher;
use crate::resolver::{Resolver, map_versions};
use crate::store;
use crate::linker::{Linker, PackageInstance};
use anyhow::Context;
use std::collections::{VecDeque, HashSet, BTreeMap};
use std::time::Instant;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use once_cell::sync::Lazy;
use clap::{Parser, Subcommand};
use anyhow::{Result, bail};
use std::path::PathBuf;
// (imports consolidated above)

#[derive(Parser, Debug)]
#[command(name = "pacm", version, about = "Prototype JavaScript package manager (Rust)")]
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
    /// Install all deps or add specific packages (placeholder network)
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
}

impl PacmCli {
    pub fn parse() -> Self { <Self as Parser>::parse() }

    pub fn run(&self) -> Result<()> {
        match &self.command {
            None => { self.print_help(); Ok(()) },
            Some(Commands::Init { name, version }) => cmd_init(name.clone(), version.clone()),
            Some(Commands::Install { packages, dev, optional, no_save, exact }) => cmd_install(packages.clone(), *dev, *optional, *no_save, *exact),
            Some(Commands::Add { package, dev, optional, no_save, exact }) => cmd_install(vec![package.clone()], *dev, *optional, *no_save, *exact),
            Some(Commands::List) => cmd_list(),
        }
    }

    fn print_help(&self) {
        println!("pacm - prototype package manager (Rust)\n");
        println!("Commands:\n  init\n  install [pkg..]\n  add <pkg>\n  list");
    }
}

fn cmd_init(name: Option<String>, version: Option<String>) -> Result<()> {
    let path = PathBuf::from("package.json");
    if path.exists() {
        bail!("package.json already exists");
    }
    let manifest = Manifest::new(name.unwrap_or_else(|| "my-app".into()), version.unwrap_or_else(|| "0.1.0".into()));
    manifest::write(&manifest, &path)?;
    println!("{gray}[pacm]{reset} {green}init{reset} created {name}@{ver}", gray=C_GRAY, reset=C_RESET, green=C_GREEN, name=manifest.name, ver=manifest.version);
    Ok(())
}

// ANSI color helpers (simple, no dependency). If not supported, the raw codes will appear; modern Windows supports them.
const C_RESET: &str = "\x1b[0m";
const C_DIM: &str = "\x1b[2m";
const C_CYAN: &str = "\x1b[36m";
const C_GREEN: &str = "\x1b[32m"; // keep for actions
const C_MAGENTA: &str = "\x1b[35m";
const C_YELLOW: &str = "\x1b[33m";
const C_RED: &str = "\x1b[31m";
const C_GRAY: &str = "\x1b[90m"; // prefix color

// Global progress handle + warning injector so other modules can surface warnings above dynamic line.
pub(crate) struct ProgressRenderer { last_len: usize, last_status: String }
impl ProgressRenderer { fn new() -> Self { Self { last_len: 0, last_status: String::new() } } }

static PROGRESS_HANDLE: Lazy<Mutex<Option<Arc<Mutex<ProgressRenderer>>>>> = Lazy::new(|| Mutex::new(None));

pub(crate) fn set_progress_handle(h: Arc<Mutex<ProgressRenderer>>) { *PROGRESS_HANDLE.lock().unwrap() = Some(h); }
pub(crate) fn clear_progress_handle() { *PROGRESS_HANDLE.lock().unwrap() = None; }

pub(crate) fn record_warning(msg: impl Into<String>) {
    let msg = msg.into();
    let handle_opt = PROGRESS_HANDLE.lock().unwrap().clone();
    if let Some(h) = handle_opt { // Print warning on its own line, then re-render status
        let r = h.lock().unwrap();
    print!("\r\n{gray}[pacm]{reset} {yellow}warning{reset}: {msg}\n", gray=C_GRAY, reset=C_RESET, yellow=C_YELLOW, msg=msg);
        let pad = if r.last_len > r.last_status.len() { r.last_len - r.last_status.len() } else { 0 };
        print!("{}{}", r.last_status, " ".repeat(pad));
        io::stdout().flush().ok();
    } else {
    eprintln!("[pacm] warning: {}", msg);
    }
}

fn render_status(pr: &mut ProgressRenderer, raw: impl Into<String>) {
    let raw = raw.into();
    pr.last_status = raw.clone();
    let mut out = io::stdout();
    let pad = if pr.last_len > raw.len() { pr.last_len - raw.len() } else { 0 };
    write!(out, "\r{}{}", raw, " ".repeat(pad)).ok();
    out.flush().ok();
    pr.last_len = raw.len();
}

fn format_status(kind: &str, detail: &str) -> String {
    let (color, action) = match kind { "resolving" => (C_CYAN, "resolving"), "downloading" => (C_CYAN, "downloading"), "extracting" => (C_MAGENTA, "extracting"), "linking" => (C_GREEN, "linking"), "fast" => (C_GREEN, "fast"), _ => (C_DIM, kind) };
    format!("{gray}[pacm]{reset} {color}{action}{reset} {detail}", gray=C_GRAY, reset=C_RESET, color=color, action=action, detail=detail)
}

fn finish_progress(pr: &mut ProgressRenderer) { println!(); pr.last_len = 0; }

// Represents an in-flight download progress (for large packages >10MB)
#[derive(Clone, Debug)]
struct DownloadProgress { name: String, version: String, downloaded: u64, total: Option<u64>, done: bool }

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B","KiB","MiB","GiB","TiB"];
    let mut v = bytes as f64; let mut i = 0usize; while v > 1024.0 && i < UNITS.len()-1 { v/=1024.0; i+=1; }
    if i==0 { format!("{}{}", bytes, UNITS[i]) } else { format!("{:.1}{}", v, UNITS[i]) }
}

fn cmd_install(specs: Vec<String>, dev: bool, optional: bool, no_save: bool, _exact: bool) -> Result<()> {
    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() { println!("{gray}[pacm]{reset} {red}error{reset} no package.json found. Run 'pacm init' first.", gray=C_GRAY, red=C_RED, reset=C_RESET); return Ok(()); }
    let mut manifest = manifest::load(&manifest_path)?;
    // If user provided package specs on the command line, resolve versions and optionally persist
    if !specs.is_empty() {
        // We'll resolve any provided specs to concrete versions (using latest dist-tag when no range specified)
        for spec in &specs {
            // Parse package spec allowing scoped names. Strategy:
            // - If spec contains '@' but starts with '@' (scoped) we look for the last '@' to split version, e.g. @scope/name@1.2.3
            let (name, req) = if spec.starts_with('@') {
                if let Some(idx) = spec.rfind('@') {
                    if idx == 0 { (spec.to_string(), "*".to_string()) } else { let (n, r) = spec.split_at(idx); (n.to_string(), r[1..].to_string()) }
                } else { (spec.to_string(), "*".to_string()) }
            } else {
                if let Some((n, r)) = spec.split_once('@') { (n.to_string(), if r.is_empty() { "*".to_string() } else { r.to_string() }) } else { (spec.to_string(), "*".to_string()) }
            };

            // If no-save is false, we will write the resolved version into package.json; otherwise only resolve and install
            let resolved_version = if req == "*" {
                // fetch metadata to get dist-tags.latest
                let fetcher = Fetcher::new(None)?;
                let meta = fetcher.package_metadata(&name).with_context(|| format!("fetch metadata for {}", name))?;
                if let Some(tags) = &meta.dist_tags {
                    if let Some(latest) = &tags.latest { latest.clone() } else { "*".to_string() }
                } else { "*".to_string() }
            } else { req.clone() };

            if !no_save {
                add_spec_with_version(&mut manifest, &name, &resolved_version, dev, optional)?;
            }
        }
        if !no_save { manifest::write(&manifest, &manifest_path)?; }
    }
    // Lock handling prior to potential fast path
    let lock_path = PathBuf::from("pacm-lock.json");
    let mut lock = Lockfile::load_or_default(lock_path.clone())?;
    // Copy for diffing + no-op detection
    let original_lock = lock.clone();

    // Capture old root dependency map (pre-sync)
    let old_root_deps: std::collections::BTreeMap<String,String> = original_lock.packages.get("")
        .map(|p| p.dependencies.clone()).unwrap_or_default();

    // Sync lock root + placeholder entries for new deps
    lock.sync_from_manifest(&manifest);
    let new_root_deps: std::collections::BTreeMap<String,String> = lock.packages.get("")
        .map(|p| p.dependencies.clone()).unwrap_or_default();

    // Determine added / removed top-level dependencies (by name)
    use std::collections::BTreeSet;
    let old_names: BTreeSet<_> = old_root_deps.keys().cloned().collect();
    let new_names: BTreeSet<_> = new_root_deps.keys().cloned().collect();
    let added_root: Vec<String> = new_names.difference(&old_names).cloned().collect();
    let removed_root: Vec<String> = old_names.difference(&new_names).cloned().collect();

    // If lockfile didn't change and node_modules/.pacm appear intact, behave like pnpm's no-op
    if lock == original_lock && added_root.is_empty() && removed_root.is_empty() && node_modules_intact(&manifest) {
    println!("{gray}[pacm]{reset} {dim}no dependency changes{reset}", gray=C_GRAY, dim=C_DIM, reset=C_RESET);
    println!("{gray}[pacm]{reset} {dim}0 added, 0 removed{reset}", gray=C_GRAY, dim=C_DIM, reset=C_RESET);
    println!("{gray}[pacm]{reset} {green}already up to date{reset}", gray=C_GRAY, green=C_GREEN, reset=C_RESET);
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
                    prune_unreachable(&mut lock, &instances);
                }
            let start = Instant::now();
            let mut merged_root_deps: BTreeMap<String, String> = BTreeMap::new();
            for (n, r) in &manifest.dependencies { merged_root_deps.insert(n.clone(), r.clone()); }
            for (n, r) in &manifest.dev_dependencies { merged_root_deps.insert(n.clone(), r.clone()); }
            for (n, r) in &manifest.optional_dependencies { merged_root_deps.insert(n.clone(), r.clone()); }
            // Setup progress renderer so warnings appear above status
            let progress = Arc::new(Mutex::new(ProgressRenderer::new()));
            set_progress_handle(progress.clone());
            {
                let mut pr = progress.lock().unwrap();
                render_status(&mut pr, format_status("fast", "link: using cached store; skipping resolution"));
            }
            let linker = Linker::new();
            let as_hash: std::collections::HashMap<String, PackageInstance> = instances.into_iter().collect();
            linker.link_project(&std::env::current_dir()?, &as_hash, &merged_root_deps)?;
            // Finish status line
            {
                let mut pr = progress.lock().unwrap();
                finish_progress(&mut pr);
            }
            clear_progress_handle();
                // Write updated (pruned) lockfile
                lockfile::write(&lock, lock_path.clone())?;
                let dur = start.elapsed();
                // Summary lines
                if added_root.is_empty() && removed_root.is_empty() { println!("{gray}[pacm]{reset} {dim}no dependency changes{reset}", gray=C_GRAY, dim=C_DIM, reset=C_RESET); }
                for r in &removed_root {
                    if let Some(ver) = original_lock.packages.get(&format!("node_modules/{}", r)).and_then(|e| e.version.as_ref()) {
                        println!("{gray}[pacm]{reset} {red}-{reset} {name}@{ver}", gray=C_GRAY, red=C_RED, reset=C_RESET, name=r, ver=ver);
                    } else {
                        println!("{gray}[pacm]{reset} {red}-{reset} {name}", gray=C_GRAY, red=C_RED, reset=C_RESET, name=r);
                    }
                }
                let total = as_hash.len();
                println!("{gray}[pacm]{reset} summary: {green}0 added{reset}, {red}{rm} removed{reset}", gray=C_GRAY, green=C_GREEN, red=C_RED, rm=removed_root.len(), reset=C_RESET);
                println!("{gray}[pacm]{reset} {green}linked{reset} {total} packages (all cached) in {secs:.2?}", gray=C_GRAY, green=C_GREEN, reset=C_RESET, total=total, secs=dur);
                return Ok(());
            }
        }
    }

    // Proceed with full resolution path
    let fetcher = Fetcher::new(None)?;
    let resolver = Resolver::new();
    #[derive(Clone)]
    struct Task { name: String, range: String }
    let mut queue: VecDeque<Task> = VecDeque::new();
    for (n, r) in &manifest.dependencies { queue.push_back(Task { name: n.clone(), range: r.clone() }); }
    for (n, r) in &manifest.dev_dependencies { queue.push_back(Task { name: n.clone(), range: r.clone() }); }
    for (n, r) in &manifest.optional_dependencies { queue.push_back(Task { name: n.clone(), range: r.clone() }); }
    let mut visited_name_version: HashSet<(String,String)> = HashSet::new();

    let start = Instant::now();
    let mut installed_count = 0usize;
    use std::sync::atomic::{AtomicBool, Ordering};
    let progress = Arc::new(Mutex::new(ProgressRenderer::new()));
    set_progress_handle(progress.clone());
    let downloads: Arc<Mutex<Vec<DownloadProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let progress_clone = progress.clone();
    let downloads_clone = downloads.clone();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_thread = stop_flag.clone();

    let painter = thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(100));
            let mut pr = progress_clone.lock().unwrap();
            let dl = downloads_clone.lock().unwrap();
            let mut active_lines = Vec::new();
            for d in dl.iter() { if !d.done { active_lines.push(format!("{}@{} {} / {}", d.name, d.version, human_size(d.downloaded), d.total.map(human_size).unwrap_or_else(||"?".into()))); } }
            if active_lines.is_empty() {
                if stop_flag_thread.load(Ordering::SeqCst) { break; }
                continue;
            }
            render_status(&mut pr, active_lines.join(" | "));
            if dl.iter().all(|d| d.done) && stop_flag_thread.load(Ordering::SeqCst) { break; }
        }
    });

    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();

    while let Some(Task { name, range }) = queue.pop_front() {
        if visited_name_version.iter().any(|(n, _)| n == &name) { continue; }
        {
            let mut pr = progress.lock().unwrap();
            render_status(&mut pr, format_status("resolving", &format!("{}@{}", name, range)));
        }
        let meta = fetcher.package_metadata(&name).with_context(|| format!("fetch metadata for {}", name))?;
        let version_map = map_versions(&meta);
        let (picked_ver, tarball_url) = resolver.pick_version(&version_map, &range)?;
        if visited_name_version.contains(&(name.clone(), picked_ver.to_string())) { continue; }
        let version_meta = meta.versions.get(&picked_ver.to_string()).expect("version meta");
        let integrity_hint = version_meta.dist.integrity.as_deref();
        let mut reused = false;
        let (hex, integrity) = if let Some(int_hint) = integrity_hint {
            if let Some(existing_hex) = store::exists_by_integrity(int_hint) {
                if store::package_path(&existing_hex).exists() {
                    reused = true;
                    (existing_hex, int_hint.to_string())
                } else {
                    { let mut pr = progress.lock().unwrap(); render_status(&mut pr, format_status("downloading", &format!("{}@{}", name, picked_ver))); }
                    let bytes = perform_download(&fetcher, &name, &picked_ver.to_string(), &tarball_url, &downloads)?;
                    { let mut pr = progress.lock().unwrap(); render_status(&mut pr, format_status("extracting", &format!("{}@{}", name, picked_ver))); }
                    store::ensure_package(&bytes, Some(int_hint))?
                }
            } else {
                { let mut pr = progress.lock().unwrap(); render_status(&mut pr, format_status("downloading", &format!("{}@{}", name, picked_ver))); }
                let bytes = perform_download(&fetcher, &name, &picked_ver.to_string(), &tarball_url, &downloads)?;
                { let mut pr = progress.lock().unwrap(); render_status(&mut pr, format_status("extracting", &format!("{}@{}", name, picked_ver))); }
                store::ensure_package(&bytes, Some(int_hint))?
            }
        } else {
            { let mut pr = progress.lock().unwrap(); render_status(&mut pr, format_status("downloading", &format!("{}@{}", name, picked_ver))); }
            let bytes = perform_download(&fetcher, &name, &picked_ver.to_string(), &tarball_url, &downloads)?;
            { let mut pr = progress.lock().unwrap(); render_status(&mut pr, format_status("extracting", &format!("{}@{}", name, picked_ver))); }
            store::ensure_package(&bytes, None)?
        };
        // Defer linking – build instance map
        let key = format!("node_modules/{}", name);
        let mut dep_map: BTreeMap<String, String> = BTreeMap::new();
        for (dn, dr) in &version_meta.dependencies { dep_map.insert(dn.clone(), dr.clone()); }
        let entry = lock.packages.entry(key).or_insert(PackageEntry { version: None, integrity: None, resolved: None, dependencies: Default::default() });
        entry.version = Some(picked_ver.to_string());
        entry.integrity = Some(integrity);
        entry.resolved = Some(tarball_url);
        entry.dependencies = dep_map.clone();
        instances.insert(name.clone(), PackageInstance { name: name.clone(), version: picked_ver.to_string(), dependencies: dep_map.clone(), store_hex: hex.clone() });
        visited_name_version.insert((name.clone(), picked_ver.to_string()));
    if !reused { installed_count += 1; }
        for (dn, dr) in dep_map { queue.push_back(Task { name: dn, range: dr }); }
    }
    // Before linking phase: clear dynamic line so upcoming warnings stack above future linking status
    {
        let mut pr = progress.lock().unwrap();
        if pr.last_len > 0 { print!("\r{}\r", " ".repeat(pr.last_len)); io::stdout().flush().ok(); }
        pr.last_status.clear(); pr.last_len = 0;
    }
    // Before linking: prune unreachable entries (we have full graph in instances)
    prune_unreachable(&mut lock, &instances);
    // Perform linking phase (virtual store + facade)
    let linker = Linker::new();
    // root_deps should include dependencies, devDependencies and optionalDependencies so added dev/optional packages are linked
    let mut merged_root_deps: BTreeMap<String, String> = BTreeMap::new();
    for (n, r) in &manifest.dependencies { merged_root_deps.insert(n.clone(), r.clone()); }
    for (n, r) in &manifest.dev_dependencies { merged_root_deps.insert(n.clone(), r.clone()); }
    for (n, r) in &manifest.optional_dependencies { merged_root_deps.insert(n.clone(), r.clone()); }
    let total_packages_for_summary = instances.len();
    let instances_for_link: std::collections::HashMap<String, PackageInstance> = instances.clone().into_iter().collect();
    linker.link_project(&std::env::current_dir()?, &instances_for_link, &merged_root_deps)?;
    lockfile::write(&lock, lock_path.clone())?;
    let dur = start.elapsed();
    // Move to new line before summary
    // Ensure painter terminates (if active). Give it a moment.
    // Mark painter stop (downloads finished) and show final linking status once *after* warnings.
    stop_flag.store(true, Ordering::SeqCst);
    {
        let mut pr = progress.lock().unwrap();
        render_status(&mut pr, format_status("linking", "graph"));
        finish_progress(&mut pr);
    }
    painter.join().ok();
    clear_progress_handle();
    // Compute summary data
    let total = total_packages_for_summary;
    let reused = total.saturating_sub(installed_count);
    // Added root packages now have versions resolved (if any)
    if added_root.is_empty() && removed_root.is_empty() { println!("{gray}[pacm]{reset} {dim}no dependency changes{reset}", gray=C_GRAY, dim=C_DIM, reset=C_RESET); }
    for a in &added_root {
    if let Some(inst) = instances.get(a) { println!("{gray}[pacm]{reset} {green}+{reset} {name}@{ver}", gray=C_GRAY, green=C_GREEN, reset=C_RESET, name=a, ver=inst.version); } else { println!("{gray}[pacm]{reset} {green}+{reset} {name}", gray=C_GRAY, green=C_GREEN, reset=C_RESET, name=a); }
    }
    for r in &removed_root {
        if let Some(ver) = original_lock.packages.get(&format!("node_modules/{}", r)).and_then(|e| e.version.as_ref()) {
            println!("{gray}[pacm]{reset} {red}-{reset} {name}@{ver}", gray=C_GRAY, red=C_RED, reset=C_RESET, name=r, ver=ver);
        } else { println!("{gray}[pacm]{reset} {red}-{reset} {name}", gray=C_GRAY, red=C_RED, reset=C_RESET, name=r); }
    }
    println!("{gray}[pacm]{reset} summary: {green}{add} added{reset}, {red}{rm} removed{reset}", gray=C_GRAY, green=C_GREEN, red=C_RED, add=added_root.len(), rm=removed_root.len(), reset=C_RESET);
    println!("{gray}[pacm]{reset} {green}installed{reset} {total} packages ({green}{dl} downloaded{reset}, {dim}{re} reused{reset}) in {secs:.2?}", gray=C_GRAY, green=C_GREEN, dim=C_DIM, reset=C_RESET, total=total, dl=installed_count, re=reused, secs=dur);
    Ok(())
}

// Attempt to build instances map purely from lockfile + store contents.
fn build_fast_instances(manifest: &Manifest, lock: &Lockfile) -> Option<BTreeMap<String, PackageInstance>> {
    use std::collections::{BTreeMap, VecDeque, HashSet};
    let mut needed: HashSet<String> = HashSet::new();
    for (n, _) in &manifest.dependencies { needed.insert(n.clone()); }
    for (n, _) in &manifest.dev_dependencies { needed.insert(n.clone()); }
    for (n, _) in &manifest.optional_dependencies { needed.insert(n.clone()); }
    if needed.is_empty() { return Some(BTreeMap::new()); }
    let mut queue: VecDeque<String> = needed.iter().cloned().collect();
    // Collect closure of dependencies from lockfile without resolution.
    while let Some(name) = queue.pop_front() {
        let key = format!("node_modules/{}", name);
        if let Some(entry) = lock.packages.get(&key) {
            for dep in entry.dependencies.keys() { if needed.insert(dep.clone()) { queue.push_back(dep.clone()); } }
        } else { return None; }
    }
    // Build instance map ensuring store availability
    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();
    for name in needed.iter() {
        let key = format!("node_modules/{}", name);
        let entry = lock.packages.get(&key)?; // missing -> cannot fast path
        let version = entry.version.clone()?;
        let integrity = entry.integrity.clone()?;
        let hex = match store::exists_by_integrity(&integrity) { Some(h) => h, None => return None };
        if !store::package_path(&hex).exists() { return None; }
        instances.insert(name.clone(), PackageInstance { name: name.clone(), version, dependencies: entry.dependencies.clone(), store_hex: hex });
    }
    Some(instances)
}

// Remove removed root dependencies from lock packages
fn prune_removed_from_lock(lock: &mut Lockfile, removed: &[String]) {
    for name in removed {
        let key = format!("node_modules/{}", name);
        lock.packages.remove(&key);
    }
    // Also update root dependencies already changed by sync_from_manifest
}

// Prune any lockfile package entries not present in the current resolved instance map (excluding root entry "")
fn prune_unreachable(lock: &mut Lockfile, instances: &BTreeMap<String, PackageInstance>) {
    let keep: std::collections::HashSet<String> = instances.keys().cloned().collect();
    let mut to_remove: Vec<String> = Vec::new();
    for k in lock.packages.keys() { if k.is_empty() { continue; } if let Some(stripped) = k.strip_prefix("node_modules/") { if !keep.contains(stripped) { to_remove.push(k.clone()); } } }
    for k in to_remove { lock.packages.remove(&k); }
}

fn remove_dirs(names: &[String]) {
    use std::fs;
    for n in names {
        let p = PathBuf::from("node_modules").join(n);
        if p.exists() { let _ = fs::remove_dir_all(&p); }
    }
}

// Perform a download with optional progress (large packages >10MB)
fn perform_download(fetcher: &Fetcher, name: &str, version: &str, url: &str, downloads: &Arc<Mutex<Vec<DownloadProgress>>>) -> Result<Vec<u8>> {
    // First attempt HEAD/metadata—reqwest blocking doesn't easily expose HEAD reused connection, skip and stream directly.
    // We'll stream and if size threshold exceeded show progress entry.
    let entry_index: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let dl_vec_clone = downloads.clone();
    let name_s = name.to_string(); let ver_s = version.to_string();
    let bytes = fetcher.download_tarball_stream(url, |downloaded, total| {
        // Determine if we should create a progress entry
        if total.unwrap_or(0) > 10 * 1024 * 1024 { // >10MB
            let mut idx_lock = entry_index.lock().unwrap();
            if idx_lock.is_none() {
                let mut dls = dl_vec_clone.lock().unwrap();
                dls.push(DownloadProgress { name: name_s.clone(), version: ver_s.clone(), downloaded, total, done: false });
                *idx_lock = Some(dls.len()-1);
            } else {
                let i = idx_lock.unwrap();
                let mut dls = dl_vec_clone.lock().unwrap();
                if let Some(entry) = dls.get_mut(i) { entry.downloaded = downloaded; entry.total = total; if total.map(|t| downloaded>=t).unwrap_or(false) { entry.done = true; } }
            }
        }
    })?;
    // Mark done if we had an entry but total unknown until end and large
    if let Some(i) = *entry_index.lock().unwrap() {
        let mut dls = downloads.lock().unwrap();
        if let Some(entry) = dls.get_mut(i) { entry.downloaded = entry.total.unwrap_or(entry.downloaded); entry.done = true; }
    }
    Ok(bytes)
}

fn add_spec_with_version(manifest: &mut Manifest, name: &str, version: &str, dev: bool, optional: bool) -> Result<()> {
    // avoid writing empty keys
    if name.is_empty() { anyhow::bail!("empty package name") }
    if dev {
        manifest.dev_dependencies.insert(name.to_string(), version.to_string());
    } else if optional {
        manifest.optional_dependencies.insert(name.to_string(), version.to_string());
    } else {
        manifest.dependencies.insert(name.to_string(), version.to_string());
    }
    Ok(())
}

fn node_modules_intact(manifest: &Manifest) -> bool {
    use std::path::PathBuf;
    let node_modules = PathBuf::from("node_modules");
    if !node_modules.exists() { return false; }
    let pacm_dir = node_modules.join(".pacm");
    if !pacm_dir.exists() { return false; }
    for (name, _) in &manifest.dependencies {
        if !node_modules.join(name).exists() { return false; }
    }
    for (name, _) in &manifest.dev_dependencies {
        if !node_modules.join(name).exists() { return false; }
    }
    for (name, _) in &manifest.optional_dependencies {
        if !node_modules.join(name).exists() { return false; }
    }
    true
}

fn cmd_list() -> Result<()> {
    let lock_path = PathBuf::from("pacm-lock.json");
    if !lock_path.exists() {
        println!("{gray}[pacm]{reset} {red}error{reset} no lockfile. Run 'pacm install'.", gray=C_GRAY, red=C_RED, reset=C_RESET);
        return Ok(());
    }
    let lock = lockfile::load(&lock_path)?;
    println!("{gray}[pacm]{reset} packages ({count} entries):", gray=C_GRAY, reset=C_RESET, count=lock.packages.len());
    for (k, v) in &lock.packages {
        println!("{gray}[pacm]{reset}  {dim}-{reset} {name} => {ver}", gray=C_GRAY, dim=C_DIM, reset=C_RESET, name=k, ver=v.version.as_deref().unwrap_or("(unresolved)"));
    }
    Ok(())
}
