use pacm::manifest::{load, write, Manifest};

#[test]
fn manifest_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("package.json");
    let mut m = Manifest::new("demo".into(), "1.0.0".into());
    m.dependencies.insert("lodash".into(), "^4.17.0".into());
    write(&m, &path).unwrap();
    let read_back = load(&path).unwrap();
    assert_eq!(read_back.name, "demo");
    assert_eq!(read_back.dependencies.get("lodash").unwrap(), "^4.17.0");
}
