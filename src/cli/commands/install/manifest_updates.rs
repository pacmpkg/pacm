use crate::fetch::Fetcher;
use crate::manifest::{self, Manifest};
use anyhow::{Context, Result};

pub(super) fn update_manifest_for_specs(
    specs: &[String],
    manifest: &mut Manifest,
    manifest_path: &std::path::Path,
    dev: bool,
    optional: bool,
    no_save: bool,
) -> Result<()> {
    if specs.is_empty() {
        return Ok(());
    }

    let fetcher = if no_save { None } else { Some(Fetcher::new(None)?) };

    for spec in specs {
        let (name, req) = parse_spec(spec);
        let resolved_version = if no_save {
            req.clone()
        } else {
            resolve_version_for_manifest(&name, &req, fetcher.as_ref())?
        };
        if !no_save {
            crate::cli::commands::install::util::add_spec_with_version(
                manifest,
                &name,
                &resolved_version,
                dev,
                optional,
            )?;
        }
    }

    if !no_save {
        manifest::write(manifest, manifest_path)?;
    }

    Ok(())
}

pub fn parse_spec(spec: &str) -> (String, String) {
    if spec.starts_with('@') {
        if let Some(idx) = spec.rfind('@') {
            if idx == 0 {
                return (spec.to_string(), "*".to_string());
            }
            let (name, range) = spec.split_at(idx);
            return (name.to_string(), range[1..].to_string());
        }
    } else if let Some((name, range)) = spec.split_once('@') {
        let range = if range.is_empty() { "*" } else { range };
        return (name.to_string(), range.to_string());
    }

    (spec.to_string(), "*".to_string())
}

fn resolve_version_for_manifest(
    name: &str,
    req: &str,
    fetcher: Option<&Fetcher>,
) -> Result<String> {
    let req_trimmed = req.trim();
    let cached_versions = crate::cache::cached_versions(name);

    if req_trimmed == "*" {
        if let Some(version) = cached_versions.first() {
            return Ok(version.to_string());
        }
    } else if let Ok(range) =
        semver::VersionReq::parse(&crate::resolver::canonicalize_npm_range(req_trimmed))
    {
        if let Some(version) =
            cached_versions.into_iter().find(|candidate| range.matches(candidate))
        {
            return Ok(version.to_string());
        }
    }

    if let Some(fetcher) = fetcher {
        if req_trimmed.eq_ignore_ascii_case("latest") || req_trimmed == "*" {
            let meta = fetcher
                .package_version_metadata(name, "latest")
                .with_context(|| format!("fetch metadata for {name}"))?;
            Ok(meta.version)
        } else if crate::cli::commands::install::util::looks_like_dist_tag(req_trimmed) {
            let meta = fetcher
                .package_version_metadata(name, req_trimmed)
                .with_context(|| format!("fetch metadata for {name}"))?;
            Ok(meta.version)
        } else {
            Ok(req_trimmed.to_string())
        }
    } else {
        Ok(req_trimmed.to_string())
    }
}
