use super::download::{human_size, perform_download, DownloadProgress};
use super::fast::build_fast_instances;
use super::manifest_updates::{parse_spec, update_manifest_for_specs};
use super::node_modules::node_modules_intact;
use super::platform::platform_supported;
use super::progress::{format_status, ProgressRenderer};
use super::prune::{
    cleanup_empty_node_modules_dir, lockfile_has_no_packages, prune_removed_from_lock,
    prune_unreachable, remove_dirs,
};
use crate::colors::*;
use crate::fetch::Fetcher;
use crate::installer::{Installer, PackageInstance};
use crate::lockfile::{self, Lockfile, PackageEntry};
use crate::manifest;
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn ensure_lock_entry<'a>(lock: &'a mut Lockfile, name: &str) -> &'a mut PackageEntry {
    let key = format!("node_modules/{}", name);
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
    })
}

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
}

pub(crate) fn cmd_install(
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

    update_manifest_for_specs(
        &specs,
        &mut manifest,
        &manifest_path,
        dev,
        optional,
        no_save,
    )?;

    let lock_path = PathBuf::from("pacm.lockb");
    let mut lock = if lock_path.exists() {
        Lockfile::load_or_default(lock_path.clone())?
    } else {
        let legacy = PathBuf::from("pacm-lock.json");
        if legacy.exists() {
            let lf = lockfile::load_json_compat(&legacy)?;
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
    let original_lock = lock.clone();

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
            let start = Instant::now();
            let mut merged_root_deps: BTreeMap<String, String> = BTreeMap::new();
            merged_root_deps.extend(manifest.dependencies.clone());
            merged_root_deps.extend(manifest.dev_dependencies.clone());
            merged_root_deps.extend(manifest.optional_dependencies.clone());
            let progress = Arc::new(Mutex::new(ProgressRenderer::new()));
            {
                let mut pr = progress.lock().unwrap();
                pr.render(format_status(
                    "fast",
                    "link: using cached store; skipping resolution",
                ));
            }
            let installer = Installer::new();
            let as_hash: std::collections::HashMap<String, PackageInstance> =
                instances.clone().into_iter().collect();
            installer.install(&std::env::current_dir()?, &as_hash, &merged_root_deps)?;
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
                "{gray}[pacm]{reset} summary: {green}0 added{reset}, {red}{removed} removed{reset}",
                gray = C_GRAY,
                green = C_GREEN,
                red = C_RED,
                removed = removed_root.len(),
                reset = C_RESET
            );
            println!(
                "{gray}[pacm]{reset} {green}linked{reset} {total} packages (all cached) in {duration:.2?}",
                gray = C_GRAY,
                green = C_GREEN,
                reset = C_RESET,
                total = total,
                duration = dur
            );
            return Ok(());
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
            queue.push_back(Task {
                name: n.clone(),
                range: r.clone(),
                optional_root: false,
            });
        }
        for (n, r) in &manifest.dev_dependencies {
            queue.push_back(Task {
                name: n.clone(),
                range: r.clone(),
                optional_root: false,
            });
        }
        for (n, r) in &manifest.optional_dependencies {
            queue.push_back(Task {
                name: n.clone(),
                range: r.clone(),
                optional_root: true,
            });
        }
    } else {
        for spec in &specs {
            let (name, req) = parse_spec(spec);
            queue.push_back(Task {
                name,
                range: req,
                optional_root: optional,
            });
        }
    }

    let mut visited_name_version: HashSet<(String, String)> = HashSet::new();
    let start = Instant::now();
    let mut installed_count = 0usize;
    let progress = Arc::new(Mutex::new(ProgressRenderer::new()));
    let downloads: Arc<Mutex<Vec<DownloadProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let progress_clone = progress.clone();
    let downloads_clone = downloads.clone();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_thread = stop_flag.clone();

    let painter = if no_progress {
        None
    } else {
        Some(thread::spawn(move || {
            let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut tick: usize = 0;
            loop {
                thread::sleep(Duration::from_millis(100));
                let mut pr = progress_clone.lock().unwrap();
                let dl = downloads_clone.lock().unwrap();
                let mut active_lines = Vec::new();
                for d in dl.iter() {
                    if d.done {
                        continue;
                    }
                    let frame = spinner_frames[tick % spinner_frames.len()];
                    let total = d.total.unwrap_or(0);
                    let pct = if total > 0 {
                        (d.downloaded as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };
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
                    let speed = if elapsed > 0.0 {
                        d.downloaded as f64 / elapsed
                    } else {
                        0.0
                    };
                    let eta = if total > 0 && speed > 0.0 {
                        let remain = (total.saturating_sub(d.downloaded)) as f64 / speed;
                        format!("{remain:.1}s")
                    } else {
                        "?s".to_string()
                    };
                    active_lines.push(format!(
                        "{frame} {name}@{ver} {bar} {pct:.0}% {done}/{total} {spd}/s ETA {eta}",
                        frame = frame,
                        name = d.name,
                        ver = d.version,
                        bar = bar,
                        pct = pct,
                        done = human_size(d.downloaded),
                        total = if total > 0 {
                            human_size(total)
                        } else {
                            "?".into()
                        },
                        spd = human_size(speed as u64),
                        eta = eta
                    ));
                }
                tick = tick.wrapping_add(1);
                if active_lines.is_empty() {
                    if stop_flag_thread.load(Ordering::SeqCst) {
                        break;
                    }
                    continue;
                }
                pr.render(active_lines.join(" | "));
                if dl.iter().all(|d| d.done) && stop_flag_thread.load(Ordering::SeqCst) {
                    break;
                }
            }
        }))
    };

    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();

    while let Some(Task {
        name,
        range,
        optional_root,
    }) = queue.pop_front()
    {
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
                    bail!("cannot resolve dist-tag '{}' for {} offline", range, name);
                }
                let meta = fetcher
                    .package_metadata(&name)
                    .with_context(|| format!("fetch metadata for {}", name))?;
                if let Some(tags) = &meta.dist_tags {
                    if let Some(ver_s) = tags.get(&range) {
                        let ver = semver::Version::parse(ver_s).with_context(|| {
                            format!("invalid version '{}' for tag '{}'", ver_s, range)
                        })?;
                        let tar = meta
                            .versions
                            .get(ver_s)
                            .map(|v| v.dist.tarball.clone())
                            .unwrap_or_default();
                        Ok((ver, tar))
                    } else {
                        bail!("unknown dist-tag '{}' for {}", range, name);
                    }
                } else {
                    bail!("no dist-tags available for {}", name);
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
                    .with_context(|| format!("fetch metadata for {}", name))?;
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
                            &format!("skip optional {} (resolve failed)", name),
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
        let (integrity_owned, dep_map, opt_map, peer_map, peer_meta_map, resolved_url): (
            Option<String>,
            BTreeMap<String, String>,
            BTreeMap<String, String>,
            BTreeMap<String, String>,
            BTreeMap<String, crate::lockfile::PeerMeta>,
            Option<String>,
        ) = if tarball_url.is_empty() {
            match crate::cache::read_cached_manifest(&name, &picked_version) {
                Ok(mut cached_mf) => {
                    package_os = std::mem::take(&mut cached_mf.os);
                    package_cpu = std::mem::take(&mut cached_mf.cpu_arch);
                    (
                        None,
                        cached_mf.dependencies.into_iter().collect(),
                        cached_mf
                            .optional_dependencies
                            .into_iter()
                            .filter(|(n, _)| {
                                if let Some(ver) = lock
                                    .packages
                                    .get(&format!("node_modules/{}", n))
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
                            .map(|(k, v)| {
                                (
                                    k,
                                    crate::lockfile::PeerMeta {
                                        optional: v.optional,
                                    },
                                )
                            })
                            .collect(),
                        None,
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
                        )
                    } else {
                        return Err(e);
                    }
                }
            }
        } else {
            let meta2 = match fetcher
                .package_metadata(&name)
                .with_context(|| format!("fetch metadata for {}", name))
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
                        anyhow::bail!("version metadata missing for {}@{}", name, picked_ver);
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
                pmm.insert(
                    n.clone(),
                    crate::lockfile::PeerMeta {
                        optional: m.optional,
                    },
                );
            }
            (
                integrity_owned,
                dm,
                om,
                pm,
                pmm,
                Some(version_meta.dist.tarball.clone()),
            )
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
        let integrity = if cached {
            reused = true;
            integrity_owned.as_deref().unwrap_or("").to_string()
        } else {
            if prefer_offline {
                if optional_root {
                    continue;
                }
                bail!(
                    "{name}@{ver} not in cache and --prefer-offline is set",
                    name = name,
                    ver = picked_ver
                );
            }
            if !no_progress {
                let mut pr = progress.lock().unwrap();
                pr.render(format_status(
                    "downloading",
                    &format!("{name}@{picked_version}"),
                ));
            }
            let url = resolved_url
                .as_deref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| tarball_url.clone());
            let bytes = match perform_download(&fetcher, &name, &picked_version, &url, &downloads) {
                Ok(b) => b,
                Err(e) => {
                    if optional_root {
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            };
            if !no_progress {
                let mut pr = progress.lock().unwrap();
                pr.render(format_status(
                    "extracting",
                    &format!("{name}@{picked_version}"),
                ));
            }
            match crate::cache::ensure_cached_package(
                &name,
                &picked_version,
                &bytes,
                integrity_owned.as_deref(),
            ) {
                Ok(i) => i,
                Err(e) => {
                    if optional_root {
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        };

        let integrity_for_entry = if integrity.is_empty() {
            None
        } else {
            Some(integrity.as_str())
        };
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
            queue.push_back(Task {
                name: dn,
                range: dr,
                optional_root: optflag,
            });
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
                for (peer, _range) in &entry.peer_dependencies {
                    let is_optional = entry
                        .peer_dependencies_meta
                        .get(peer)
                        .map(|m| m.optional)
                        .unwrap_or(false);
                    if is_optional {
                        continue;
                    }
                    if !installed.contains(peer) {
                        println!(
                            "{gray}[pacm]{reset} {yellow}warning{reset} missing peer for {pkg}: requires {peer}",
                            gray = C_GRAY,
                            yellow = C_YELLOW,
                            reset = C_RESET,
                            pkg = pkg_name,
                            peer = peer
                        );
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

    let installer = Installer::new();
    let mut merged_root_deps: BTreeMap<String, String> = BTreeMap::new();
    merged_root_deps.extend(manifest.dependencies.clone());
    merged_root_deps.extend(manifest.dev_dependencies.clone());
    merged_root_deps.extend(manifest.optional_dependencies.clone());

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

    stop_flag.store(true, Ordering::SeqCst);
    if let Some(p) = painter {
        p.join().ok();
    }
    if !no_progress {
        let mut pr = progress.lock().unwrap();
        pr.render(format_status("linking", "graph"));
        pr.finish();
    }

    let total = total_packages_for_summary;
    let reused = total.saturating_sub(installed_count);

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
        "{gray}[pacm]{reset} summary: {green}{add} added{reset}, {red}{removed} removed{reset}",
        gray = C_GRAY,
        green = C_GREEN,
        red = C_RED,
        add = added_root.len(),
        removed = removed_root.len(),
        reset = C_RESET
    );
    println!(
        "{gray}[pacm]{reset} {green}installed{reset} {total} packages ({green}{downloaded} downloaded{reset}, {dim}{reused_count} reused{reset}) in {duration:.2?}",
        gray = C_GRAY,
        green = C_GREEN,
        dim = C_DIM,
        reset = C_RESET,
        total = total,
        downloaded = installed_count,
        reused_count = reused,
        duration = dur
    );
    Ok(())
}
