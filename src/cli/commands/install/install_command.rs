use super::fast::build_fast_instances;
use super::manifest_updates::{parse_spec, update_manifest_for_specs};
use super::node_modules::node_modules_intact;
use super::platform::platform_supported;
use super::progress::{format_status, ProgressRenderer};
use super::prune::{
    cleanup_empty_node_modules_dir, lockfile_has_no_packages, prune_removed_from_lock,
    prune_unreachable, remove_dirs,
};
use crate::cache::{CasStore, DependencyFingerprint, EnsureParams, StoreEntry};
use crate::colors::*;
use crate::fetch::Fetcher;
use crate::installer::{InstallMode, InstallPlanEntry, Installer, PackageInstance};
use crate::lockfile::{self, Lockfile, PackageEntry};
use crate::manifest;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
    let manifest_path = PathBuf::from("package.json");
    if !manifest_path.exists() {
        println!("{C_GRAY}[pacm]{C_RESET} {C_RED}error{C_RESET} no package.json found. Run 'pacm init' first.");
        return Ok(());
    }
    let mut manifest = manifest::load(&manifest_path)?;

    update_manifest_for_specs(&specs, &mut manifest, &manifest_path, dev, optional, no_save)?;

    let lock_path = PathBuf::from("pacm.lockb");
    let mut lock = if lock_path.exists() {
        Lockfile::load_or_default(lock_path.clone())?
    } else {
        let legacy = PathBuf::from("pacm-lock.json");
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
    let project_root = std::env::current_dir()?;

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
        && node_modules_intact(&manifest)
    {
        println!("{C_GRAY}[pacm]{C_RESET} {C_DIM}no dependency changes{C_RESET}");
        println!("{C_GRAY}[pacm]{C_RESET} {C_DIM}0 added, 0 removed{C_RESET}");
        println!("{C_GRAY}[pacm]{C_RESET} {C_GREEN}already up to date{C_RESET}");
        return Ok(());
    }

    if specs.is_empty() && added_root.is_empty() {
        if let Some(instances) = build_fast_instances(&manifest, &lock) {
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

    let fetcher = Fetcher::new(None)?;
    let resolver = crate::resolver::Resolver::new();

    #[derive(Clone)]
    struct Task {
        name: String,
        range: String,
        optional_root: bool,
    }

    let mut queue: VecDeque<Task> = VecDeque::new();
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
        let entry = ensure_store_for_package(store, lock, name, &mut memo, &mut visiting)?;
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
        let dep_store_entry = ensure_store_for_package(store, lock, &dep, memo, visiting)?;
        dep_fps.push(DependencyFingerprint {
            name: dep.clone(),
            version: dep_version.clone(),
            store_key: Some(dep_store_entry.store_key.clone()),
        });
    }

    let source_dir = crate::cache::cache_package_path(name, &version);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::cache_package_path;
    use crate::lockfile::Lockfile;
    use once_cell::sync::Lazy;
    use serde_json::json;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use tempfile::tempdir;

    static TEST_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[derive(Debug)]
    struct EnvSandbox {
        temp: tempfile::TempDir,
        prev_xdg: Option<OsString>,
        prev_local: Option<OsString>,
        prev_appdata: Option<OsString>,
        prev_home: Option<OsString>,
    }

    impl EnvSandbox {
        fn new() -> Self {
            let temp = tempdir().expect("create sandbox tempdir");
            let data_home = temp.path().join("data-home");
            fs::create_dir_all(&data_home).expect("create data-home dir");

            let prev_xdg = env::var_os("XDG_DATA_HOME");
            env::set_var("XDG_DATA_HOME", &data_home);

            let prev_local = env::var_os("LOCALAPPDATA");
            env::set_var("LOCALAPPDATA", &data_home);

            let prev_appdata = env::var_os("APPDATA");
            env::set_var("APPDATA", &data_home);

            let prev_home = env::var_os("HOME");
            env::set_var("HOME", temp.path());

            Self { temp, prev_xdg, prev_local, prev_appdata, prev_home }
        }

        fn project_root(&self) -> PathBuf {
            self.temp.path().join("project")
        }
    }

    impl Drop for EnvSandbox {
        fn drop(&mut self) {
            restore_env("XDG_DATA_HOME", &self.prev_xdg);
            restore_env("LOCALAPPDATA", &self.prev_local);
            restore_env("APPDATA", &self.prev_appdata);
            restore_env("HOME", &self.prev_home);
        }
    }

    fn restore_env(key: &str, previous: &Option<OsString>) {
        if let Some(val) = previous {
            env::set_var(key, val);
        } else {
            env::remove_var(key);
        }
    }

    struct CwdGuard {
        prev: PathBuf,
    }

    impl CwdGuard {
        fn change_to(dir: &Path) -> std::io::Result<Self> {
            let prev = env::current_dir()?;
            env::set_current_dir(dir)?;
            Ok(Self { prev })
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.prev);
        }
    }

    fn write_project_manifest(project_root: &Path, manifest: &serde_json::Value) {
        fs::create_dir_all(project_root).expect("create project dir");
        let manifest_path = project_root.join("package.json");
        let data = serde_json::to_string_pretty(manifest).expect("serialize manifest");
        fs::write(manifest_path, data).expect("write package.json");
    }

    fn seed_cached_package(
        name: &str,
        version: &str,
        manifest: serde_json::Value,
        files: &[(&str, &str)],
    ) {
        let dir = cache_package_path(name, version);
        fs::create_dir_all(&dir).expect("create cached package dir");
        let manifest_path = dir.join("package.json");
        fs::write(&manifest_path, manifest.to_string()).expect("write cached manifest");
        // If manifest declares scripts, also write a registry sidecar so store.ensure_entry can pick it up
        if let Some(scripts_val) = manifest.get("scripts") {
            let sidecar_path = dir.join(".registry-scripts.json");
            if let Ok(txt) = serde_json::to_string_pretty(scripts_val) {
                let _ = fs::write(&sidecar_path, txt);
            }
        }
        for (rel, contents) in files {
            let file_path = dir.join(rel);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).expect("create parent dir");
            }
            fs::write(file_path, contents).expect("write cached file");
        }
    }

    #[test]
    fn scripts_run_executes_registry_scripts() -> anyhow::Result<()> {
        let _guard = match TEST_MUTEX.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let sandbox = EnvSandbox::new();
        let project_root = sandbox.project_root();

        // platform specific echo commands
        #[cfg(windows)]
        let scripts = json!({
            "preinstall": "cmd /C echo pre > pre.txt",
            "install": "cmd /C echo inst > inst.txt",
            "postinstall": "cmd /C echo post > post.txt",
        });
        #[cfg(not(windows))]
        let scripts = json!({
            "preinstall": "sh -c 'echo pre > pre.txt'",
            "install": "sh -c 'echo inst > inst.txt'",
            "postinstall": "sh -c 'echo post > post.txt'",
        });

        write_project_manifest(
            &project_root,
            &json!({
                "name": "script-app",
                "version": "0.1.0",
                "dependencies": { "scripty": "1.0.0" }
            }),
        );

        seed_cached_package(
            "scripty",
            "1.0.0",
            json!({ "name": "scripty", "version": "1.0.0", "scripts": scripts }),
            &[("index.js", "module.exports = 'scripty';\n")],
        );

        let _cwd = CwdGuard::change_to(&project_root)?;
        cmd_install(Vec::new(), install_options_copy())?;

        // Run scripts for package directly (auto-confirm)
        crate::cli::commands::cmd_scripts_run(
            vec!["scripty".to_string()],
            false,
            false,
            true,
            false,
        )?;

        let sdir = project_root.join("node_modules").join("scripty");
        assert!(sdir.join("pre.txt").exists());
        assert!(sdir.join("inst.txt").exists());
        assert!(sdir.join("post.txt").exists());

        Ok(())
    }

    fn lockfile_path(project_root: &Path) -> PathBuf {
        project_root.join("pacm.lockb")
    }

    fn install_options_copy() -> InstallOptions {
        InstallOptions { copy: true, no_progress: true, ..InstallOptions::default() }
    }

    #[test]
    fn installs_cached_packages_and_updates_lock() -> anyhow::Result<()> {
        let _guard = match TEST_MUTEX.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let sandbox = EnvSandbox::new();
        let project_root = sandbox.project_root();
        write_project_manifest(
            &project_root,
            &json!({
                "name": "demo-app",
                "version": "0.1.0",
                "dependencies": {
                    "alpha": "1.0.0",
                    "beta": "2.0.0"
                },
                "optionalDependencies": {
                    "optional-root": "1.0.0"
                }
            }),
        );

        let host_os = super::super::platform::node_platform();
        let block_os = format!("!{host_os}");

        seed_cached_package(
            "alpha",
            "1.0.0",
            json!({
                "name": "alpha",
                "version": "1.0.0",
                "dependencies": { "gamma": "1.0.0" },
                "optionalDependencies": { "alpha-optional": "1.0.0" },
                "bin": { "alpha": "bin.js" }
            }),
            &[("bin.js", "#!/usr/bin/env node\nconsole.log('alpha');\n")],
        );

        seed_cached_package(
            "beta",
            "2.0.0",
            json!({
                "name": "beta",
                "version": "2.0.0",
                "peerDependencies": { "gamma": "^1.0.0" }
            }),
            &[("index.js", "module.exports = 'beta';\n")],
        );

        seed_cached_package(
            "gamma",
            "1.0.0",
            json!({
                "name": "gamma",
                "version": "1.0.0",
                "bin": { "gamma-cli": "cli.js" }
            }),
            &[("cli.js", "#!/usr/bin/env node\nconsole.log('gamma');\n")],
        );

        seed_cached_package(
            "alpha-optional",
            "1.0.0",
            json!({
                "name": "alpha-optional",
                "version": "1.0.0",
                "os": [block_os.clone()]
            }),
            &[("index.js", "module.exports = 'optional';\n")],
        );

        seed_cached_package(
            "optional-root",
            "1.0.0",
            json!({
                "name": "optional-root",
                "version": "1.0.0",
                "os": [block_os.clone()]
            }),
            &[("root.js", "module.exports = 'optional-root';\n")],
        );

        let _cwd = CwdGuard::change_to(&project_root)?;
        let options = install_options_copy();
        cmd_install(Vec::new(), options)?;

        #[cfg(windows)]
        let gamma_bin = project_root.join("node_modules\\.bin\\gamma-cli.exe");
        #[cfg(not(windows))]
        let gamma_bin = project_root.join("node_modules/.bin/gamma-cli");
        assert!(gamma_bin.exists(), "gamma bin shim missing");

        let alpha_dir = project_root.join("node_modules").join("alpha");
        assert!(alpha_dir.join("bin.js").exists());
        let gamma_dir = project_root.join("node_modules").join("gamma");
        assert!(gamma_dir.join("cli.js").exists());

        let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
        assert!(lock.packages.get("node_modules/alpha").is_some());
        assert!(lock.packages.get("node_modules/gamma").is_some());
        if let Some(optional_entry) = lock.packages.get("node_modules/optional-root") {
            assert_eq!(optional_entry.version.as_deref(), Some("1.0.0"));
            assert_eq!(optional_entry.os, vec![block_os.clone()]);
            assert!(optional_entry.store_key.is_none());
        } else {
            panic!("optional-root entry missing from lockfile");
        }

        Ok(())
    }

    #[test]
    fn reinstall_prunes_removed_packages() -> anyhow::Result<()> {
        let _guard = match TEST_MUTEX.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let sandbox = EnvSandbox::new();
        let project_root = sandbox.project_root();
        write_project_manifest(
            &project_root,
            &json!({
                "name": "demo-app",
                "version": "0.1.0",
                "dependencies": { "delta": "1.0.0", "epsilon": "1.0.0" }
            }),
        );

        seed_cached_package(
            "delta",
            "1.0.0",
            json!({ "name": "delta", "version": "1.0.0" }),
            &[("index.js", "module.exports = 'delta';\n")],
        );

        seed_cached_package(
            "epsilon",
            "1.0.0",
            json!({ "name": "epsilon", "version": "1.0.0" }),
            &[("index.js", "module.exports = 'epsilon';\n")],
        );

        let _cwd = CwdGuard::change_to(&project_root)?;
        cmd_install(Vec::new(), install_options_copy())?;

        // Rewrite manifest to drop epsilon
        write_project_manifest(
            &project_root,
            &json!({
                "name": "demo-app",
                "version": "0.1.0",
                "dependencies": { "delta": "1.0.0" }
            }),
        );

        cmd_install(Vec::new(), install_options_copy())?;

        let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
        assert!(lock.packages.get("node_modules/delta").is_some());
        assert!(lock.packages.get("node_modules/epsilon").is_none());

        let epsilon_dir = project_root.join("node_modules").join("epsilon");
        assert!(!epsilon_dir.exists(), "epsilon directory should be pruned");
        Ok(())
    }

    #[test]
    fn install_from_specs_updates_manifest() -> anyhow::Result<()> {
        let _guard = match TEST_MUTEX.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let sandbox = EnvSandbox::new();
        let project_root = sandbox.project_root();
        write_project_manifest(
            &project_root,
            &json!({
                "name": "spec-app",
                "version": "0.1.0",
                "dependencies": {}
            }),
        );

        seed_cached_package(
            "zeta",
            "1.0.0",
            json!({ "name": "zeta", "version": "1.0.0" }),
            &[("index.js", "module.exports = 'zeta';\n")],
        );

        let _cwd = CwdGuard::change_to(&project_root)?;
        cmd_install(vec!["zeta@1.0.0".to_string()], install_options_copy())?;

        let manifest_text = fs::read_to_string(project_root.join("package.json"))?;
        let manifest_json: serde_json::Value = serde_json::from_str(&manifest_text)?;
        let deps = manifest_json
            .get("dependencies")
            .and_then(|v| v.as_object())
            .expect("dependencies present");
        assert_eq!(deps.get("zeta").and_then(|v| v.as_str()), Some("1.0.0"));

        let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
        assert!(lock.packages.get("node_modules/zeta").is_some());
        Ok(())
    }
}
