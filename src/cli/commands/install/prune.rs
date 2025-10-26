use crate::lockfile::{Lockfile, PackageEntry};
use std::path::PathBuf;

pub(crate) fn prune_removed_from_lock(lock: &mut Lockfile, removed: &[String]) {
    for name in removed {
        let key = format!("node_modules/{name}");
        lock.packages.remove(&key);
    }
}

pub(crate) fn prune_unreachable(lock: &mut Lockfile) -> Vec<String> {
    use std::collections::{HashSet, VecDeque};

    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    if let Some(root) = lock.packages.get("") {
        enqueue_root(root, &mut queue);
    }

    while let Some(name) = queue.pop_front() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let key = format!("node_modules/{name}");
        if let Some(entry) = lock.packages.get(&key) {
            enqueue_entry(entry, &mut queue);
        }
    }

    let mut to_remove = Vec::new();
    let mut removed_names = Vec::new();
    for key in lock.packages.keys().cloned().collect::<Vec<_>>() {
        if key.is_empty() {
            continue;
        }
        if let Some(stripped) = key.strip_prefix("node_modules/") {
            if !reachable.contains(stripped) {
                to_remove.push(key.clone());
                removed_names.push(stripped.to_string());
            }
        }
    }
    for key in to_remove {
        lock.packages.remove(&key);
    }
    removed_names
}

pub(crate) fn remove_dirs(names: &[String]) {
    use std::fs;
    for name in names {
        let mut path = PathBuf::from("node_modules");
        for part in name.split('/') {
            path = path.join(part);
        }
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
            if let Some(scope_dir) = path.parent() {
                if let Ok(mut read_dir) = fs::read_dir(scope_dir) {
                    if read_dir.next().is_none() {
                        let _ = fs::remove_dir(scope_dir);
                    }
                }
            }
        }
    }
}

pub(crate) fn cleanup_empty_node_modules_dir() {
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

pub(crate) fn lockfile_has_no_packages(lock: &Lockfile) -> bool {
    if let Some(root) = lock.packages.get("") {
        let only_root = lock.packages.len() == 1;
        let no_deps = root.dependencies.is_empty()
            && root.dev_dependencies.is_empty()
            && root.optional_dependencies.is_empty()
            && root.peer_dependencies.is_empty();
        only_root && no_deps
    } else {
        lock.packages.is_empty()
    }
}

fn enqueue_root(entry: &PackageEntry, queue: &mut std::collections::VecDeque<String>) {
    for name in entry
        .dependencies
        .keys()
        .chain(entry.dev_dependencies.keys())
        .chain(entry.optional_dependencies.keys())
    {
        queue.push_back(name.clone());
    }
    for peer in entry.peer_dependencies.keys() {
        let is_optional = entry
            .peer_dependencies_meta
            .get(peer)
            .map(|meta| meta.optional)
            .unwrap_or(false);
        if !is_optional {
            queue.push_back(peer.clone());
        }
    }
}

fn enqueue_entry(entry: &PackageEntry, queue: &mut std::collections::VecDeque<String>) {
    for dep in entry.dependencies.keys() {
        queue.push_back(dep.clone());
    }
    for dep in entry.optional_dependencies.keys() {
        queue.push_back(dep.clone());
    }
    for peer in entry.peer_dependencies.keys() {
        let is_optional = entry
            .peer_dependencies_meta
            .get(peer)
            .map(|meta| meta.optional)
            .unwrap_or(false);
        if !is_optional {
            queue.push_back(peer.clone());
        }
    }
}
