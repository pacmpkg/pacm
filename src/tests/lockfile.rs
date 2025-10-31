use crate::lockfile::{load, write, Lockfile};
use crate::manifest::Manifest;

#[test]
fn lockfile_sync() {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest = Manifest::new("demo".into(), "0.1.0".into());
    manifest.dependencies.insert("foo".into(), "^1.0.0".into());
    let mut lock = Lockfile::default();
    lock.sync_from_manifest(&manifest);
    let lock_path = dir.path().join("pacm.lockb");
    write(&lock, lock_path.clone()).unwrap();
    let loaded = load(&lock_path).unwrap();
    assert!(loaded.packages.contains_key(""));
    assert!(loaded.packages.contains_key("node_modules/foo"));
}

use crate::lockfile::{decode_current_binary, encode_current_binary, PackageEntry, PeerMeta};
use std::collections::BTreeMap;

#[test]
fn encode_decode_roundtrip() {
    let mut lf = Lockfile::default();
    lf.format = 7;
    let mut entry = PackageEntry {
        version: Some("1.2.3".to_string()),
        integrity: Some("sha512-deadbeef".to_string()),
        resolved: Some("https://registry.example/pkg".to_string()),
        dependencies: BTreeMap::from([(String::from("dep"), String::from("^1.0.0"))]),
        dev_dependencies: BTreeMap::from([(String::from("dev"), String::from("~2.0.0"))]),
        optional_dependencies: BTreeMap::from([(String::from("opt"), String::from("3.0.0"))]),
        peer_dependencies: BTreeMap::from([(String::from("peer"), String::from(">=4"))]),
        peer_dependencies_meta: BTreeMap::from([(
            String::from("peer"),
            PeerMeta { optional: true },
        )]),
        os: vec![String::from("linux")],
        cpu_arch: vec![String::from("x64")],
        store_key: None,
        content_hash: None,
        link_mode: None,
        store_path: None,
    };
    lf.packages.insert(String::from(""), entry.clone());
    entry.version = Some("0.0.1".into());
    lf.packages.insert(String::from("node_modules/dep"), entry);

    let encoded = encode_current_binary(&lf).expect("encode");
    assert!(encoded.starts_with(b"PACMLOCK"));

    let decoded = decode_current_binary(&encoded).expect("decode");
    assert_eq!(lf, decoded);
}
