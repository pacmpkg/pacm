use crate::manifest::Manifest;
use anyhow::Result;

pub(super) fn add_spec_with_version(
    manifest: &mut Manifest,
    name: &str,
    version: &str,
    dev: bool,
    optional: bool,
) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("empty package name");
    }
    if dev {
        manifest.dev_dependencies.insert(name.to_string(), version.to_string());
    } else if optional {
        manifest.optional_dependencies.insert(name.to_string(), version.to_string());
    } else {
        manifest.dependencies.insert(name.to_string(), version.to_string());
    }
    Ok(())
}

pub(super) fn looks_like_dist_tag(spec: &str) -> bool {
    let trimmed = spec.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return false;
    }
    if trimmed.eq_ignore_ascii_case("latest") {
        return false;
    }
    if trimmed.contains(' ') || trimmed.contains("||") || trimmed.contains(',') {
        return false;
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return false;
    }
    let canon = crate::resolver::canonicalize_npm_range(trimmed);
    if canon == "*" {
        return false;
    }
    semver::Version::parse(trimmed).is_err() && semver::VersionReq::parse(&canon).is_err()
}
