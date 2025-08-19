use anyhow::{Result, Context};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::Duration;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::collections::HashMap;

static CLIENT: Lazy<Client> = Lazy::new(|| {
	Client::builder()
		.timeout(Duration::from_secs(30))
		.user_agent("pacm/0.1.0 (+https://github.com/infinitejs/pacm)")
		.build()
		.expect("http client")
});

static META_CACHE: Lazy<Mutex<HashMap<String, NpmMetadata>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub struct Fetcher { registry: String }

impl Fetcher {
	pub fn new(registry: Option<String>) -> Result<Self> { Ok(Self { registry: registry.unwrap_or_else(|| "https://registry.npmjs.org".into()) }) }

	pub fn package_metadata(&self, name: &str) -> Result<NpmMetadata> {
		if let Some(hit) = META_CACHE.lock().unwrap().get(name).cloned() { return Ok(hit); }
		let url = format!("{}/{}", self.registry, name);
		let resp = CLIENT.get(&url).send().with_context(|| format!("GET {}", url))?;
		if !resp.status().is_success() { anyhow::bail!("registry returned {} for {}", resp.status(), name); }
		let meta: NpmMetadata = resp.json()?;
		META_CACHE.lock().unwrap().insert(name.to_string(), meta.clone());
		Ok(meta)
	}

	pub fn download_tarball(&self, url: &str) -> Result<Vec<u8>> {
		let resp = CLIENT.get(url).send().with_context(|| format!("GET {}", url))?;
		if !resp.status().is_success() { anyhow::bail!("tarball fetch {} status {}", url, resp.status()); }
		let bytes = resp.bytes()?;
		Ok(bytes.to_vec())
	}
}

#[derive(Debug, Deserialize, Clone)]
pub struct NpmMetadata {
	#[serde(rename = "dist-tags")]
	pub dist_tags: Option<DistTags>,
	pub versions: std::collections::HashMap<String, NpmVersion>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DistTags {
	pub latest: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NpmVersion {
	pub version: String,
	pub dist: NpmDist,
	#[serde(default)]
	pub dependencies: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NpmDist {
	pub tarball: String,
	pub integrity: Option<String>,
	pub shasum: Option<String>,
}

