use super::common::DataHomeGuard;
use crate::cache::{cache_package_path, CasStore, EnsureParams, StoreEntry};
use crate::installer::{InstallMode, InstallPlanEntry, Installer, PackageInstance};
use crate::lockfile::{Lockfile, PackageEntry};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::tempdir;

fn prepare_cached_package(name: &str, version: &str) -> PathBuf {
    let dir = cache_package_path(name, version);
    fs::create_dir_all(&dir).expect("create cached package dir");
    let manifest = serde_json::json!({
        "name": name,
        "version": version
    });
    fs::write(dir.join("package.json"), manifest.to_string()).expect("write cached package.json");
    fs::write(dir.join("index.js"), "module.exports = 42;\n").expect("write index.js");
    dir
}

fn lock_entry(version: &str, integrity: &str) -> PackageEntry {
    PackageEntry {
        version: Some(version.to_string()),
        integrity: Some(integrity.to_string()),
        resolved: Some(format!("https://example.com/{version}.tgz")),
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
    }
}

fn package_instance(name: &str, version: &str) -> PackageInstance {
    PackageInstance {
        name: name.to_string(),
        version: version.to_string(),
        dependencies: BTreeMap::new(),
        optional_dependencies: BTreeMap::new(),
        peer_dependencies: BTreeMap::new(),
        dev_dependencies: BTreeMap::new(),
        source: None,
    }
}

fn unique_package(prefix: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}", id)
}

fn node_modules_path(root: &Path, package_name: &str) -> PathBuf {
    let mut path = root.join("node_modules");
    for part in package_name.split('/') {
        path.push(part);
    }
    path
}

fn assert_store_contains(entry: &StoreEntry, filename: &str) {
    if entry.package_dir.join(filename).exists() {
        return;
    }
    let listing: Vec<String> = fs::read_dir(&entry.package_dir)
        .map(|read_dir| {
            read_dir
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    panic!("store entry missing {filename}; entries: {:?}", listing);
}

#[test]
fn installer_links_package_and_updates_lock() {
    let _sandbox = DataHomeGuard::new();
    let name = unique_package("link");
    let pkg_dir = prepare_cached_package(&name, "1.2.3");
    assert!(pkg_dir.join("index.js").exists(), "source index missing");

    let store = CasStore::open().expect("open cas store");
    let params = EnsureParams {
        name: &name,
        version: "1.2.3",
        dependencies: &[],
        source_dir: pkg_dir.as_path(),
        integrity: Some("sha512-foo"),
        resolved: Some("https://example.com/foo.tgz"),
    };
    let store_entry = store.ensure_entry(&params).expect("ensure store entry for foo");
    assert_store_contains(&store_entry, "index.js");

    let mut lock = Lockfile::default();
    let lock_key = format!("node_modules/{name}");
    lock.packages.insert(lock_key.clone(), lock_entry("1.2.3", "sha512-foo"));

    let instance = package_instance(&name, "1.2.3");
    let mut plan = HashMap::new();
    plan.insert(
        name.clone(),
        InstallPlanEntry { package: instance.clone(), store_entry: store_entry.clone() },
    );

    let project = tempdir().expect("create project dir");
    let installer = Installer::new(InstallMode::Link);
    let outcomes =
        installer.install(project.path(), &plan, &mut lock).expect("install via link mode");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].package_name, name);
    assert_eq!(outcomes[0].link_mode, InstallMode::Link);

    let installed_pkg = node_modules_path(project.path(), &name);
    assert!(installed_pkg.join("package.json").exists());
    assert!(installed_pkg.join("index.js").exists());

    let lock_entry = lock.packages.get(&lock_key).expect("lock entry updated");
    let expected_path = store_entry.root_dir.to_string_lossy().to_string();
    assert_eq!(lock_entry.store_key.as_deref(), Some(store_entry.store_key.as_str()));
    assert_eq!(lock_entry.content_hash.as_deref(), Some(store_entry.content_hash.as_str()));
    assert_eq!(lock_entry.link_mode.as_deref(), Some("link"));
    assert_eq!(lock_entry.store_path.as_deref(), Some(expected_path.as_str()));
}

#[test]
fn installer_copy_mode_materializes_files() {
    let _sandbox = DataHomeGuard::new();
    let name = unique_package("copy");
    let pkg_dir = prepare_cached_package(&name, "4.5.6");
    assert!(pkg_dir.join("index.js").exists(), "source index missing");

    let store = CasStore::open().expect("open cas store");
    let params = EnsureParams {
        name: &name,
        version: "4.5.6",
        dependencies: &[],
        source_dir: pkg_dir.as_path(),
        integrity: Some("sha512-bar"),
        resolved: Some("https://example.com/bar.tgz"),
    };
    let store_entry = store.ensure_entry(&params).expect("ensure store entry for bar");
    assert_store_contains(&store_entry, "index.js");

    let mut lock = Lockfile::default();
    let lock_key = format!("node_modules/{name}");
    lock.packages.insert(lock_key.clone(), lock_entry("4.5.6", "sha512-bar"));

    let instance = package_instance(&name, "4.5.6");
    let mut plan = HashMap::new();
    plan.insert(
        name.clone(),
        InstallPlanEntry { package: instance.clone(), store_entry: store_entry.clone() },
    );

    let project = tempdir().expect("create project dir");
    let installer = Installer::new(InstallMode::Copy);
    let outcomes =
        installer.install(project.path(), &plan, &mut lock).expect("install via copy mode");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].package_name, name);
    assert_eq!(outcomes[0].link_mode, InstallMode::Copy);

    let installed_pkg = node_modules_path(project.path(), &name);
    assert!(installed_pkg.join("package.json").exists());
    assert!(installed_pkg.join("index.js").exists());

    let lock_entry = lock.packages.get(&lock_key).expect("lock entry updated");
    assert_eq!(lock_entry.link_mode.as_deref(), Some("copy"));
    assert_eq!(lock_entry.store_key.as_deref(), Some(store_entry.store_key.as_str()));
    assert_eq!(lock_entry.content_hash.as_deref(), Some(store_entry.content_hash.as_str()));
    let expected_path = store_entry.root_dir.to_string_lossy().to_string();
    assert_eq!(lock_entry.store_path.as_deref(), Some(expected_path.as_str()));
}
