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
        packages: Vec<String>,
    },
    /// Add a dependency (alias for install <pkg>)
    Add { package: String },
    /// List packages from lockfile
    List,
}

impl PacmCli {
    pub fn parse() -> Self { <Self as Parser>::parse() }

    pub fn run(&self) -> Result<()> {
        match &self.command {
            None => { self.print_help(); Ok(()) },
            Some(Commands::Init { name, version }) => cmd_init(name.clone(), version.clone()),
            Some(Commands::Install { packages }) => cmd_install(packages.clone()),
            Some(Commands::Add { package }) => cmd_install(vec![package.clone()]),
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
    println!("Created package.json for {}@{}", manifest.name, manifest.version);
    Ok(())
}

fn cmd_install(specs: Vec<String>) -> Result<()> {
    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() { println!("No package.json found. Run 'pacm init' first."); return Ok(()); }
    let mut manifest = manifest::load(&manifest_path)?;
    if !specs.is_empty() {
        for spec in specs { add_spec(&mut manifest, &spec)?; }
        manifest::write(&manifest, &manifest_path)?;
    }
    let fetcher = Fetcher::new(None)?;
    let resolver = Resolver::new();
    let lock_path = PathBuf::from("pacm-lock.json");
    let mut lock = Lockfile::load_or_default(lock_path.clone())?;
    lock.sync_from_manifest(&manifest);

    #[derive(Clone)]
    struct Task { name: String, range: String }
    let mut queue: VecDeque<Task> = VecDeque::new();
    for (n, r) in &manifest.dependencies { queue.push_back(Task { name: n.clone(), range: r.clone() }); }
    let mut visited_name_version: HashSet<(String,String)> = HashSet::new();

    let start = Instant::now();
    let mut last_line_len = 0usize;
    let mut installed_count = 0usize;

    fn progress(msg: &str, last_len: &mut usize) {
        let mut stdout = io::stdout();
        let pad = if *last_len > msg.len() { *last_len - msg.len() } else { 0 };
        let _ = write!(stdout, "\r{}{}", msg, " ".repeat(pad));
        let _ = stdout.flush();
        *last_len = msg.len();
    }

    // Collect instances for linker instead of linking directly now
    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();

    while let Some(Task { name, range }) = queue.pop_front() {
        if visited_name_version.iter().any(|(n, _)| n == &name) { continue; }
        progress(&format!("Resolving {}@{}", name, range), &mut last_line_len);
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
                    progress(&format!("Downloading {}@{}", name, picked_ver), &mut last_line_len);
                    let bytes = fetcher.download_tarball(&tarball_url)?;
                    progress(&format!("Extracting {}@{}", name, picked_ver), &mut last_line_len);
                    store::ensure_package(&bytes, Some(int_hint))?
                }
            } else {
                progress(&format!("Downloading {}@{}", name, picked_ver), &mut last_line_len);
                let bytes = fetcher.download_tarball(&tarball_url)?;
                progress(&format!("Extracting {}@{}", name, picked_ver), &mut last_line_len);
                store::ensure_package(&bytes, Some(int_hint))?
            }
        } else {
            progress(&format!("Downloading {}@{}", name, picked_ver), &mut last_line_len);
            let bytes = fetcher.download_tarball(&tarball_url)?;
            progress(&format!("Extracting {}@{}", name, picked_ver), &mut last_line_len);
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
    // Perform linking phase (virtual store + facade)
    progress("Linking graph", &mut last_line_len);
    let linker = Linker::new();
    linker.link_project(&std::env::current_dir()?, &instances.into_iter().collect(), &manifest.dependencies)?;
    lockfile::write(&lock, lock_path)?;
    let dur = start.elapsed();
    // Move to new line before summary
    println!("\rInstalled {} packages in {:.2?}        ", installed_count, dur);
    Ok(())
}

fn add_spec(manifest: &mut Manifest, spec: &str) -> Result<()> {
    // Parse <name>@<range?>
    let (name, range) = if let Some((n, r)) = spec.split_once('@') {
        (n.to_string(), if r.is_empty() { "*".to_string() } else { r.to_string() })
    } else { (spec.to_string(), "*".to_string()) };
    manifest.dependencies.insert(name, range);
    Ok(())
}

fn cmd_list() -> Result<()> {
    let lock_path = PathBuf::from("pacm-lock.json");
    if !lock_path.exists() {
        println!("No lockfile. Run 'pacm install'.");
        return Ok(());
    }
    let lock = lockfile::load(&lock_path)?;
    println!("Packages ({} entries):", lock.packages.len());
    for (k, v) in &lock.packages {
        println!(" - {} => {}", k, v.version.as_deref().unwrap_or("(unresolved)"));
    }
    Ok(())
}
