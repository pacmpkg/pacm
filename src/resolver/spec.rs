use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSpec {
    Registry { range: String },
    Github(GithubSpec),
    Tarball { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubSpec {
    pub owner: String,
    pub repo: String,
    pub reference: Option<String>,
}

impl PackageSpec {
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();

        if let Some(rest) = trimmed.strip_prefix("npm:") {
            return PackageSpec::Registry { range: rest.to_string() };
        }

        if let Some(rest) = trimmed.strip_prefix("github:") {
            if let Some(spec) = parse_github(rest) {
                return PackageSpec::Github(spec);
            }
        }

        if let Some(spec) = parse_github(trimmed) {
            return PackageSpec::Github(spec);
        }

        if let Some(rest) = trimmed.strip_prefix("git+") {
            if is_http_url(rest) {
                return PackageSpec::Tarball { url: rest.to_string() };
            }
        }

        if is_http_url(trimmed) {
            return PackageSpec::Tarball { url: trimmed.to_string() };
        }

        PackageSpec::Registry { range: trimmed.to_string() }
    }
}

fn parse_github(input: &str) -> Option<GithubSpec> {
    if input.starts_with('@') || input.contains(' ') {
        return None;
    }

    let (path, reference) = if let Some((lhs, rhs)) = input.split_once('#') {
        (lhs, Some(rhs.trim().to_string()))
    } else {
        (input, None)
    };

    let mut parts = path.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }

    Some(GithubSpec {
        owner: owner.to_string(),
        repo: repo.trim_end_matches(".git").to_string(),
        reference,
    })
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

/// Try to infer a package name when the spec refers to a non-registry source.
pub fn guess_name_from_spec(raw: &str) -> Option<String> {
    match PackageSpec::parse(raw) {
        PackageSpec::Github(spec) => {
            let mut repo_part = spec.repo.as_str();
            if let Some(idx) = repo_part.rfind('/') {
                repo_part = &repo_part[idx + 1..];
            }
            Some(repo_part.to_string())
        }
        PackageSpec::Tarball { url } => {
            let trimmed = url.split('?').next().unwrap_or(&url);
            if let Some(file) = trimmed.rsplit('/').next() {
                let file = file.trim_end_matches(".tar.gz");
                let file = file.trim_end_matches(".tgz");
                let file = file.trim_end_matches(".tar");
                if !file.is_empty() {
                    return Some(file.to_string());
                }
            }
            None
        }
        PackageSpec::Registry { .. } => None,
    }
}

impl GithubSpec {
    pub fn display_ref(&self) -> Option<Cow<'_, str>> {
        self.reference.as_deref().map(Cow::Borrowed)
    }
}
