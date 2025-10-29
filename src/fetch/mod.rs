use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

static CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("pacm/0.1.0 (+https://github.com/pacmpkg/pacm)")
        .build()
        .expect("http client")
});

static META_CACHE: Lazy<Mutex<HashMap<String, NpmMetadata>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static VERSION_META_CACHE: Lazy<Mutex<HashMap<String, NpmVersion>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub struct Fetcher {
    registry: String,
}

impl Fetcher {
    pub fn new(registry: Option<String>) -> Result<Self> {
        Ok(Self { registry: registry.unwrap_or_else(|| "https://registry.npmjs.org".into()) })
    }

    pub fn package_metadata(&self, name: &str) -> Result<NpmMetadata> {
        if let Some(hit) = META_CACHE.lock().unwrap().get(name).cloned() {
            return Ok(hit);
        }
        let url = format!("{}/{}", self.registry, name);
        let resp = CLIENT.get(&url).send().with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            anyhow::bail!("registry returned {} for {}", resp.status(), name);
        }
        let meta: NpmMetadata = resp.json()?;
        META_CACHE.lock().unwrap().insert(name.to_string(), meta.clone());
        Ok(meta)
    }

    pub fn package_version_metadata(&self, name: &str, spec: &str) -> Result<NpmVersion> {
        let trimmed = spec.trim();
        let key = format!("{name}@{trimmed}");
        if let Some(hit) = VERSION_META_CACHE.lock().unwrap().get(&key).cloned() {
            return Ok(hit);
        }
        let url = format!("{}/{}/{}", self.registry, name, trimmed);
        let resp = CLIENT.get(&url).send().with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            anyhow::bail!("registry returned {} for {}@{}", resp.status(), name, trimmed);
        }
        let meta: NpmVersion = resp.json()?;
        VERSION_META_CACHE.lock().unwrap().insert(key, meta.clone());
        Ok(meta)
    }

    pub fn download_tarball(&self, url: &str) -> Result<Vec<u8>> {
        let resp = CLIENT.get(url).send().with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            anyhow::bail!("tarball fetch {} status {}", url, resp.status());
        }
        let bytes = resp.bytes()?;
        Ok(bytes.to_vec())
    }

    /// Stream a tarball while invoking a callback with (downloaded_bytes, total_opt). Returns bytes.
    pub fn download_tarball_stream<F>(&self, url: &str, mut on_progress: F) -> Result<Vec<u8>>
    where
        F: FnMut(u64, Option<u64>),
    {
        use std::io::Read;
        let mut resp = CLIENT.get(url).send().with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            anyhow::bail!("tarball fetch {} status {}", url, resp.status());
        }
        let total = resp.content_length();
        let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
        let mut downloaded: u64 = 0;
        let mut tmp = [0u8; 32 * 1024];
        on_progress(0, total);
        loop {
            let n = resp.read(&mut tmp)?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            downloaded += n as u64;
            // Throttle updates: every 64KiB or on completion
            if downloaded % (64 * 1024) < n as u64
                || total.map(|t| downloaded >= t).unwrap_or(false)
            {
                on_progress(downloaded, total);
            }
        }
        Ok(buf)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct NpmMetadata {
    #[serde(rename = "dist-tags")]
    pub dist_tags: Option<std::collections::HashMap<String, String>>,
    pub versions: std::collections::HashMap<String, NpmVersion>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NpmVersion {
    pub version: String,
    pub dist: NpmDist,
    #[serde(default)]
    pub dependencies: std::collections::HashMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    pub optional_dependencies: std::collections::HashMap<String, String>,
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: std::collections::HashMap<String, String>,
    #[serde(default, rename = "peerDependenciesMeta")]
    pub peer_dependencies_meta: std::collections::HashMap<String, PeerMeta>,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default, rename = "cpu")]
    pub cpu_arch: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NpmDist {
    pub tarball: String,
    pub integrity: Option<String>,
    pub shasum: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PeerMeta {
    #[serde(default)]
    pub optional: bool,
}
