use std::path::PathBuf;
use std::collections::{BTreeMap, HashSet, VecDeque};
use anyhow::Result;
use crate::lockfile::Lockfile;
use crate::installer::PackageInstance;
use crate::manifest::Manifest;

pub(super) fn remove_dirs(names: &[String]) {
    use std::fs;
    for n in names {
        let mut p = PathBuf::from("node_modules");
        for part in n.split('/') { p = p.join(part); }
        if p.exists() {
            let _ = fs::remove_dir_all(&p);
            if let Some(scope_dir) = p.parent() {
                if let Ok(mut rd) = fs::read_dir(scope_dir) { if rd.next().is_none() { let _ = fs::remove_dir(scope_dir); } }
            }
        }
    }
}

pub(super) fn prune_unreachable(lock: &mut Lockfile, instances: &BTreeMap<String, PackageInstance>) -> Vec<String> {
    let keep: HashSet<String> = instances.keys().cloned().collect();
    let mut to_remove_keys: Vec<String> = Vec::new();
    let mut removed_names: Vec<String> = Vec::new();
    for k in lock.packages.keys() {
        if k.is_empty() { continue; }
        if let Some(stripped) = k.strip_prefix("node_modules/") { if !keep.contains(stripped) { to_remove_keys.push(k.clone()); removed_names.push(stripped.to_string()); } }
    }
    for k in to_remove_keys { lock.packages.remove(&k); }
    removed_names
}

pub(super) fn lockfile_has_no_packages(lock: &Lockfile) -> bool {
    if let Some(root) = lock.packages.get("") {
        let only_root = lock.packages.len() == 1;
        let no_deps = root.dependencies.is_empty() && root.dev_dependencies.is_empty() && root.optional_dependencies.is_empty() && root.peer_dependencies.is_empty();
        only_root && no_deps
    } else {
        lock.packages.is_empty()
    }
}

pub(super) fn prune_removed_from_lock(lock: &mut Lockfile, removed: &[String]) {
    for name in removed {
        let key = format!("node_modules/{}", name);
        lock.packages.remove(&key);
    }
}

pub(super) fn cleanup_empty_node_modules_dir() {
    use std::fs;
    let nm = PathBuf::from("node_modules");
    if nm.exists() {
        if let Ok(mut rd) = fs::read_dir(&nm) {
            if rd.next().is_none() { let _ = fs::remove_dir(&nm); }
        }
    }
}

pub(super) fn build_fast_instances(
    manifest: &Manifest,
    lock: &Lockfile,
) -> Option<BTreeMap<String, PackageInstance>> {
    let mut needed: HashSet<String> = HashSet::new();
    for (n, _) in &manifest.dependencies { needed.insert(n.clone()); }
    for (n, _) in &manifest.dev_dependencies { needed.insert(n.clone()); }
    for (n, _) in &manifest.optional_dependencies { needed.insert(n.clone()); }
    if needed.is_empty() { return Some(BTreeMap::new()); }
    let mut queue: VecDeque<String> = needed.iter().cloned().collect();
    while let Some(name) = queue.pop_front() {
        let key = format!("node_modules/{}", name);
        if let Some(entry) = lock.packages.get(&key) {
            for dep in entry.dependencies.keys() {
                if needed.insert(dep.clone()) { queue.push_back(dep.clone()); }
            }
        } else {
            return None;
        }
    }
    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();
    for name in needed.iter() {
        let key = format!("node_modules/{}", name);
        let entry = lock.packages.get(&key)?;
        let version = entry.version.clone()?;
        let _ = entry.integrity.as_ref()?;
        if !crate::cache::cache_package_path(&name, &version).exists() { return None; }
        instances.insert(
            name.clone(),
            PackageInstance { name: name.clone(), version: version.clone(), dependencies: entry.dependencies.clone() },
        );
    }
    Some(instances)
}
