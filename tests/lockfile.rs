use pacm::lockfile::{Lockfile, write, load};
use pacm::manifest::Manifest;

#[test]
fn lockfile_sync() {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest = Manifest::new("demo".into(), "0.1.0".into());
    manifest.dependencies.insert("foo".into(), "^1.0.0".into());
    let mut lock = Lockfile::default();
    lock.sync_from_manifest(&manifest);
    let lock_path = dir.path().join("pacm-lock.json");
    write(&lock, lock_path.clone()).unwrap();
    let loaded = load(&lock_path).unwrap();
    assert!(loaded.packages.contains_key(""));
    assert!(loaded.packages.contains_key("node_modules/foo"));
}
