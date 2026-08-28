use bstr::ByteSlice;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub vcs_hint: Option<String>,
    pub https_url: String,
    pub ssh_url: String,
    pub original_input: String,
}

#[derive(Debug, thiserror::Error)]
pub enum UrlError {
    #[error("empty input")]
    Empty,
    #[error("path traversal in input: {0:?}")]
    PathTraversal(String),
    #[error("malformed URL {input:?}: {reason}")]
    Malformed { input: String, reason: String },
    #[error("could not infer owner for {input:?}; set ghq.user in your gitconfig or pass --me")]
    UnknownUser { input: String },
    #[error("URL has no host: {0:?}")]
    MissingHost(String),
    #[error("URL has no repository path: {0:?}")]
    MissingPath(String),
}

pub fn from_input(
    input: &str,
    scap_user: Option<&str>,
    complete_user: bool,
) -> Result<Repo, UrlError> {
    if input.is_empty() {
        return Err(UrlError::Empty);
    }
    if has_traversal(input) {
        return Err(UrlError::PathTraversal(input.to_owned()));
    }
    let original_input = input.to_owned();

    if let Some((scheme, region, user, repo)) = parse_codecommit(input) {
        return finalize_codecommit(&original_input, scheme, region, user, repo);
    }

    let normalized = normalize_to_parseable(input)?;
    if matches!(normalized.kind, NormalizedKind::Bare) {
        return from_bare(&original_input, &normalized.value, scap_user, complete_user);
    }

    let parsed = gix_url::parse(normalized.value.as_bytes().as_bstr()).map_err(|e| {
        UrlError::Malformed { input: original_input.clone(), reason: e.to_string() }
    })?;

    let host = match parsed.host() {
        Some(h) => h.to_ascii_lowercase(),
        None if matches!(parsed.scheme, gix_url::Scheme::File) => String::new(),
        None => return Err(UrlError::MissingHost(original_input.clone())),
    };

    let raw_path = parsed.path.to_str_lossy();
    let trimmed = trim_repo_path(&raw_path);
    if trimmed.is_empty() {
        return Err(UrlError::MissingPath(original_input.clone()));
    }
    let segments: Vec<&str> = trimmed.split('/').collect();
    if segments.len() < 2 {
        return Err(UrlError::Malformed {
            input: original_input,
            reason: format!("expected owner/repo, got {trimmed:?}"),
        });
    }
    let name = segments[segments.len() - 1].to_owned();
    let owner = segments[..segments.len() - 1].join("/");

    let vcs_hint = detect_vcs(&parsed.scheme, &host);
    let ssh_user = parsed.user().unwrap_or("git").to_owned();

    let https_url = format!("https://{host}/{owner}/{name}");
    let ssh_url = format!("ssh://{ssh_user}@{host}/{owner}/{name}");

    Ok(Repo { host, owner, name, vcs_hint, https_url, ssh_url, original_input })
}

fn detect_vcs(scheme: &gix_url::Scheme, host: &str) -> Option<String> {
    use gix_url::Scheme;
    match scheme {
        Scheme::Ext(name) => {
            let n = name.to_ascii_lowercase();
            if n.starts_with("svn") {
                Some("svn".into())
            } else if n == "hg" || n.starts_with("hg+") {
                Some("hg".into())
            } else {
                Some(n)
            }
        }
        _ => {
            if host == "svn.code.sf.net" || host == "subversion.assembla.com" {
                Some("svn".into())
            } else if host.contains("hg.") {
                Some("hg".into())
            } else {
                None
            }
        }
    }
}

fn trim_repo_path(p: &str) -> String {
    let trimmed = p.trim_matches('/');
    let without_git = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    without_git.trim_matches('/').to_owned()
}

fn has_traversal(input: &str) -> bool {
    input.split(['/', '\\']).any(|seg| seg == "..")
}

#[derive(Debug)]
struct Normalized {
    value: String,
    kind: NormalizedKind,
}

#[derive(Debug)]
enum NormalizedKind {
    HasScheme,
    Scp,
    HostBare,
    Bare,
}

fn normalize_to_parseable(input: &str) -> Result<Normalized, UrlError> {
    if has_scheme(input) {
        return Ok(Normalized { value: input.to_owned(), kind: NormalizedKind::HasScheme });
    }
    if let Some((user, host, path)) = parse_scp_like(input) {
        let user_prefix = match user {
            Some(u) => format!("{u}@"),
            None => String::new(),
        };
        let path = path.trim_start_matches('/');
        return Ok(Normalized {
            value: format!("ssh://{user_prefix}{host}/{path}"),
            kind: NormalizedKind::Scp,
        });
    }
    let first = input.split('/').next().unwrap_or("");
    if looks_like_authority(first) && input.contains('/') {
        return Ok(Normalized {
            value: format!("https://{input}"),
            kind: NormalizedKind::HostBare,
        });
    }
    Ok(Normalized { value: input.to_owned(), kind: NormalizedKind::Bare })
}

fn has_scheme(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b':' {
            return i + 2 < bytes.len() && bytes[i + 1] == b'/' && bytes[i + 2] == b'/';
        }
        if !(c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.') {
            return false;
        }
        i += 1;
    }
    false
}

fn parse_scp_like(s: &str) -> Option<(Option<&str>, &str, &str)> {
    let colon = s.find(':')?;
    let (left, right) = (&s[..colon], &s[colon + 1..]);
    if left.is_empty() || right.is_empty() {
        return None;
    }
    if left.contains('/') {
        return None;
    }
    let (user, host) = match left.rfind('@') {
        Some(idx) => (Some(&left[..idx]), &left[idx + 1..]),
        None => (None, left),
    };
    if host.is_empty() {
        return None;
    }
    Some((user, host, right))
}

fn looks_like_authority(s: &str) -> bool {
    let host = s.split(':').next().unwrap_or(s);
    if !host.contains('.') {
        return false;
    }
    let mut saw_dot = false;
    for (i, c) in host.char_indices() {
        if c == '.' {
            if i == 0 {
                return false;
            }
            saw_dot = true;
        } else if !(c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }
    saw_dot
}

fn from_bare(
    original: &str,
    value: &str,
    scap_user: Option<&str>,
    complete_user: bool,
) -> Result<Repo, UrlError> {
    let trimmed = value.trim_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    match segments.len() {
        0 => Err(UrlError::Empty),
        1 => {
            let project = segments[0];
            let (owner, name) = if complete_user {
                let user = scap_user
                    .ok_or_else(|| UrlError::UnknownUser { input: original.to_owned() })?;
                (user.to_owned(), project.to_owned())
            } else {
                (project.to_owned(), project.to_owned())
            };
            Ok(build_github_repo(original, &owner, &name))
        }
        2 => Ok(build_github_repo(original, segments[0], segments[1])),
        _ => Err(UrlError::Malformed {
            input: original.to_owned(),
            reason: format!("bare input has {} path segments", segments.len()),
        }),
    }
}

fn build_github_repo(original: &str, owner: &str, name: &str) -> Repo {
    let host = "github.com".to_owned();
    Repo {
        https_url: format!("https://{host}/{owner}/{name}"),
        ssh_url: format!("ssh://git@{host}/{owner}/{name}"),
        host,
        owner: owner.to_owned(),
        name: name.to_owned(),
        vcs_hint: None,
        original_input: original.to_owned(),
    }
}

fn parse_codecommit(input: &str) -> Option<(String, Option<String>, Option<String>, String)> {
    let rest = input.strip_prefix("codecommit:")?;
    let (region, rest) = if let Some(after) = rest.strip_prefix(':') {
        let end = after.find("://")?;
        (Some(after[..end].to_owned()), &after[end + 3..])
    } else {
        (None, rest.strip_prefix("//")?)
    };
    if rest.is_empty() {
        return None;
    }
    let (user, repo) = match rest.rfind('@') {
        Some(idx) => (Some(rest[..idx].to_owned()), rest[idx + 1..].to_owned()),
        None => (None, rest.to_owned()),
    };
    if repo.is_empty() {
        return None;
    }
    Some(("codecommit".to_owned(), region, user, repo))
}

fn finalize_codecommit(
    original: &str,
    _scheme: String,
    region: Option<String>,
    user: Option<String>,
    repo_name: String,
) -> Result<Repo, UrlError> {
    let host = match region {
        Some(r) => r,
        None => "codecommit".to_owned(),
    };
    let owner = user.unwrap_or_else(|| "codecommit".to_owned());
    let name = repo_name;
    let https_url = format!("codecommit://{host}/{owner}/{name}");
    let ssh_url = https_url.clone();
    Ok(Repo {
        host,
        owner,
        name,
        vcs_hint: Some("git".to_owned()),
        https_url,
        ssh_url,
        original_input: original.to_owned(),
    })
}

#[cfg(test)]
#[path = "url_tests.rs"]
mod tests;
