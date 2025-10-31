use super::common::DataHomeGuard;
use crate::cache::{cache_package_path, CasStore, DependencyFingerprint, EnsureParams};
use serde_json::Value;
use std::fs;
use std::path::Path;

fn write_package_json(dir: &Path, name: &str, version: &str) {
    let content = serde_json::json!({
        "name": name,
        "version": version,
        "bin": "bin.js"
    });
    fs::create_dir_all(dir).expect("create package dir");
    fs::write(dir.join("package.json"), content.to_string()).expect("write package.json");
    fs::write(dir.join("bin.js"), "#!/usr/bin/env node\nconsole.log('ok');\n")
        .expect("write bin.js");
}

#[test]
fn cas_store_creates_and_loads_entry() {
    let _sandbox = DataHomeGuard::new();

    let pkg_dir = cache_package_path("foo", "1.2.3");
    write_package_json(&pkg_dir, "foo", "1.2.3");

    let store = CasStore::open().expect("open cas store");
    let deps: Vec<DependencyFingerprint> = Vec::new();
    let params = EnsureParams {
        name: "foo",
        version: "1.2.3",
        dependencies: &deps,
        source_dir: pkg_dir.as_path(),
        integrity: Some("sha512-test"),
        resolved: Some("https://example.com/foo.tgz"),
    };

    let entry = store.ensure_entry(&params).expect("ensure foo store entry");
    assert!(entry.package_dir.join("package.json").exists());
    assert!(entry.package_dir.join("bin.js").exists());
    assert!(entry.metadata_path.exists());
    assert!(entry.store_key.contains("foo@1.2.3::"));

    let metadata_text = fs::read_to_string(&entry.metadata_path).expect("read metadata");
    let metadata: Value = serde_json::from_str(&metadata_text).expect("parse metadata json");
    assert_eq!(metadata["store_key"], entry.store_key);
    assert_eq!(metadata["name"], "foo");
    assert_eq!(metadata["version"], "1.2.3");
    assert_eq!(metadata["integrity"], "sha512-test");

    let loaded =
        store.load_entry(&entry.store_key).expect("load entry").expect("entry should exist");
    assert_eq!(loaded.store_key, entry.store_key);
    assert_eq!(loaded.content_hash, entry.content_hash);
    assert_eq!(loaded.graph_hash, entry.graph_hash);
    assert_eq!(loaded.dependencies.len(), entry.dependencies.len());
    assert_eq!(loaded.package_dir, entry.package_dir);

    let entry_again = store.ensure_entry(&params).expect("ensure entry second time");
    assert_eq!(entry_again.store_key, entry.store_key);
    assert_eq!(entry_again.created_at, entry.created_at);
    assert_eq!(entry_again.dependencies.len(), entry.dependencies.len());
}

#[test]
fn cas_store_dependency_order_deterministic() {
    let _sandbox = DataHomeGuard::new();

    let dep_a_dir = cache_package_path("dep-a", "1.0.0");
    write_package_json(&dep_a_dir, "dep-a", "1.0.0");
    let dep_b_dir = cache_package_path("dep-b", "2.0.0");
    write_package_json(&dep_b_dir, "dep-b", "2.0.0");
    let parent_dir = cache_package_path("parent", "3.0.0");
    write_package_json(&parent_dir, "parent", "3.0.0");

    let store = CasStore::open().expect("open cas store");

    let dep_a_entry = store
        .ensure_entry(&EnsureParams {
            name: "dep-a",
            version: "1.0.0",
            dependencies: &[],
            source_dir: dep_a_dir.as_path(),
            integrity: Some("sha512-dep-a"),
            resolved: Some("https://example.com/dep-a.tgz"),
        })
        .expect("ensure dep-a entry");
    let dep_b_entry = store
        .ensure_entry(&EnsureParams {
            name: "dep-b",
            version: "2.0.0",
            dependencies: &[],
            source_dir: dep_b_dir.as_path(),
            integrity: Some("sha512-dep-b"),
            resolved: Some("https://example.com/dep-b.tgz"),
        })
        .expect("ensure dep-b entry");

    let deps_forward = vec![
        DependencyFingerprint {
            name: "dep-a".into(),
            version: "1.0.0".into(),
            store_key: Some(dep_a_entry.store_key.clone()),
        },
        DependencyFingerprint {
            name: "dep-b".into(),
            version: "2.0.0".into(),
            store_key: Some(dep_b_entry.store_key.clone()),
        },
    ];
    let first = store
        .ensure_entry(&EnsureParams {
            name: "parent",
            version: "3.0.0",
            dependencies: &deps_forward,
            source_dir: parent_dir.as_path(),
            integrity: Some("sha512-parent"),
            resolved: Some("https://example.com/parent.tgz"),
        })
        .expect("ensure parent forward order");

    let deps_reverse = vec![deps_forward[1].clone(), deps_forward[0].clone()];
    let second = store
        .ensure_entry(&EnsureParams {
            name: "parent",
            version: "3.0.0",
            dependencies: &deps_reverse,
            source_dir: parent_dir.as_path(),
            integrity: Some("sha512-parent"),
            resolved: Some("https://example.com/parent.tgz"),
        })
        .expect("ensure parent reverse order");

    assert_eq!(first.store_key, second.store_key);
    assert_eq!(first.graph_hash, second.graph_hash);
    assert_eq!(first.root_dir, second.root_dir);
    assert_eq!(first.dependencies.len(), 2);
    let deps_first: Vec<(String, Option<String>)> =
        first.dependencies.iter().map(|d| (d.name.clone(), d.store_key.clone())).collect();
    let deps_second: Vec<(String, Option<String>)> =
        second.dependencies.iter().map(|d| (d.name.clone(), d.store_key.clone())).collect();
    assert_eq!(deps_first, deps_second);

    // Store path should live under the cas store root directory.
    assert!(first.root_dir.starts_with(store.root()));
}
