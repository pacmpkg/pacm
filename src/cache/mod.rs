use crate::fsutil::cache_root;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha512};
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use tar::Archive;

fn cache_dir_for(name: &str, version: &str) -> PathBuf {
    let mut root = cache_root();
    root.push("pkgs");
    for part in name.split('/') {
        root.push(part);
    }
    root.push(version);
    root
}

pub fn cache_package_path(name: &str, version: &str) -> PathBuf {
    let mut d = cache_dir_for(name, version);
    d.push("package");
    d
}

pub fn ensure_cached_package(
    name: &str,
    version: &str,
    bytes: &[u8],
    integrity_hint: Option<&str>,
) -> Result<String> {
    // Hash bytes for integrity verification only
    let mut hasher = Sha512::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let computed_integrity = format!("sha512-{}", STANDARD.encode(digest));
    if let Some(integrity) = integrity_hint {
        if let Some(b64) = integrity.strip_prefix("sha512-") {
            let raw = STANDARD
                .decode(b64)
                .with_context(|| "decode integrity base64")?;
            if raw != digest[..] {
                anyhow::bail!(
                    "integrity mismatch: expected {}, got {}",
                    integrity,
                    computed_integrity
                );
            }
        }
    }
    let dir = cache_dir_for(name, version);
    let marker = cache_package_path(name, version);
    if marker.exists() {
        return Ok(integrity_hint.unwrap_or(&computed_integrity).to_string());
    }
    let tmp = dir.with_extension("tmp");
    fs::create_dir_all(&tmp)?;
    let extract_root = tmp.join("package");
    fs::create_dir_all(&extract_root)?;
    let gz = GzDecoder::new(bytes);
    let mut ar = Archive::new(gz);
    for entry in ar.entries()? {
        let mut e = entry?;
        let path = e.path()?; // relative path inside tar
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let comps: Vec<_> = path.components().collect();
        let stripped: std::path::PathBuf =
            if comps.len() > 1 && comps[0].as_os_str() == OsStr::new("package") {
                comps[1..].iter().collect()
            } else {
                path.to_path_buf()
            };
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let dest_path = extract_root.join(&stripped);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }
        e.unpack(&dest_path)?;
    }
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
            fs::remove_dir(&only_path)?;
        }
    }
    fs::create_dir_all(dir.parent().unwrap())?;
    fs::rename(&tmp, &dir)?;
    Ok(integrity_hint.unwrap_or(&computed_integrity).to_string())
}

/// Return all cached semantic versions for a given package, sorted descending.
pub fn cached_versions(name: &str) -> Vec<Version> {
    let mut root = cache_root();
    root.push("pkgs");
    for part in name.split('/') {
        root.push(part);
    }
    let mut out: Vec<Version> = Vec::new();
    if let Ok(rd) = fs::read_dir(&root) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                if let Some(ver_str) = p.file_name().and_then(|o| o.to_str()) {
                    if let Ok(v) = Version::parse(ver_str) {
                        out.push(v);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.cmp(a));
    out
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct CachedManifest {
    #[serde(default)]
    pub name: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub dependencies: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    pub optional_dependencies: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "peerDependenciesMeta")]
    pub peer_dependencies_meta: std::collections::BTreeMap<String, PeerMeta>,
    #[serde(default)]
    pub bin: Option<BinField>,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default, rename = "cpu")]
    pub cpu_arch: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum BinField {
    Single(String),
    Map(std::collections::BTreeMap<String, String>),
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PeerMeta {
    #[serde(default)]
    pub optional: bool,
}

/// Read the cached package.json for a cached package, returning minimal fields.
pub fn read_cached_manifest(name: &str, version: &str) -> Result<CachedManifest> {
    let mut p = cache_package_path(name, version);
    p.push("package.json");
    let txt = fs::read_to_string(&p)
        .with_context(|| format!("read cached package.json at {}", p.display()))?;
    let mf: CachedManifest = serde_json::from_str(&txt)
        .with_context(|| format!("parse cached package.json at {}", p.display()))?;
    Ok(mf)
}
