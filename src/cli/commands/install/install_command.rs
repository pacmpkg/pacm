use super::fast::build_fast_instances;
use super::manifest_updates::{parse_spec, update_manifest_for_specs};
use super::node_modules::node_modules_intact;
use super::platform::platform_supported;
use super::progress::{format_status, ProgressRenderer};
use super::prune::{
    cleanup_empty_node_modules_dir, lockfile_has_no_packages, prune_removed_from_lock,
    prune_unreachable, remove_dirs,
};
use crate::cache::{CachedManifest, CasStore, DependencyFingerprint, EnsureParams, StoreEntry};
use crate::colors::*;
use crate::fetch::Fetcher;
use crate::installer::{InstallMode, InstallPlanEntry, Installer, PackageInstance};
use crate::lockfile::{self, Lockfile, PackageEntry};
use crate::manifest;
use crate::resolver::spec::PackageSpec;
use crate::workspaces::{discover_workspaces, workspace_dep_satisfies, WorkspaceInfo};
use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tar::Archive;

use rayon::prelude::*;

fn ensure_lock_entry<'a>(lock: &'a mut Lockfile, name: &str) -> &'a mut PackageEntry {
    let key = format!("node_modules/{name}");
    lock.packages.entry(key).or_insert(PackageEntry {
        version: None,
        integrity: None,
        resolved: None,
        dependencies: BTreeMap::new(),
        dev_dependencies: BTreeMap::new(),
        optional_dependencies: BTreeMap::new(),
        peer_dependencies: BTreeMap::new(),
        peer_dependencies_meta: BTreeMap::new(),
        os: Vec::new(),
        cpu_arch: Vec::new(),
        store_key: None,
        content_hash: None,
        link_mode: None,
        store_path: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn write_lock_entry(
    lock: &mut Lockfile,
    name: &str,
    version: &str,
    integrity: Option<&str>,
    resolved: Option<&str>,
    dependencies: &BTreeMap<String, String>,
    dev_dependencies: &BTreeMap<String, String>,
    optional_dependencies: &BTreeMap<String, String>,
    peer_dependencies: &BTreeMap<String, String>,
    peer_meta: &BTreeMap<String, crate::lockfile::PeerMeta>,
    os: &[String],
    cpu_arch: &[String],
) {
    let entry = ensure_lock_entry(lock, name);
    entry.version = Some(version.to_string());
    entry.integrity = integrity.map(|s| s.to_string());
    entry.resolved = resolved.map(|s| s.to_string());
    entry.dependencies = dependencies.clone();
    entry.dev_dependencies = dev_dependencies.clone();
    entry.optional_dependencies = optional_dependencies.clone();
    entry.peer_dependencies = peer_dependencies.clone();
    entry.peer_dependencies_meta = peer_meta.clone();
    entry.os = os.to_vec();
    entry.cpu_arch = cpu_arch.to_vec();
    entry.store_key = None;
    entry.content_hash = None;
    entry.link_mode = None;
    entry.store_path = None;
}

#[derive(Clone)]
struct PendingDownload {
    name: String,
    version: String,
    url: String,
    integrity_hint: Option<String>,
    scripts: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone)]
struct GithubResolved {
    tarball_url: String,
    commit: String,
}

fn resolve_github_tarball(spec: &crate::resolver::spec::GithubSpec) -> Result<GithubResolved> {
    #[derive(serde::Deserialize)]
    struct RepoInfo {
        default_branch: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct CommitInfo {
        sha: String,
    }

    let client = crate::fetch::http_client();
    let base = format!("https://api.github.com/repos/{}/{}", spec.owner, spec.repo);
    let reference = if let Some(r) = &spec.reference {
        r.clone()
    } else {
        let resp = client.get(&base).send().with_context(|| format!("GET {base}"))?;
        if resp.status().is_success() {
            let info: RepoInfo = resp.json()?;
            info.default_branch.unwrap_or_else(|| "main".to_string())
        } else {
            // Fall back to common defaults if API rate limits or errors
            "main".to_string()
        }
    };

    let commit_url = format!("{base}/commits/{reference}");
    let resp = client.get(&commit_url).send().with_context(|| format!("GET {commit_url}"))?;
    if !resp.status().is_success() {
        // Last-resort fallback to master if main/default failed
        if reference != "master" {
            let fallback_url = format!("{base}/commits/master");
            let resp_fb =
                client.get(&fallback_url).send().with_context(|| format!("GET {fallback_url}"))?;
            if resp_fb.status().is_success() {
                let commit: CommitInfo = resp_fb.json()?;
                let tarball_url = format!(
                    "https://codeload.github.com/{}/{}/tar.gz/{}",
                    spec.owner, spec.repo, commit.sha
                );
                return Ok(GithubResolved { tarball_url, commit: commit.sha });
            }
        }
        anyhow::bail!("failed to resolve GitHub ref {reference} for {}/{}", spec.owner, spec.repo);
    }

    let commit: CommitInfo = resp.json()?;
    let tarball_url =
        format!("https://codeload.github.com/{}/{}/tar.gz/{}", spec.owner, spec.repo, commit.sha);
    Ok(GithubResolved { tarball_url, commit: commit.sha })
}

fn read_manifest_from_tarball(bytes: &[u8]) -> Result<CachedManifest> {
    let gz = GzDecoder::new(bytes);
    let mut ar = Archive::new(gz);
    for entry in ar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.file_name().map(|n| n == "package.json").unwrap_or(false) {
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            let mf: CachedManifest = serde_json::from_str(&buf)?;
            return Ok(mf);
        }
    }
    anyhow::bail!("package.json not found in tarball")
}

fn short_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{:02x}", byte));
    }
    hex.chars().take(8).collect()
}

fn append_build(base: &str, build_tag: &str) -> String {
    if base.contains('+') {
        format!("{base}.{build_tag}")
    } else {
        format!("{base}+{build_tag}")
    }
}

fn write_scripts_sidecar(package: &str, version: &str, scripts: &BTreeMap<String, String>) {
    if scripts.is_empty() {
        return;
    }
    let sidecar = crate::cache::cache_package_path(package, version).join(".registry-scripts.json");
    if let Ok(txt) = serde_json::to_string_pretty(scripts) {
        let _ = std::fs::write(sidecar, txt);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InstallOptions {
    pub dev: bool,
    pub optional: bool,
    pub no_save: bool,
    pub exact: bool,
    pub prefer_offline: bool,
    pub no_progress: bool,
    pub link: bool,
    pub copy: bool,
}

fn download_into_cache(
    fetcher: &Fetcher,
    name: &str,
    version: &str,
    url: &str,
    integrity_hint: Option<&str>,
    scripts: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<String> {
    let bytes = fetcher
        .download_tarball(url)
        .with_context(|| format!("download tarball for {name}@{version}"))?;
    let integrity = crate::cache::ensure_cached_package(name, version, &bytes, integrity_hint)?;
    // write registry scripts sidecar if provided
    if let Some(s) = scripts {
        let cache_path = crate::cache::cache_package_path(name, version);
        let sidecar = cache_path.join(".registry-scripts.json");
        if let Ok(txt) = serde_json::to_string_pretty(s) {
            let _ = std::fs::write(&sidecar, txt);
        }
    }
    Ok(integrity)
}

pub(crate) fn cmd_install(specs: Vec<String>, options: InstallOptions) -> Result<()> {
    let InstallOptions {
        dev,
        optional,
        no_save,
        exact: _exact,
        prefer_offline,
        no_progress,
        link,
        copy,
    } = options;
    let project_root = std::env::current_dir()?;
    let manifest_path = project_root.join("package.json");
    if !manifest_path.exists() {
        println!("{C_GRAY}[pacm]{C_RESET} {C_RED}error{C_RESET} no package.json found. Run 'pacm init' first.");
        return Ok(());
    }
    let mut manifest = manifest::load(&manifest_path)?;
    let workspaces_vec = discover_workspaces(&project_root, &manifest)?;
    let mut workspace_map: BTreeMap<String, WorkspaceInfo> = BTreeMap::new();
    for ws in workspaces_vec {
        workspace_map.insert(ws.name.clone(), ws);
    }
    let workspace_names: Vec<String> = workspace_map.keys().cloned().collect();

    update_manifest_for_specs(&specs, &mut manifest, &manifest_path, dev, optional, no_save)?;

    let lock_path = project_root.join("pacm.lockb");
    let mut lock = if lock_path.exists() {
        Lockfile::load_or_default(lock_path.clone())?
    } else {
        let legacy = project_root.join("pacm-lock.json");
        if legacy.exists() {
            let lf = lockfile::load_json_compat(&legacy)?;
            lockfile::write(&lf, lock_path.clone())?;
            println!("{C_GRAY}[pacm]{C_RESET} migrated lockfile to binary: pacm.lockb");
            lf
        } else {
            Lockfile::default()
        }
    };
    let original_lock = lock.clone();

    if link && copy {
        bail!("--link and --copy cannot be used together");
    }
    let install_mode = if copy { InstallMode::Copy } else { InstallMode::Link };
    let store = CasStore::open()?;

    let old_root_deps: BTreeMap<String, String> = original_lock
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

    lock.sync_from_manifest(&manifest);
    let new_root_deps: BTreeMap<String, String> = lock
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

    let old_names: BTreeSet<_> = old_root_deps.keys().cloned().collect();
    let new_names: BTreeSet<_> = new_root_deps.keys().cloned().collect();
    let added_root: Vec<String> = new_names.difference(&old_names).cloned().collect();
    let removed_root: Vec<String> = old_names.difference(&new_names).cloned().collect();

    if lock == original_lock
        && added_root.is_empty()
        && removed_root.is_empty()
        && node_modules_intact(&manifest, &workspace_names)
    {
        println!("{C_GRAY}[pacm]{C_RESET} {C_DIM}no dependency changes{C_RESET}");
        println!("{C_GRAY}[pacm]{C_RESET} {C_DIM}0 added, 0 removed{C_RESET}");
        println!("{C_GRAY}[pacm]{C_RESET} {C_GREEN}already up to date{C_RESET}");
        return Ok(());
    }

    if specs.is_empty() && added_root.is_empty() {
        if let Some(instances) = build_fast_instances(&manifest, &lock, &workspace_names) {
            if !removed_root.is_empty() {
                prune_removed_from_lock(&mut lock, &removed_root);
                remove_dirs(&removed_root);
                let trans_removed = prune_unreachable(&mut lock);
                if !trans_removed.is_empty() {
                    remove_dirs(&trans_removed);
                }
            }
            if let Ok(plan) = build_plan_from_lock(&store, &lock, &instances) {
                let start = Instant::now();
                let progress = Arc::new(Mutex::new(ProgressRenderer::new()));
                {
                    let mut pr = progress.lock().unwrap();
                    pr.render(format_status(
                        "fast",
                        "link: using cached store; skipping resolution",
                    ));
                }
                let installer = Installer::new(install_mode);
                let outcomes = installer.install(&project_root, &plan, &mut lock)?;
                {
                    let mut pr = progress.lock().unwrap();
                    pr.finish();
                }
                lockfile::write(&lock, lock_path.clone())?;
                if lockfile_has_no_packages(&lock) {
                    let _ = std::fs::remove_file(&lock_path);
                }
                cleanup_empty_node_modules_dir();
                let dur = start.elapsed();
                if added_root.is_empty() && removed_root.is_empty() {
                    println!("{C_GRAY}[pacm]{C_RESET} {C_DIM}no dependency changes{C_RESET}");
                }
                for r in &removed_root {
                    if let Some(ver) = original_lock
                        .packages
                        .get(&format!("node_modules/{r}"))
                        .and_then(|e| e.version.as_ref())
                    {
                        println!("{C_GRAY}[pacm]{C_RESET} {C_RED}-{C_RESET} {r}@{ver}");
                    } else {
                        println!("{C_GRAY}[pacm]{C_RESET} {C_RED}-{C_RESET} {r}");
                    }
                }
                let total = plan.len();
                println!(
                    "{gray}[pacm]{reset} summary: {green}0 added{reset}, {red}{removed} removed{reset}",
                    gray = C_GRAY,
                    green = C_GREEN,
                    red = C_RED,
                    removed = removed_root.len(),
                    reset = C_RESET
                );
                let linked_count =
                    outcomes.iter().filter(|o| o.link_mode == InstallMode::Link).count();
                let copied_count = total.saturating_sub(linked_count);
                if copied_count == 0 {
                    println!(
                        "{C_GRAY}[pacm]{C_RESET} {C_GREEN}linked{C_RESET} {total} packages (all cached) in {dur:.2?}"
                    );
                } else {
                    println!(
                        "{C_GRAY}[pacm]{C_RESET} linked {C_GREEN}{linked_count}{C_RESET} packages ({C_DIM}{copied_count}{C_RESET} copied fallback) in {dur:.2?}"
                    );
                }
                return Ok(());
            }
        }
    }

    let registry_override = std::env::var("PACM_REGISTRY").ok();
    let fetcher = Fetcher::new(registry_override)?;
    let resolver = crate::resolver::Resolver::new();

    #[derive(Clone)]
    struct Task {
        name: String,
        range: String,
        optional_root: bool,
    }

    let mut queue: VecDeque<Task> = VecDeque::new();
    for ws in workspace_map.values() {
        queue.push_back(Task {
            name: ws.name.clone(),
            range: format!("workspace:{}", ws.version),
            optional_root: false,
        });
        for (n, r) in &ws.manifest.dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone(), optional_root: false });
        }
        for (n, r) in &ws.manifest.dev_dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone(), optional_root: false });
        }
        for (n, r) in &ws.manifest.optional_dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone(), optional_root: true });
        }
    }
    if specs.is_empty() {
        for (n, r) in &manifest.dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone(), optional_root: false });
        }
        for (n, r) in &manifest.dev_dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone(), optional_root: false });
        }
        for (n, r) in &manifest.optional_dependencies {
            queue.push_back(Task { name: n.clone(), range: r.clone(), optional_root: true });
        }
    } else {
        for spec in &specs {
            let (name, req) = parse_spec(spec);
            queue.push_back(Task { name, range: req, optional_root: optional });
        }
    }

    let mut visited_name_version: HashSet<(String, String)> = HashSet::new();
    let start = Instant::now();
    let mut installed_count = 0usize;
    let progress = Arc::new(Mutex::new(ProgressRenderer::new()));
    let mut pending_downloads: Vec<PendingDownload> = Vec::new();

    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();

    while let Some(Task { name, range, optional_root }) = queue.pop_front() {
        if visited_name_version.iter().any(|(n, _)| n == &name) {
            continue;
        }
        if !no_progress {
            let mut pr = progress.lock().unwrap();
            pr.render(format_status("resolving", &format!("{name}@{range}")));
        }

        if let Some(ws) = workspace_map.get(&name) {
            let ws_version = ws.manifest.version.clone();
            if !workspace_dep_satisfies(&range, &ws_version) {
                if optional_root {
                    continue;
                }
                bail!("workspace {name}@{ws_version} does not satisfy range {range}");
            }
            if visited_name_version.contains(&(name.clone(), ws_version.clone())) {
                continue;
            }
            let package_os = ws.manifest.os.clone();
            let package_cpu = ws.manifest.cpu_arch.clone();
            let platform_ok = platform_supported(&package_os, &package_cpu);
            let resolved_hint = Some(format!("workspace:{}", ws.relative_path));
            if !platform_ok {
                if optional_root {
                    write_lock_entry(
                        &mut lock,
                        &name,
                        &ws_version,
                        None,
                        resolved_hint.as_deref(),
                        &ws.manifest.dependencies,
                        &ws.manifest.dev_dependencies,
                        &ws.manifest.optional_dependencies,
                        &ws.manifest.peer_dependencies,
                        &BTreeMap::new(),
                        &package_os,
                        &package_cpu,
                    );
                    visited_name_version.insert((name.clone(), ws_version.clone()));
                    continue;
                } else {
                    bail!("workspace {name}@{ws_version} is not supported on this platform");
                }
            }

            write_lock_entry(
                &mut lock,
                &name,
                &ws_version,
                None,
                resolved_hint.as_deref(),
                &ws.manifest.dependencies,
                &ws.manifest.dev_dependencies,
                &ws.manifest.optional_dependencies,
                &ws.manifest.peer_dependencies,
                &BTreeMap::new(),
                &package_os,
                &package_cpu,
            );
            instances.insert(
                name.clone(),
                PackageInstance {
                    name: name.clone(),
                    version: ws_version.clone(),
                    dependencies: ws.manifest.dependencies.clone(),
                    optional_dependencies: ws.manifest.optional_dependencies.clone(),
                    peer_dependencies: ws.manifest.peer_dependencies.clone(),
                    dev_dependencies: ws.manifest.dev_dependencies.clone(),
                    source: Some(ws.dir.clone()),
                },
            );
            visited_name_version.insert((name.clone(), ws_version.clone()));

            let mut to_enqueue: Vec<(String, String, bool)> = Vec::new();
            for (dn, dr) in ws.manifest.dependencies.iter() {
                to_enqueue.push((dn.clone(), dr.clone(), false));
            }
            for (dn, dr) in ws.manifest.dev_dependencies.iter() {
                to_enqueue.push((dn.clone(), dr.clone(), false));
            }
            for (dn, dr) in ws.manifest.optional_dependencies.iter() {
                to_enqueue.push((dn.clone(), dr.clone(), true));
            }
            for (dn, dr) in ws.manifest.peer_dependencies.iter() {
                to_enqueue.push((dn.clone(), dr.clone(), false));
            }
            for (dn, dr, optflag) in to_enqueue {
                queue.push_back(Task { name: dn, range: dr, optional_root: optflag });
            }
            continue;
        }

        let spec_kind = PackageSpec::parse(&range);

        if let PackageSpec::Github(gh_spec) = &spec_kind {
            if !no_progress {
                let mut pr = progress.lock().unwrap();
                pr.render(format_status("resolving", &format!("{name} (github)")));
            }

            let resolved = match resolve_github_tarball(gh_spec) {
                Ok(r) => r,
                Err(e) => {
                    if optional_root {
                        continue;
                    }
                    return Err(e);
                }
            };

            let bytes = match fetcher.download_tarball(&resolved.tarball_url) {
                Ok(b) => b,
                Err(e) => {
                    if optional_root {
                        continue;
                    }
                    return Err(e);
                }
            };

            let manifest_from_tar = match read_manifest_from_tarball(&bytes) {
                Ok(mf) => mf,
                Err(e) => {
                    if optional_root {
                        continue;
                    }
                    return Err(e);
                }
            };

            let base_version = manifest_from_tar.version.clone().unwrap_or_else(|| "0.0.0".into());
            let short = resolved.commit.chars().take(8).collect::<String>();
            let picked_version = append_build(&base_version, &format!("git.{short}"));
            let cache_exists = crate::cache::cache_package_path(&name, &picked_version).exists();
            let integrity_for_entry_string =
                match crate::cache::ensure_cached_package(&name, &picked_version, &bytes, None) {
                    Ok(i) => Some(i),
                    Err(e) => {
                        if optional_root {
                            continue;
                        }
                        return Err(e);
                    }
                };

            write_scripts_sidecar(&name, &picked_version, &manifest_from_tar.scripts);

            let package_os = manifest_from_tar.os.clone();
            let package_cpu = manifest_from_tar.cpu_arch.clone();
            let platform_ok = platform_supported(&package_os, &package_cpu);
            if !platform_ok {
                if optional_root {
                    write_lock_entry(
                        &mut lock,
                        &name,
                        &picked_version,
                        integrity_for_entry_string.as_deref(),
                        Some(resolved.tarball_url.as_str()),
                        &manifest_from_tar.dependencies,
                        &BTreeMap::new(),
                        &manifest_from_tar.optional_dependencies,
                        &manifest_from_tar.peer_dependencies,
                        &manifest_from_tar
                            .peer_dependencies_meta
                            .into_iter()
                            .map(|(k, v)| (k, crate::lockfile::PeerMeta { optional: v.optional }))
                            .collect(),
                        &package_os,
                        &package_cpu,
                    );
                    visited_name_version.insert((name.clone(), picked_version.clone()));
                    continue;
                }
                bail!("{}@{} is not supported on this platform", name, picked_version);
            }

            let peer_meta_map: BTreeMap<String, crate::lockfile::PeerMeta> = manifest_from_tar
                .peer_dependencies_meta
                .iter()
                .map(|(k, v)| (k.clone(), crate::lockfile::PeerMeta { optional: v.optional }))
                .collect();

            write_lock_entry(
                &mut lock,
                &name,
                &picked_version,
                integrity_for_entry_string.as_deref(),
                Some(resolved.tarball_url.as_str()),
                &manifest_from_tar.dependencies,
                &BTreeMap::new(),
                &manifest_from_tar.optional_dependencies,
                &manifest_from_tar.peer_dependencies,
                &peer_meta_map,
                &package_os,
                &package_cpu,
            );

            instances.insert(
                name.clone(),
                PackageInstance {
                    name: name.clone(),
                    version: picked_version.clone(),
                    dependencies: manifest_from_tar.dependencies.clone(),
                    optional_dependencies: manifest_from_tar.optional_dependencies.clone(),
                    peer_dependencies: manifest_from_tar.peer_dependencies.clone(),
                    dev_dependencies: BTreeMap::new(),
                    source: None,
                },
            );
            visited_name_version.insert((name.clone(), picked_version.clone()));
            if !cache_exists {
                installed_count += 1;
            }

            let mut to_enqueue: Vec<(String, String, bool)> = Vec::new();
            for (dn, dr) in manifest_from_tar.dependencies.into_iter() {
                to_enqueue.push((dn, dr, optional_root));
            }
            for (dn, dr) in manifest_from_tar.optional_dependencies.into_iter() {
                to_enqueue.push((dn, dr, true));
            }
            for (dn, dr) in manifest_from_tar.peer_dependencies.into_iter() {
                let is_optional_peer = peer_meta_map.get(&dn).map(|m| m.optional).unwrap_or(false);
                if !is_optional_peer {
                    to_enqueue.push((dn, dr, false));
                }
            }
            for (dn, dr, optflag) in to_enqueue {
                queue.push_back(Task { name: dn, range: dr, optional_root: optflag });
            }
            continue;
        }

        if let PackageSpec::Tarball { url } = &spec_kind {
            if !no_progress {
                let mut pr = progress.lock().unwrap();
                pr.render(format_status("resolving", &format!("{name} (tarball)")));
            }

            let bytes = match fetcher.download_tarball(url) {
                Ok(b) => b,
                Err(e) => {
                    if optional_root {
                        continue;
                    }
                    return Err(e);
                }
            };

            let manifest_from_tar = match read_manifest_from_tarball(&bytes) {
                Ok(mf) => mf,
                Err(e) => {
                    if optional_root {
                        continue;
                    }
                    return Err(e);
                }
            };

            let base_version = manifest_from_tar.version.clone().unwrap_or_else(|| "0.0.0".into());
            let version_tag = append_build(&base_version, &format!("remote.{}", short_hash(url)));
            let cache_exists = crate::cache::cache_package_path(&name, &version_tag).exists();
            let integrity_for_entry_string =
                match crate::cache::ensure_cached_package(&name, &version_tag, &bytes, None) {
                    Ok(i) => Some(i),
                    Err(e) => {
                        if optional_root {
                            continue;
                        }
                        return Err(e);
                    }
                };
            write_scripts_sidecar(&name, &version_tag, &manifest_from_tar.scripts);

            let package_os = manifest_from_tar.os.clone();
            let package_cpu = manifest_from_tar.cpu_arch.clone();
            let platform_ok = platform_supported(&package_os, &package_cpu);
            if !platform_ok {
                if optional_root {
                    write_lock_entry(
                        &mut lock,
                        &name,
                        &version_tag,
                        integrity_for_entry_string.as_deref(),
                        Some(url.as_str()),
                        &manifest_from_tar.dependencies,
                        &BTreeMap::new(),
                        &manifest_from_tar.optional_dependencies,
                        &manifest_from_tar.peer_dependencies,
                        &manifest_from_tar
                            .peer_dependencies_meta
                            .into_iter()
                            .map(|(k, v)| (k, crate::lockfile::PeerMeta { optional: v.optional }))
                            .collect(),
                        &package_os,
                        &package_cpu,
                    );
                    visited_name_version.insert((name.clone(), version_tag.clone()));
                    continue;
                }
                bail!("{}@{} is not supported on this platform", name, version_tag);
            }

            let peer_meta_map: BTreeMap<String, crate::lockfile::PeerMeta> = manifest_from_tar
                .peer_dependencies_meta
                .iter()
                .map(|(k, v)| (k.clone(), crate::lockfile::PeerMeta { optional: v.optional }))
                .collect();

            write_lock_entry(
                &mut lock,
                &name,
                &version_tag,
                integrity_for_entry_string.as_deref(),
                Some(url.as_str()),
                &manifest_from_tar.dependencies,
                &BTreeMap::new(),
                &manifest_from_tar.optional_dependencies,
                &manifest_from_tar.peer_dependencies,
                &peer_meta_map,
                &package_os,
                &package_cpu,
            );

            instances.insert(
                name.clone(),
                PackageInstance {
                    name: name.clone(),
                    version: version_tag.clone(),
                    dependencies: manifest_from_tar.dependencies.clone(),
                    optional_dependencies: manifest_from_tar.optional_dependencies.clone(),
                    peer_dependencies: manifest_from_tar.peer_dependencies.clone(),
                    dev_dependencies: BTreeMap::new(),
                    source: None,
                },
            );
            visited_name_version.insert((name.clone(), version_tag.clone()));
            if !cache_exists {
                installed_count += 1;
            }

            let mut to_enqueue: Vec<(String, String, bool)> = Vec::new();
            for (dn, dr) in manifest_from_tar.dependencies.into_iter() {
                to_enqueue.push((dn, dr, optional_root));
            }
            for (dn, dr) in manifest_from_tar.optional_dependencies.into_iter() {
                to_enqueue.push((dn, dr, true));
            }
            for (dn, dr) in manifest_from_tar.peer_dependencies.into_iter() {
                let is_optional_peer = peer_meta_map.get(&dn).map(|m| m.optional).unwrap_or(false);
                if !is_optional_peer {
                    to_enqueue.push((dn, dr, false));
                }
            }
            for (dn, dr, optflag) in to_enqueue {
                queue.push_back(Task { name: dn, range: dr, optional_root: optflag });
            }
            continue;
        }

        let range = match spec_kind {
            PackageSpec::Registry { range } => range,
            _ => range,
        };

        let picked_result: anyhow::Result<(semver::Version, String)> = (|| {
            let cached = crate::cache::cached_versions(&name);
            let canon = crate::resolver::canonicalize_npm_range(&range);
            let parsed_req = semver::VersionReq::parse(&canon).ok();
            let looks_like_tag =
                !range.contains(' ') && !range.contains("||") && !range.contains(',');
            let is_tag_spec = parsed_req.is_none()
                && canon != "*"
                && !range.eq_ignore_ascii_case("latest")
                && looks_like_tag;
            if is_tag_spec {
                if prefer_offline {
                    bail!("cannot resolve dist-tag '{range}' for {name} offline");
                }
                let meta = fetcher
                    .package_metadata(&name)
                    .with_context(|| format!("fetch metadata for {name}"))?;
                if let Some(tags) = &meta.dist_tags {
                    if let Some(ver_s) = tags.get(&range) {
                        let ver = semver::Version::parse(ver_s).with_context(|| {
                            format!("invalid version '{ver_s}' for tag '{range}'")
                        })?;
                        let tar = meta
                            .versions
                            .get(ver_s)
                            .map(|v| v.dist.tarball.clone())
                            .unwrap_or_default();
                        Ok((ver, tar))
                    } else {
                        bail!("unknown dist-tag '{range}' for {name}");
                    }
                } else {
                    bail!("no dist-tags available for {name}");
                }
            } else {
                if range.contains("||") || canon.contains("||") {
                    let mut map: BTreeMap<semver::Version, String> = BTreeMap::new();
                    for v in cached.into_iter() {
                        map.insert(v, String::new());
                    }
                    if !map.is_empty() {
                        if let Ok((ver, _)) = resolver.pick_version(&map, &range) {
                            return Ok((ver, String::new()));
                        }
                    }
                } else {
                    let req = if canon == "*" {
                        semver::VersionReq::STAR
                    } else {
                        parsed_req.unwrap_or(semver::VersionReq::STAR)
                    };
                    if let Some(ver) = crate::cache::cached_versions(&name)
                        .into_iter()
                        .find(|candidate| req.matches(candidate))
                    {
                        return Ok((ver.clone(), String::new()));
                    }
                }
                let meta = fetcher
                    .package_metadata(&name)
                    .with_context(|| format!("fetch metadata for {name}"))?;
                if range.eq_ignore_ascii_case("latest") {
                    if let Some(tags) = &meta.dist_tags {
                        if let Some(ver_s) = tags.get("latest") {
                            let ver = semver::Version::parse(ver_s)?;
                            let tar = meta
                                .versions
                                .get(ver_s)
                                .map(|v| v.dist.tarball.clone())
                                .unwrap_or_default();
                            Ok((ver, tar))
                        } else {
                            let version_map = crate::resolver::map_versions(&meta);
                            resolver.pick_version(&version_map, "*")
                        }
                    } else {
                        let version_map = crate::resolver::map_versions(&meta);
                        resolver.pick_version(&version_map, "*")
                    }
                } else {
                    let version_map = crate::resolver::map_versions(&meta);
                    resolver.pick_version(&version_map, &range)
                }
            }
        })();

        let (picked_ver, tarball_url) = match picked_result {
            Ok(v) => v,
            Err(e) => {
                if optional_root {
                    if !no_progress {
                        let mut pr = progress.lock().unwrap();
                        pr.render(format_status(
                            "fast",
                            &format!("skip optional {name} (resolve failed)"),
                        ));
                    }
                    continue;
                }
                return Err(e);
            }
        };

        let picked_version = picked_ver.to_string();
        if visited_name_version.contains(&(name.clone(), picked_version.clone())) {
            continue;
        }

        let mut package_os: Vec<String> = Vec::new();
        let mut package_cpu: Vec<String> = Vec::new();
        #[allow(clippy::type_complexity)]
        let (integrity_owned, dep_map, opt_map, peer_map, peer_meta_map, resolved_url, scripts_map): (
            Option<String>,
            BTreeMap<String, String>,
            BTreeMap<String, String>,
            BTreeMap<String, String>,
            BTreeMap<String, crate::lockfile::PeerMeta>,
            Option<String>,
            Option<std::collections::BTreeMap<String, String>>,
        ) = if tarball_url.is_empty() {
            match crate::cache::read_cached_manifest(&name, &picked_version) {
                Ok(mut cached_mf) => {
                    package_os = std::mem::take(&mut cached_mf.os);
                    package_cpu = std::mem::take(&mut cached_mf.cpu_arch);
                    // Try to fetch registry metadata for scripts if possible (don't if prefer_offline)
                    let scripts = if !prefer_offline {
                        match fetcher.package_version_metadata(&name, &picked_version) {
                            Ok(vm) => vm.scripts.clone().into_iter().collect::<std::collections::BTreeMap<_, _>>(),
                            Err(_) => std::collections::BTreeMap::new(),
                        }
                    } else {
                        std::collections::BTreeMap::new()
                    };
                    (
                        None,
                        cached_mf.dependencies.into_iter().collect(),
                        cached_mf
                            .optional_dependencies
                            .into_iter()
                            .filter(|(n, _)| {
                                if let Some(ver) = lock
                                    .packages
                                    .get(&format!("node_modules/{n}"))
                                    .and_then(|e| e.version.clone())
                                {
                                    if let Ok(m) = crate::cache::read_cached_manifest(n, &ver) {
                                        return platform_supported(&m.os, &m.cpu_arch);
                                    }
                                }
                                true
                            })
                            .collect(),
                        cached_mf.peer_dependencies.into_iter().collect(),
                        cached_mf
                            .peer_dependencies_meta
                            .into_iter()
                            .map(|(k, v)| (k, crate::lockfile::PeerMeta { optional: v.optional }))
                            .collect(),
                        None,
                        Some(scripts),
                    )
                }
                Err(e) => {
                    if optional_root {
                        (
                            None,
                            BTreeMap::new(),
                            BTreeMap::new(),
                            BTreeMap::new(),
                            BTreeMap::new(),
                            None,
                            None,
                        )
                    } else {
                        return Err(e);
                    }
                }
            }
        } else {
            let meta2 = match fetcher
                .package_metadata(&name)
                .with_context(|| format!("fetch metadata for {name}"))
            {
                Ok(m) => m,
                Err(e) => {
                    if optional_root {
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            };
            let version_meta = match meta2.versions.get(&picked_version) {
                Some(v) => v,
                None => {
                    if optional_root {
                        continue;
                    } else {
                        anyhow::bail!("version metadata missing for {name}@{picked_ver}");
                    }
                }
            };
            package_os = version_meta.os.clone();
            package_cpu = version_meta.cpu_arch.clone();
            let integrity_owned = version_meta.dist.integrity.clone();
            let mut dm = BTreeMap::new();
            for (dn, dr) in &version_meta.dependencies {
                dm.insert(dn.clone(), dr.clone());
            }
            let mut om = BTreeMap::new();
            for (dn, dr) in &version_meta.optional_dependencies {
                om.insert(dn.clone(), dr.clone());
            }
            let mut pm = BTreeMap::new();
            for (dn, dr) in &version_meta.peer_dependencies {
                pm.insert(dn.clone(), dr.clone());
            }
            let mut pmm = BTreeMap::new();
            for (n, m) in &version_meta.peer_dependencies_meta {
                pmm.insert(n.clone(), crate::lockfile::PeerMeta { optional: m.optional });
            }
            (integrity_owned, dm, om, pm, pmm, Some(version_meta.dist.tarball.clone()), Some(version_meta.scripts.clone()))
        };

        let resolved_for_lock = resolved_url.clone().or_else(|| {
            if !tarball_url.is_empty() {
                Some(tarball_url.clone())
            } else {
                None
            }
        });

        let platform_ok = platform_supported(&package_os, &package_cpu);
        if !platform_ok && optional_root {
            if !no_progress {
                let mut pr = progress.lock().unwrap();
                pr.render(format_status(
                    "fast",
                    &format!("{name}@{picked_version} skipped (platform mismatch)"),
                ));
            }
            write_lock_entry(
                &mut lock,
                &name,
                &picked_version,
                integrity_owned.as_deref(),
                resolved_for_lock.as_deref(),
                &dep_map,
                &BTreeMap::new(),
                &opt_map,
                &peer_map,
                &peer_meta_map,
                &package_os,
                &package_cpu,
            );
            visited_name_version.insert((name.clone(), picked_version.clone()));
            continue;
        }
        let mut reused = false;
        let cached = crate::cache::cache_package_path(&name, &picked_version).exists();
        let integrity_for_entry_string: Option<String>;

        if cached {
            reused = true;
            integrity_for_entry_string = integrity_owned.clone();
        } else {
            if prefer_offline {
                if optional_root {
                    continue;
                }
                bail!("{name}@{picked_ver} not in cache and --prefer-offline is set");
            }
            let url = resolved_url
                .as_deref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| tarball_url.clone());

            if optional_root {
                if !no_progress {
                    let mut pr = progress.lock().unwrap();
                    pr.render(format_status("downloading", &format!("{name}@{picked_version}")));
                }
                let download_result = download_into_cache(
                    &fetcher,
                    &name,
                    &picked_version,
                    &url,
                    integrity_owned.as_deref(),
                    scripts_map.as_ref(),
                );
                match download_result {
                    Ok(integrity) => {
                        integrity_for_entry_string = Some(integrity);
                    }
                    Err(e) => {
                        if optional_root {
                            if !no_progress {
                                let mut pr = progress.lock().unwrap();
                                pr.render(format_status(
                                    "fast",
                                    &format!(
                                        "skip optional {name}@{picked_version} (download failed)"
                                    ),
                                ));
                            }
                            continue;
                        } else {
                            return Err(e);
                        }
                    }
                }
            } else {
                if !no_progress {
                    let mut pr = progress.lock().unwrap();
                    pr.render(format_status(
                        "queued",
                        &format!("download {name}@{picked_version}"),
                    ));
                }
                pending_downloads.push(PendingDownload {
                    name: name.clone(),
                    version: picked_version.clone(),
                    url,
                    integrity_hint: integrity_owned.clone(),
                    scripts: scripts_map.clone(),
                });
                integrity_for_entry_string = integrity_owned.clone();
            }
        }

        let integrity_for_entry = integrity_for_entry_string.as_deref();
        write_lock_entry(
            &mut lock,
            &name,
            &picked_version,
            integrity_for_entry,
            resolved_for_lock.as_deref(),
            &dep_map,
            &BTreeMap::new(),
            &opt_map,
            &peer_map,
            &peer_meta_map,
            &package_os,
            &package_cpu,
        );
        instances.insert(
            name.clone(),
            PackageInstance {
                name: name.clone(),
                version: picked_version.clone(),
                dependencies: dep_map.clone(),
                optional_dependencies: opt_map.clone(),
                peer_dependencies: peer_map.clone(),
                dev_dependencies: BTreeMap::new(),
                source: None,
            },
        );
        visited_name_version.insert((name.clone(), picked_version.clone()));
        if !reused {
            installed_count += 1;
        }

        let mut to_enqueue: Vec<(String, String, bool)> = Vec::new();
        for (dn, dr) in dep_map.into_iter() {
            to_enqueue.push((dn, dr, optional_root));
        }
        for (dn, dr) in opt_map.into_iter() {
            to_enqueue.push((dn, dr, true));
        }
        for (dn, dr) in peer_map.into_iter() {
            let is_optional_peer = peer_meta_map.get(&dn).map(|m| m.optional).unwrap_or(false);
            if !is_optional_peer {
                to_enqueue.push((dn, dr, false));
            }
        }
        for (dn, dr, optflag) in to_enqueue {
            queue.push_back(Task { name: dn, range: dr, optional_root: optflag });
        }
    }

    if !pending_downloads.is_empty() {
        if !no_progress {
            let mut pr = progress.lock().unwrap();
            pr.render(format_status(
                "downloading",
                &format!("{} packages in parallel", pending_downloads.len()),
            ));
        }

        let download_results: Result<Vec<(String, String)>> = pending_downloads
            .par_iter()
            .map(|pd| -> Result<(String, String)> {
                let integrity = download_into_cache(
                    &fetcher,
                    &pd.name,
                    &pd.version,
                    &pd.url,
                    pd.integrity_hint.as_deref(),
                    pd.scripts.as_ref(),
                )?;
                Ok((pd.name.clone(), integrity))
            })
            .collect();

        let download_results = download_results?;
        for (pkg_name, integrity) in download_results {
            if let Some(entry) = lock.packages.get_mut(&format!("node_modules/{pkg_name}")) {
                entry.integrity = Some(integrity);
            }
        }

        if !no_progress {
            let mut pr = progress.lock().unwrap();
            pr.render(format_status(
                "cached",
                &format!("downloaded {} packages", pending_downloads.len()),
            ));
        }
    }

    if !no_progress {
        let mut pr = progress.lock().unwrap();
        pr.clear_line();
    }

    {
        let installed: HashSet<String> = instances.keys().cloned().collect();
        for (k, entry) in lock.packages.iter() {
            if k.is_empty() {
                continue;
            }
            if let Some(pkg_name) = k.strip_prefix("node_modules/") {
                for peer in entry.peer_dependencies.keys() {
                    let is_optional =
                        entry.peer_dependencies_meta.get(peer).map(|m| m.optional).unwrap_or(false);
                    if is_optional {
                        continue;
                    }
                    if !installed.contains(peer) {
                        println!("{C_GRAY}[pacm]{C_RESET} {C_YELLOW}warning{C_RESET} missing peer for {pkg_name}: requires {peer}");
                    }
                }
            }
        }
    }

    if specs.is_empty() {
        let trans_removed = prune_unreachable(&mut lock);
        if !trans_removed.is_empty() {
            remove_dirs(&trans_removed);
        }
    }

    let plan = ensure_store_plan(&store, &mut lock, &instances)?;
    let installer = Installer::new(install_mode);
    let outcomes = installer.install(&project_root, &plan, &mut lock)?;
    lockfile::write(&lock, lock_path.clone())?;
    if lockfile_has_no_packages(&lock) {
        let _ = std::fs::remove_file(&lock_path);
    }
    cleanup_empty_node_modules_dir();
    let dur = start.elapsed();

    if !no_progress {
        let mut pr = progress.lock().unwrap();
        pr.render(format_status("linking", "graph"));
        pr.finish();
    }

    let total = plan.len();
    let reused = total.saturating_sub(installed_count);
    let linked_count = outcomes.iter().filter(|o| o.link_mode == InstallMode::Link).count();
    let copied_count = total.saturating_sub(linked_count);

    if added_root.is_empty() && removed_root.is_empty() {
        println!("{C_GRAY}[pacm]{C_RESET} {C_DIM}no dependency changes{C_RESET}");
    }
    for a in &added_root {
        if let Some(inst) = instances.get(a) {
            println!("{C_GRAY}[pacm]{C_RESET} {C_GREEN}+{C_RESET} {}@{}", a, inst.version);
        } else {
            println!("{C_GRAY}[pacm]{C_RESET} {C_GREEN}+{C_RESET} {a}");
        }
    }
    for r in &removed_root {
        if let Some(ver) = original_lock
            .packages
            .get(&format!("node_modules/{r}"))
            .and_then(|e| e.version.as_ref())
        {
            println!("{C_GRAY}[pacm]{C_RESET} {C_RED}-{C_RESET} {r}@{ver}");
        } else {
            println!("{C_GRAY}[pacm]{C_RESET} {C_RED}-{C_RESET} {r}");
        }
    }
    println!(
        "{gray}[pacm]{reset} summary: {green}{add} added{reset}, {red}{removed} removed{reset}",
        gray = C_GRAY,
        green = C_GREEN,
        red = C_RED,
        add = added_root.len(),
        removed = removed_root.len(),
        reset = C_RESET
    );
    if copied_count == 0 {
        println!("{C_GRAY}[pacm]{C_RESET} linking: {C_GREEN}{linked_count}{C_RESET} linked");
    } else {
        println!(
            "{C_GRAY}[pacm]{C_RESET} linking: {C_GREEN}{linked_count}{C_RESET} linked, {C_DIM}{copied_count}{C_RESET} copied"
        );
    }
    println!(
        "{C_GRAY}[pacm]{C_RESET} {C_GREEN}installed{C_RESET} {total} packages ({C_GREEN}{installed_count} downloaded{C_RESET}, {C_DIM}{reused} reused{C_RESET}) in {dur:.2?}"
    );
    // Detect packages that declare lifecycle scripts (preinstall/install/postinstall)
    let mut pkgs_with_scripts: Vec<String> = Vec::new();
    for (name, plan_entry) in &plan {
        // read store metadata JSON to inspect scripts
        if let Ok(txt) = std::fs::read_to_string(&plan_entry.store_entry.metadata_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                if let Some(scripts) = val.get("scripts") {
                    if scripts.get("preinstall").is_some()
                        || scripts.get("install").is_some()
                        || scripts.get("postinstall").is_some()
                    {
                        pkgs_with_scripts.push(name.clone());
                    }
                }
            }
        }
    }

    // root project scripts from local package.json
    let mut root_has_scripts = false;
    let local_pkg = std::path::PathBuf::from("package.json");
    if local_pkg.exists() {
        if let Ok(txt) = std::fs::read_to_string(&local_pkg) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                if let Some(scripts) = val.get("scripts") {
                    if scripts.get("preinstall").is_some()
                        || scripts.get("install").is_some()
                        || scripts.get("postinstall").is_some()
                    {
                        root_has_scripts = true;
                    }
                }
            }
        }
    }

    if !pkgs_with_scripts.is_empty() || root_has_scripts {
        println!(
            "{C_GRAY}[pacm]{C_RESET} {C_YELLOW}note{C_RESET}: lifecycle scripts detected for some packages. pacm does not run them during 'install' by default."
        );
        if root_has_scripts {
            println!(
                "{C_GRAY}[pacm]{C_RESET} root package has lifecycle scripts defined in package.json"
            );
        }
        if !pkgs_with_scripts.is_empty() {
            println!(
                "{C_GRAY}[pacm]{C_RESET} packages with scripts: {}",
                pkgs_with_scripts.join(", ")
            );
        }
        println!(
            "{C_GRAY}[pacm]{C_RESET} run 'pacm scripts run --all' to execute lifecycle scripts, or 'pacm scripts run <pkg..>' to run for specific packages."
        );
    }

    Ok(())
}

fn build_plan_from_lock(
    store: &CasStore,
    lock: &Lockfile,
    instances: &BTreeMap<String, PackageInstance>,
) -> Result<HashMap<String, InstallPlanEntry>> {
    let mut plan = HashMap::new();
    for (name, instance) in instances {
        let key = format!("node_modules/{name}");
        let lock_entry =
            lock.packages.get(&key).ok_or_else(|| anyhow!("lockfile missing entry for {name}"))?;
        let store_key = lock_entry
            .store_key
            .as_ref()
            .ok_or_else(|| anyhow!("no storeKey recorded for {name}"))?;
        let store_entry = store
            .load_entry(store_key)?
            .ok_or_else(|| anyhow!("store entry {store_key} not found on disk"))?;
        plan.insert(name.clone(), InstallPlanEntry { package: instance.clone(), store_entry });
    }
    Ok(plan)
}

fn ensure_store_plan(
    store: &CasStore,
    lock: &mut Lockfile,
    instances: &BTreeMap<String, PackageInstance>,
) -> Result<HashMap<String, InstallPlanEntry>> {
    let mut memo: HashMap<String, StoreEntry> = HashMap::new();
    let mut visiting: HashSet<String> = HashSet::new();

    for name in instances.keys() {
        let entry =
            ensure_store_for_package(store, lock, instances, name, &mut memo, &mut visiting)?;
        if let Some(lock_entry) = lock.packages.get_mut(&format!("node_modules/{name}")) {
            lock_entry.store_key = Some(entry.store_key.clone());
            lock_entry.content_hash = Some(entry.content_hash.clone());
            lock_entry.store_path = Some(entry.root_dir.display().to_string());
            lock_entry.link_mode = None;
        }
    }

    let mut plan = HashMap::new();
    for (name, instance) in instances {
        if let Some(entry) = memo.get(name) {
            plan.insert(
                name.clone(),
                InstallPlanEntry { package: instance.clone(), store_entry: entry.clone() },
            );
        }
    }
    Ok(plan)
}

fn ensure_store_for_package(
    store: &CasStore,
    lock: &Lockfile,
    instances: &BTreeMap<String, PackageInstance>,
    name: &str,
    memo: &mut HashMap<String, StoreEntry>,
    visiting: &mut HashSet<String>,
) -> Result<StoreEntry> {
    if let Some(existing) = memo.get(name) {
        return Ok(existing.clone());
    }
    if !visiting.insert(name.to_string()) {
        bail!("cyclic dependency detected involving {name}");
    }

    let key = format!("node_modules/{name}");
    let lock_entry =
        lock.packages.get(&key).ok_or_else(|| anyhow!("lockfile missing entry for {name}"))?;
    let version = lock_entry
        .version
        .as_ref()
        .ok_or_else(|| anyhow!("lockfile missing version for {name}"))?
        .clone();

    let mut dep_names: Vec<String> = Vec::new();
    dep_names.extend(lock_entry.dependencies.keys().cloned());
    dep_names.extend(lock_entry.dev_dependencies.keys().cloned());
    dep_names.extend(lock_entry.optional_dependencies.keys().cloned());
    dep_names.extend(lock_entry.peer_dependencies.keys().cloned());
    dep_names.sort();
    dep_names.dedup();

    let mut dep_fps: Vec<DependencyFingerprint> = Vec::with_capacity(dep_names.len());
    for dep in dep_names {
        let dep_key = format!("node_modules/{dep}");
        let Some(dep_entry) = lock.packages.get(&dep_key) else {
            continue;
        };
        let Some(dep_version) = dep_entry.version.as_ref() else {
            continue;
        };
        // If this dependency is optional for the parent package and the package
        // declares an OS/CPU restriction that does not match this host, skip it.
        if lock_entry.optional_dependencies.contains_key(&dep)
            && !platform_supported(&dep_entry.os, &dep_entry.cpu_arch)
        {
            // skip optional dependency incompatible with platform
            continue;
        }
        let dep_store_entry =
            ensure_store_for_package(store, lock, instances, &dep, memo, visiting)?;
        dep_fps.push(DependencyFingerprint {
            name: dep.clone(),
            version: dep_version.clone(),
            store_key: Some(dep_store_entry.store_key.clone()),
        });
    }

    let source_dir = if let Some(inst) = instances.get(name) {
        inst.source.clone().unwrap_or_else(|| crate::cache::cache_package_path(name, &version))
    } else {
        crate::cache::cache_package_path(name, &version)
    };
    let params = EnsureParams {
        name,
        version: &version,
        dependencies: &dep_fps,
        source_dir: &source_dir,
        integrity: lock_entry.integrity.as_deref(),
        resolved: lock_entry.resolved.as_deref(),
    };
    let store_entry = store.ensure_entry(&params)?;
    visiting.remove(name);
    memo.insert(name.to_string(), store_entry.clone());
    Ok(store_entry)
}
