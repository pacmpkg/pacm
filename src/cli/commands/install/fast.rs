use crate::installer::PackageInstance;
use crate::lockfile::Lockfile;
use crate::manifest::Manifest;
use std::collections::BTreeMap;

pub(crate) fn build_fast_instances(
    manifest: &Manifest,
    lock: &Lockfile,
) -> Option<BTreeMap<String, PackageInstance>> {
    use std::collections::{HashSet, VecDeque};
    let mut needed: HashSet<String> = HashSet::new();
    for name in manifest.dependencies.keys() {
        needed.insert(name.clone());
    }
    for name in manifest.dev_dependencies.keys() {
        needed.insert(name.clone());
    }
    for name in manifest.optional_dependencies.keys() {
        needed.insert(name.clone());
    }
    if needed.is_empty() {
        return Some(BTreeMap::new());
    }

    let mut queue: VecDeque<String> = needed.iter().cloned().collect();
    while let Some(name) = queue.pop_front() {
        let key = format!("node_modules/{name}");
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

    let mut instances: BTreeMap<String, PackageInstance> = BTreeMap::new();
    for name in needed.iter() {
        let key = format!("node_modules/{name}");
        let entry = lock.packages.get(&key)?;
        let version = entry.version.clone()?;
        let _ = entry.integrity.as_ref()?;
        if !crate::cache::cache_package_path(name, &version).exists() {
            return None;
        }
        instances.insert(
            name.clone(),
            PackageInstance {
                name: name.clone(),
                version: version.clone(),
                dependencies: entry.dependencies.clone(),
                optional_dependencies: entry.optional_dependencies.clone(),
                peer_dependencies: entry.peer_dependencies.clone(),
            },
        );
    }
    Some(instances)
}
