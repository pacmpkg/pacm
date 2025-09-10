use std::path::{PathBuf, Path};
use crate::fsutil::store_root;
use anyhow::{Result, Context};
use sha2::{Sha512, Digest};
use std::fs;
use tar::Archive;
use flate2::read::GzDecoder;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::ffi::OsStr;

pub fn package_dir(sha512_hex: &str) -> PathBuf {
    let mut root = store_root();
    root.push("sha512");
    let (first2, _) = sha512_hex.split_at(2.min(sha512_hex.len()));
    root.push(first2);
    root.push(sha512_hex);
    root
}

pub fn package_path(sha512_hex: &str) -> PathBuf { let mut d = package_dir(sha512_hex); d.push("package"); d }

/// If integrity (sha512-BASE64) maps to a stored package return its hex digest (whether or not it exists we return hex for potential use)
pub fn exists_by_integrity(integrity: &str) -> Option<String> {
    if !integrity.starts_with("sha512-") { return None; }
    let b64 = &integrity[7..];
    if let Ok(raw) = STANDARD.decode(b64) {
        let hex = hex::encode(raw);
        if package_path(&hex).exists() { return Some(hex); }
        return Some(hex); // return hex anyway for potential path planning
    }
    None
}

/// Ensure tarball bytes are present in store. If integrity is provided, verify.
pub fn ensure_package(bytes: &[u8], integrity_hint: Option<&str>) -> Result<(String, String)> {
    // Hash bytes
    let mut hasher = Sha512::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let computed_hex = hex::encode(&digest);
    let computed_integrity = format!("sha512-{}", STANDARD.encode(&digest));
    if let Some(integrity) = integrity_hint {
        if integrity.starts_with("sha512-") {
            let b64 = &integrity[7..];
            let raw = STANDARD.decode(b64).with_context(|| "decode integrity base64")?;
            if raw != digest[..] { anyhow::bail!("integrity mismatch: expected {}, got {}", integrity, computed_integrity); }
        }
    }
    let dir = package_dir(&computed_hex);
    let marker = package_path(&computed_hex);
    if marker.exists() { return Ok((computed_hex, computed_integrity)); }
    let tmp = dir.with_extension("tmp");
    fs::create_dir_all(&tmp)?;
    let extract_root = tmp.join("package");
    fs::create_dir_all(&extract_root)?;
    // Extract while flattening leading 'package/' directory if present
    let gz = GzDecoder::new(bytes);
    let mut ar = Archive::new(gz);
    for entry in ar.entries()? {
        let mut e = entry?;
        let path = e.path()?; // relative path inside tar
        if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) { continue; }
        let comps: Vec<_> = path.components().collect();
        let stripped: std::path::PathBuf = if comps.len()>1 && comps[0].as_os_str()==OsStr::new("package") { comps[1..].iter().collect() } else { path.to_path_buf() };
        if stripped.as_os_str().is_empty() { continue; }
        let dest_path = extract_root.join(&stripped);
        if let Some(parent) = dest_path.parent() { fs::create_dir_all(parent)?; }
        e.unpack(&dest_path)?;
    }
    // Some tarballs (especially scoped packages) extract as package/<pkgname>/... when
    // the real package root is the inner directory. If we have exactly one child
    // directory inside extract_root and it contains a package.json, promote its
    // contents one level up so the stored package dir has package.json at the root.
    let mut entries = Vec::new();
    for d in fs::read_dir(&extract_root)? {
        entries.push(d?);
    }
    if entries.len() == 1 {
        let only = &entries[0];
        let only_path = only.path();
        if only.file_type()?.is_dir() && only_path.join("package.json").exists() {
            for child in fs::read_dir(&only_path)? {
                let child = child?;
                let from = child.path();
                let to = extract_root.join(child.file_name());
                fs::rename(&from, &to)?;
            }
            // remove now-empty directory
            fs::remove_dir(&only_path)?;
        }
    }
    fs::create_dir_all(dir.parent().unwrap())?;
    fs::rename(&tmp, &dir)?;
    Ok((computed_hex, computed_integrity))
}

pub fn link_into_project(store_pkg_dir: &Path, project_root: &Path, name: &str) -> Result<()> {
    // Legacy helper now enforces symlink-only policy
    let nm = project_root.join("node_modules");
    fs::create_dir_all(&nm)?;
    let target = nm.join(name);
    if target.exists() { return Ok(()); }
    let legacy = store_pkg_dir.join("package");
    let source = if legacy.exists() { legacy } else { store_pkg_dir.to_path_buf() };
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_dir; symlink_dir(&source, &target)?; return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink; symlink(&source, &target)?; return Ok(());
    }
    #[cfg(not(any(unix, windows)))]
    {
        // Minimal fallback: create directory with shallow symlinked (copied) files
        copy_dir::copy_dir(&source, &target)?; return Ok(());
    }
}

// (legacy copy_dir module removed; all linking now via symlinks)
