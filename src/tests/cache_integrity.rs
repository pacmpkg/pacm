use super::common::DataHomeGuard;
use crate::cache::{cache_package_path, ensure_cached_package};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::io::Cursor;
use std::path::Path;
use tar::Builder;

fn build_tarball(entries: &[(&str, &str)]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = Builder::new(encoder);
    for (path, contents) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).expect("set tar path");
        header.set_size(contents.as_bytes().len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, Path::new(path), &mut Cursor::new(contents.as_bytes()))
            .expect("append tar data");
    }
    let encoder = builder.into_inner().expect("finish tar builder");
    encoder.finish().expect("finish gzip encoder")
}

#[test]
fn ensure_cached_package_stores_contents() -> anyhow::Result<()> {
    let _sandbox = DataHomeGuard::new();
    let bytes = build_tarball(&[
        ("package/package.json", r#"{"name":"omega","version":"1.0.0"}"#),
        ("package/lib/index.js", "module.exports = 1;\n"),
    ]);

    let integrity = ensure_cached_package("omega", "1.0.0", &bytes, None)?;
    assert!(integrity.starts_with("sha512-"));

    let pkg_dir = cache_package_path("omega", "1.0.0");
    assert!(pkg_dir.join("package.json").exists());
    assert!(pkg_dir.join("lib").join("index.js").exists());

    let integrity_again = ensure_cached_package("omega", "1.0.0", &bytes, Some(&integrity))?;
    assert_eq!(integrity, integrity_again);

    Ok(())
}

#[test]
fn ensure_cached_package_rejects_bad_integrity() {
    let _sandbox = DataHomeGuard::new();
    let bytes = build_tarball(&[("package/package.json", r#"{"name":"theta","version":"1.0.0"}"#)]);

    let bogus = format!("sha512-{}", STANDARD.encode([0u8; 64]));
    let err = ensure_cached_package("theta", "1.0.0", &bytes, Some(&bogus)).unwrap_err();
    assert!(err.to_string().contains("integrity mismatch"));

    // Cache directory should remain empty because the extraction failed.
    let pkg_dir = cache_package_path("theta", "1.0.0");
    assert!(!pkg_dir.exists());
}
