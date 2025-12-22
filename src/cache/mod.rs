use crate::fsutil::{cache_root, store_root};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use data_encoding::BASE32_NOPAD;
use flate2::read::GzDecoder;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tar::Archive;
use walkdir::WalkDir;

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
            let raw = STANDARD.decode(b64).with_context(|| "decode integrity base64")?;
            if raw != digest[..] {
                anyhow::bail!("integrity mismatch: expected {integrity}, got {computed_integrity}");
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
        if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
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
    #[serde(default, deserialize_with = "map_or_empty")]
    pub dependencies: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies", deserialize_with = "map_or_empty")]
    pub dev_dependencies: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies", deserialize_with = "map_or_empty")]
    pub optional_dependencies: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies", deserialize_with = "map_or_empty")]
    pub peer_dependencies: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "peerDependenciesMeta")]
    pub peer_dependencies_meta: std::collections::BTreeMap<String, PeerMeta>,
    #[serde(default)]
    pub bin: Option<BinField>,
    #[serde(default)]
    pub scripts: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default, rename = "cpu")]
    pub cpu_arch: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
#[allow(dead_code)]
enum MapOrSeq {
    Map(std::collections::BTreeMap<String, String>),
    Seq(Vec<serde_json::Value>),
    Null(Option<()>),
}

fn map_or_empty<'de, D>(
    deserializer: D,
) -> std::result::Result<std::collections::BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = MapOrSeq::deserialize(deserializer)?;
    match v {
        MapOrSeq::Map(m) => Ok(m),
        MapOrSeq::Seq(_) | MapOrSeq::Null(_) => Ok(std::collections::BTreeMap::new()),
    }
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

#[derive(Debug, Clone)]
pub struct DependencyFingerprint {
    pub name: String,
    pub version: String,
    pub store_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredDependency {
    pub name: String,
    pub version: String,
    pub store_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreMetadata {
    store_key: String,
    name: String,
    version: String,
    graph_hash: String,
    content_hash: String,
    size: u64,
    created_at: u64,
    integrity: Option<String>,
    resolved: Option<String>,
    dependencies: Vec<StoredDependency>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub scripts: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct StoreEntry {
    pub store_key: String,
    pub name: String,
    pub version: String,
    pub graph_hash: String,
    pub content_hash: String,
    pub size: u64,
    pub integrity: Option<String>,
    pub resolved: Option<String>,
    pub created_at: u64,
    pub dependencies: Vec<StoredDependency>,
    pub root_dir: PathBuf,
    pub package_dir: PathBuf,
    pub metadata_path: PathBuf,
}

impl StoreEntry {
    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }
}

#[derive(Debug, Clone)]
pub struct EnsureParams<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub dependencies: &'a [DependencyFingerprint],
    pub source_dir: &'a Path,
    pub integrity: Option<&'a str>,
    pub resolved: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct CasStore {
    root: PathBuf,
    packages_dir: PathBuf,
    tmp_dir: PathBuf,
}

impl CasStore {
    pub fn open() -> Result<Self> {
        let root = store_root();
        let packages_dir = root.join("packages");
        let tmp_dir = root.join("tmp");
        fs::create_dir_all(&packages_dir)
            .with_context(|| format!("create store packages dir at {}", packages_dir.display()))?;
        fs::create_dir_all(&tmp_dir)
            .with_context(|| format!("create store tmp dir at {}", tmp_dir.display()))?;
        Ok(Self { root, packages_dir, tmp_dir })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure_entry(&self, params: &EnsureParams) -> Result<StoreEntry> {
        let (graph_hash, store_key) =
            compute_graph_hash(params.name, params.version, params.dependencies)?;
        let final_dir = self.store_dir_for(params.name, params.version, &graph_hash);
        let metadata_path = final_dir.join("metadata.json");
        if metadata_path.exists() {
            let metadata = read_metadata(&metadata_path)?;
            return Ok(build_store_entry(final_dir, metadata));
        }

        let tmp_target = self.tmp_dir.join(format!(
            "{}-{}-{}",
            sanitize_for_fs(params.name),
            params.version.replace('/', "_"),
            unique_suffix()
        ));
        let tmp_package_dir = tmp_target.join("package");
        fs::create_dir_all(&tmp_package_dir)?;
        copy_tree(params.source_dir, &tmp_package_dir).with_context(|| {
            format!("copy package contents for {}@{} into store", params.name, params.version)
        })?;
        let (content_hash, total_size) = compute_tree_content_hash(&tmp_package_dir)?;
        let metadata = StoreMetadata {
            store_key: store_key.clone(),
            name: params.name.to_string(),
            version: params.version.to_string(),
            graph_hash: graph_hash.clone(),
            content_hash: content_hash.clone(),
            size: total_size,
            created_at: unix_timestamp()?,
            integrity: params.integrity.map(|s| s.to_string()),
            resolved: params.resolved.map(|s| s.to_string()),
            dependencies: params
                .dependencies
                .iter()
                .map(|d| StoredDependency {
                    name: d.name.clone(),
                    version: d.version.clone(),
                    store_key: d.store_key.clone(),
                })
                .collect(),
            scripts: {
                // Attempt to read registry scripts sidecar from the source_dir (cache package path)
                let mut scripts_map = std::collections::BTreeMap::new();
                let sidecar = params.source_dir.join(".registry-scripts.json");
                if sidecar.exists() {
                    if let Ok(txt) = std::fs::read_to_string(&sidecar) {
                        if let Ok(parsed) =
                            serde_json::from_str::<std::collections::BTreeMap<String, String>>(&txt)
                        {
                            scripts_map = parsed;
                        }
                    }
                }
                scripts_map
            },
        };
        let metadata_tmp_path = tmp_target.join("metadata.json");
        write_metadata(&metadata_tmp_path, &metadata)?;

        if let Some(parent) = final_dir.parent() {
            fs::create_dir_all(parent)?;
        }

        match fs::rename(&tmp_target, &final_dir) {
            Ok(()) => {}
            Err(rename_err) => {
                if metadata_path.exists() {
                    fs::remove_dir_all(&tmp_target).ok();
                    let metadata = read_metadata(&metadata_path)?;
                    return Ok(build_store_entry(final_dir, metadata));
                }
                return Err(rename_err.into());
            }
        }

        Ok(StoreEntry {
            store_key,
            name: metadata.name.clone(),
            version: metadata.version.clone(),
            graph_hash,
            content_hash,
            size: metadata.size,
            integrity: metadata.integrity.clone(),
            resolved: metadata.resolved.clone(),
            created_at: metadata.created_at,
            dependencies: metadata.dependencies.clone(),
            root_dir: final_dir.clone(),
            package_dir: final_dir.join("package"),
            metadata_path,
        })
    }

    pub fn load_entry(&self, store_key: &str) -> Result<Option<StoreEntry>> {
        let Some((name, version, graph_hash)) = split_store_key(store_key) else {
            return Ok(None);
        };
        let dir = self.store_dir_for(&name, &version, &graph_hash);
        let metadata_path = dir.join("metadata.json");
        if !metadata_path.exists() {
            return Ok(None);
        }
        let metadata = read_metadata(&metadata_path)?;
        Ok(Some(build_store_entry(dir, metadata)))
    }

    fn store_dir_for(&self, name: &str, version: &str, graph_hash: &str) -> PathBuf {
        let mut dir = self.packages_dir.clone();
        let mut parts: Vec<&str> = name.split('/').collect();
        if let Some(last) = parts.pop() {
            for part in parts {
                dir.push(part);
            }
            dir.push(format!("{last}@{version}_{graph_hash}"));
        } else {
            dir.push(format!("{name}@{version}_{graph_hash}"));
        }
        dir
    }
}

fn build_store_entry(dir: PathBuf, metadata: StoreMetadata) -> StoreEntry {
    StoreEntry {
        store_key: metadata.store_key.clone(),
        name: metadata.name.clone(),
        version: metadata.version.clone(),
        graph_hash: metadata.graph_hash.clone(),
        content_hash: metadata.content_hash.clone(),
        size: metadata.size,
        integrity: metadata.integrity.clone(),
        resolved: metadata.resolved.clone(),
        created_at: metadata.created_at,
        dependencies: metadata.dependencies.clone(),
        root_dir: dir.clone(),
        package_dir: dir.join("package"),
        metadata_path: dir.join("metadata.json"),
    }
}

fn compute_graph_hash(
    name: &str,
    version: &str,
    deps: &[DependencyFingerprint],
) -> Result<(String, String)> {
    #[derive(Serialize)]
    struct GraphItem<'a> {
        name: &'a str,
        version: &'a str,
        store_key: Option<&'a str>,
    }

    let mut items: Vec<GraphItem<'_>> = deps
        .iter()
        .map(|d| GraphItem {
            name: d.name.as_str(),
            version: d.version.as_str(),
            store_key: d.store_key.as_deref(),
        })
        .collect();
    items.sort_by(|a, b| a.name.cmp(b.name));
    let serialized = serde_json::to_vec(&items)?;
    let mut hasher = Sha256::new();
    hasher.update(serialized);
    let digest = hasher.finalize();
    let graph_hash = BASE32_NOPAD.encode(&digest);
    let store_key = format!("{name}@{version}::{graph_hash}");
    Ok((graph_hash, store_key))
}

fn read_metadata(path: &Path) -> Result<StoreMetadata> {
    let txt = fs::read_to_string(path)?;
    let metadata: StoreMetadata = serde_json::from_str(&txt)?;
    Ok(metadata)
}

fn write_metadata(path: &Path, metadata: &StoreMetadata) -> Result<()> {
    let txt = serde_json::to_string_pretty(metadata)?;
    fs::write(path, txt)?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in WalkDir::new(from).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(from)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let dest = to.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(entry.path(), &dest)?;
        let perms = entry.metadata()?.permissions();
        fs::set_permissions(&dest, perms)?;
    }
    Ok(())
}

fn compute_tree_content_hash(root: &Path) -> Result<(String, u64)> {
    #[derive(Debug)]
    struct ContentEntry {
        path: String,
        kind: u8,
        size: u64,
        readonly: bool,
        digest: Option<[u8; 32]>,
    }

    let mut entries: Vec<ContentEntry> = Vec::new();
    let mut total_size: u64 = 0;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(root)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let meta = entry.metadata()?;
        if entry.file_type().is_dir() {
            entries.push(ContentEntry {
                path: rel_str,
                kind: b'd',
                size: 0,
                readonly: meta.permissions().readonly(),
                digest: None,
            });
            continue;
        }
        let mut file = fs::File::open(entry.path())?;
        let mut f_hasher = Sha256::new();
        let mut buf = [0u8; 8192];
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            f_hasher.update(&buf[..read]);
        }
        let digest = f_hasher.finalize();
        let mut digest_bytes = [0u8; 32];
        digest_bytes.copy_from_slice(&digest);
        let size = meta.len();
        total_size = total_size.saturating_add(size);
        entries.push(ContentEntry {
            path: rel_str,
            kind: b'f',
            size,
            readonly: meta.permissions().readonly(),
            digest: Some(digest_bytes),
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hasher = Sha256::new();
    for entry in &entries {
        hasher.update(entry.path.as_bytes());
        hasher.update([0u8]);
        hasher.update([entry.kind]);
        hasher.update(entry.size.to_le_bytes());
        hasher.update([if entry.readonly { 1u8 } else { 0u8 }]);
        if let Some(digest) = &entry.digest {
            hasher.update(digest);
        }
    }
    let digest = hasher.finalize();
    Ok((hex::encode(digest), total_size))
}

fn unix_timestamp() -> Result<u64> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(now.as_secs())
}

fn split_store_key(store_key: &str) -> Option<(String, String, String)> {
    let (prefix, graph_hash) = store_key.split_once("::")?;
    let (name, version) = prefix.rsplit_once('@')?;
    Some((name.to_string(), version.to_string(), graph_hash.to_string()))
}

fn sanitize_for_fs(name: &str) -> String {
    let mut sanitized = name.replace(['/', '\\'], "_");
    if sanitized.is_empty() {
        sanitized.push('_');
    }
    sanitized
}

fn unique_suffix() -> String {
    unix_timestamp()
        .map(|ts| format!("{}-{:x}", ts, std::process::id()))
        .unwrap_or_else(|_| format!("fallback-{:x}", std::process::id()))
}
