use crate::manifest::{self, Manifest};
use anyhow::{Context, Result};
use glob::glob;
use semver::{Version, VersionReq};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub name: String,
    pub version: String,
    pub dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
    pub relative_path: String,
}

pub fn discover_workspaces(root: &Path, manifest: &Manifest) -> Result<Vec<WorkspaceInfo>> {
    if manifest.workspaces.is_empty() {
        return Ok(Vec::new());
    }

    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
    let mut by_name: BTreeMap<String, WorkspaceInfo> = BTreeMap::new();

    for pattern in manifest.workspaces.packages() {
        if pattern.trim().is_empty() {
            continue;
        }
        let abs_pattern = root.join(pattern);
        let pat_str = abs_pattern.to_string_lossy().replace('\\', "/");
        for entry in glob(&pat_str).with_context(|| format!("expand workspace pattern {pat_str}"))? {
            let path = match entry {
                Ok(p) => p,
                Err(e) => return Err(e.into()),
            };
            let pkg_dir = if path.is_file() {
                if path.file_name().map(|n| n == "package.json").unwrap_or(false) {
                    path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.clone())
                } else {
                    path.clone()
                }
            } else {
                path.clone()
            };
            if pkg_dir == root {
                continue;
            }
            let manifest_path = pkg_dir.join("package.json");
            if !manifest_path.exists() {
                continue;
            }
            let canon = manifest_path
                .canonicalize()
                .unwrap_or_else(|_| manifest_path.clone());
            if !seen_dirs.insert(canon.clone()) {
                continue;
            }
            let pkg_manifest = manifest::load(&manifest_path)
                .with_context(|| format!("load workspace manifest at {}", manifest_path.display()))?;
            if pkg_manifest.name.is_empty() {
                anyhow::bail!("workspace at {} has empty name", manifest_path.display());
            }
            let rel = rel_path_str(root, &pkg_dir);
            let info = WorkspaceInfo {
                name: pkg_manifest.name.clone(),
                version: pkg_manifest.version.clone(),
                dir: pkg_dir.clone(),
                manifest_path: manifest_path.clone(),
                manifest: pkg_manifest,
                relative_path: rel,
            };
            if let Some(prev) = by_name.insert(info.name.clone(), info) {
                anyhow::bail!(
                    "duplicate workspace package name '{}' at {} and {}",
                    prev.name,
                    prev.dir.display(),
                    pkg_dir.display()
                );
            }
        }
    }

    Ok(by_name.into_values().collect())
}

pub fn workspace_dep_satisfies(range: &str, version: &str) -> bool {
    let trimmed = range.trim();
    let spec = trimmed.strip_prefix("workspace:").unwrap_or(trimmed);
    if spec.is_empty() || spec == "*" {
        return true;
    }
    if let Ok(ver) = Version::parse(version) {
        let canon = crate::resolver::canonicalize_npm_range(spec);
        if let Ok(req) = VersionReq::parse(&canon) {
            if req.matches(&ver) {
                return true;
            }
        }
    }
    spec == version
}

fn rel_path_str(root: &Path, dir: &Path) -> String {
    if let Ok(rel) = dir.strip_prefix(root) {
        let s = rel.to_string_lossy();
        let normalized = s.trim_start_matches('.').trim_start_matches(std::path::MAIN_SEPARATOR);
        let clean = if normalized.is_empty() { "." } else { normalized };
        clean.replace('\\', "/")
    } else {
        dir.to_string_lossy().replace('\\', "/")
    }
}
