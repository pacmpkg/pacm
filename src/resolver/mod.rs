use anyhow::{anyhow, Result};
use semver::{Version, VersionReq};
use std::collections::BTreeMap;
use std::str::FromStr;

#[derive(Debug)]
pub struct Resolver;

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        Self
    }

    pub fn pick_version(
        &self,
        versions: &BTreeMap<Version, String>,
        range: &str,
    ) -> Result<(Version, String)> {
        let norm = canonicalize_npm_range(range);
        // Support npm-style OR sets using '||' by evaluating as union of sub-ranges
        let is_or = range.contains("||") || norm.contains("||");
        let reqs: Vec<VersionReq> = if is_or {
            let parts: Vec<&str> = range
                .split("||")
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .collect();
            let mut v: Vec<VersionReq> = Vec::new();
            for p in parts {
                let pnorm = canonicalize_npm_range(p);
                if pnorm == "*" {
                    v.push(VersionReq::STAR);
                } else {
                    match VersionReq::from_str(&pnorm) {
                        Ok(r) => v.push(r),
                        Err(e) => {
                            return Err(anyhow!(
                                "invalid semver sub-range '{}' (orig '{}'): {}",
                                pnorm,
                                p,
                                e
                            ))
                        }
                    }
                }
            }
            if v.is_empty() {
                return Err(anyhow!("empty OR range '{}'", range));
            }
            v
        } else {
            let req = if norm == "*" {
                VersionReq::STAR
            } else {
                VersionReq::from_str(&norm).map_err(|e| {
                    anyhow!("invalid semver range '{}' (orig '{}'): {}", norm, range, e)
                })?
            };
            vec![req]
        };
        let mut candidates: Vec<_> = versions.iter().collect();
        candidates.sort_by(|a, b| b.0.cmp(a.0)); // descending
        for (ver, tarball) in candidates {
            // Any-of matching for OR sets; single element behaves as before
            if reqs.iter().any(|r| r.matches(ver)) {
                return Ok((ver.clone(), tarball.clone()));
            }
        }
        Err(anyhow!("no version matches range {}", range))
    }
}

pub fn map_versions(meta: &crate::fetch::NpmMetadata) -> BTreeMap<Version, String> {
    let mut map = BTreeMap::new();
    for v in meta.versions.values() {
        if let Ok(ver) = Version::parse(&v.version) {
            map.insert(ver, v.dist.tarball.clone());
        }
    }
    map
}

pub fn canonicalize_npm_range(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() || s == "*" || s == "latest" {
        return "*".into();
    }

    // Preserve npm-style OR sets. We'll evaluate them in the resolver by splitting.
    if s.contains("||") {
        return s.to_string();
    }

    // If it parses as a full semver (including prerelease/build), treat as exact
    if semver::Version::parse(s).is_ok() {
        return format!("={s}");
    }

    // Hyphen range: "1.2.3 - 2.3.4" => ">=1.2.3, <=2.3.4"
    if let Some(idx) = s.find(" - ") {
        // require spaces around - to avoid confusion with prerelease
        let (a, b) = s.split_at(idx);
        let b = &b[3..];
        let left = a.trim();
        let right = b.trim();
        if is_version_like(left) && is_version_like(right) {
            return format!(">={left}, <={right}");
        }
    }

    // Tokenize by whitespace and reconstruct comparators, inserting commas.
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() > 1 {
        // Build comparators vector
        let mut comps: Vec<String> = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            let t = tokens[i];
            let next = tokens.get(i + 1).copied();
            if is_op(t) {
                if let Some(ver) = next {
                    comps.push(format!("{t}{ver}"));
                    i += 2;
                    continue;
                }
                // dangling operator – fall back to original string
                return s.to_string();
            }
            // Possibly version separated from next comparator – treat as exact or wildcard pattern
            if is_version_like(t) {
                // Handle wildcard expansions
                if t.contains('x') || t.contains('X') || t.contains('*') {
                    return expand_wildcard(t);
                }
                // Bare major / major.minor expansions handled below
                if is_numeric(t) {
                    return format!("^{t}.0.0");
                }
                if count_dots(t) == 1 && t.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    // major.minor
                    let (maj, min) = {
                        let mut parts = t.split('.');
                        (parts.next().unwrap(), parts.next().unwrap())
                    };
                    // >=maj.min.0 <maj.(min+1).0
                    if let Ok(min_i) = min.parse::<u64>() {
                        return format!(">={maj}.{min}.0, <{maj}.{}.0", min_i + 1);
                    }
                }
                comps.push(format!("={t}"));
                i += 1;
                continue;
            }
            // Unknown token – give up and return original
            return s.to_string();
        }
        if !comps.is_empty() {
            let joined = comps.join(", ");
            return joined;
        }
    }

    // Fallback expansions for simple patterns
    if is_numeric(s) {
        return format!("^{s}.0.0");
    }
    if s.ends_with(".x") || s.ends_with(".*") {
        return expand_wildcard(s);
    }
    // Try as-is; if semver crate would reject we'll let caller produce error.
    s.to_string()
}

fn is_op(t: &str) -> bool {
    matches!(t, ">" | "<" | ">=" | "<=" | "=" | "^" | "~")
}
fn is_numeric(t: &str) -> bool {
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
}
fn count_dots(t: &str) -> usize {
    t.chars().filter(|&c| c == '.').count()
}
fn is_version_like(t: &str) -> bool {
    let mut has_digit = false;
    for c in t.chars() {
        if c.is_ascii_digit() {
            has_digit = true;
            continue;
        }
        if !matches!(c, '.' | '-' | 'x' | 'X' | '*' | 'a'..='z' | 'A'..='Z') {
            return false;
        }
    }
    has_digit
}

fn expand_wildcard(pattern: &str) -> String {
    // Patterns: 1.x, 1.2.x, 1.* etc
    let parts: Vec<&str> = pattern.split('.').collect();
    if parts.is_empty() {
        return pattern.to_string();
    }
    if parts.len() == 2 && (parts[1].eq_ignore_ascii_case("x") || parts[1] == "*") {
        if let Ok(maj) = parts[0].parse::<u64>() {
            return format!(">={maj}.0.0, <{} .0.0", maj + 1)
                .replace("  ", " ")
                .replace(" .", ".");
        }
    }
    if parts.len() == 3 && (parts[2].eq_ignore_ascii_case("x") || parts[2] == "*") {
        if let (Ok(maj), Ok(min)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
            return format!(">={maj}.{min}.0, <{maj}.{}.0", min + 1);
        }
    }
    // Fallback return original
    pattern.to_string()
}
