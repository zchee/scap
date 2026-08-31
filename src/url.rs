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
    #[error(
        "You must specify a region. You can also configure your region by running \"aws configure\"."
    )]
    MissingCodecommitRegion,
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

    if let Some(codecommit) = parse_codecommit(input) {
        return finalize_codecommit(&original_input, codecommit);
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

/// The capture groups of ghq's CodeCommit pattern, borrowed from the input.
///
/// The numbering is ghq's own (url.go:25, four groups): 1 is the literal
/// scheme `codecommit` and carries nothing, 2 is `region`, 3 is `user` (the
/// optional `<profile>@`) and 4 is `name`. The translation of that pattern in
/// `src/url_tests.rs` keeps groups 1 to 3 and leaves the name uncaptured, so a
/// `caps.get(2)` there is the region, not the name.
struct CodecommitRef<'a> {
    region: Option<&'a str>,
    user: Option<&'a str>,
    name: &'a str,
}

/// Matches `input` against ghq's CodeCommit pattern (url.go:25):
///
/// ```text
/// ^(codecommit):(?::([a-z][a-z0-9-]+):)?//(?:([^]]+)@)?([\w\.-]+)$
/// ```
///
/// with Go's ASCII `\w` (`[A-Za-z0-9_]`), returning its capture groups, or
/// `None` for an input the pattern rejects. Hand-written so the crate carries
/// no runtime regex engine; `src/url_tests.rs` holds a differential test
/// against that pattern compiled with the `regex` dev-dependency.
///
/// This is the crate's *only* CodeCommit recogniser: [`from_input`] dispatches
/// on it and [`is_codecommit_input`] is its predicate, so one grammar decides
/// both where a target is routed and which root resolves it. An input ghq's
/// pattern rejects therefore cannot reach [`finalize_codecommit`], and cannot
/// reach the `aws configure get region` fallback either; it takes the ordinary
/// URL path, which is what ghq does with it (`newURL`, url.go:57-107, runs the
/// pattern first and falls through to `url.Parse` when it misses).
fn parse_codecommit(input: &str) -> Option<CodecommitRef<'_>> {
    let rest = input.strip_prefix("codecommit:")?;

    // Optional `:<region>:`. The region class excludes `:`, so the region can
    // only end at the next colon, and that colon must be followed by `//`.
    let (region, after_region) = match rest.strip_prefix(':') {
        Some(with_region) => {
            let end = with_region.find(':')?;
            let region = &with_region[..end];
            // `[a-z][a-z0-9-]+`: at least two bytes.
            let bytes = region.as_bytes();
            if bytes.len() < 2 || !bytes[0].is_ascii_lowercase() {
                return None;
            }
            let tail_ok = bytes[1..]
                .iter()
                .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !tail_ok {
                return None;
            }
            (Some(region), &with_region[end + 1..])
        }
        None => (None, rest),
    };

    let authority = after_region.strip_prefix("//")?;

    // The host class excludes `@`, so the user/host separator can only be the
    // *last* `@`; ghq's user class `[^]]+` admits `@` and `/` inside the user.
    let (user, name) = match authority.rfind('@') {
        Some(at) => (Some(&authority[..at]), &authority[at + 1..]),
        None => (None, authority),
    };

    if let Some(user) = user
        && (user.is_empty() || user.contains(']'))
    {
        return None;
    }

    // `[\w.-]+`, ASCII: any non-ASCII byte fails here, as it does in Go.
    if name.is_empty()
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return None;
    }

    Some(CodecommitRef { region, user, name })
}

/// Reports whether `input` is a CodeCommit-style URL under ghq's pattern
/// (url.go:25) -- the predicate half of [`parse_codecommit`], which holds the
/// pattern itself.
pub(crate) fn is_codecommit_input(input: &str) -> bool {
    parse_codecommit(input).is_some()
}

/// Builds the destination path components (host/owner/name) for a parsed
/// CodeCommit ref, matching ghq's `LocalRepositoryFromURL`
/// (local_repository.go:75-86):
///
/// ```go
/// pathParts := append(
///     []string{remoteURL.Hostname()}, strings.Split(remoteURL.Path, "/")...,
/// )
/// ```
///
/// `newURL` (url.go:100-106) builds the `*url.URL` for a codecommit ref as
/// `Host: region, User: url.User(user), Path: repoName` -- `Path` has no
/// leading slash and is never more than the bare repo name, so
/// `strings.Split(Path, "/")` is always a one-element slice and `pathParts`
/// is always exactly `[region, repo]`. `User` (the optional `<profile>@`)
/// is parsed into the URL but never read by `LocalRepositoryFromURL`, so it
/// contributes nothing to the destination -- with or without a profile,
/// ghq's destination is `<root>/<region>/<repo>`, never
/// `<root>/<region>/<profile-or-placeholder>/<repo>`. `path::rel_path`
/// already skips empty owner segments, so mirroring this is just: no owner
/// segment, ever.
///
/// Region resolution when the ref omits `::<region>:` mirrors ghq's
/// `AWS_REGION` / `AWS_DEFAULT_REGION` / `aws configure get region` chain
/// (url.go:63-97) via [`resolve_codecommit_region`]; a bare `codecommit`
/// input has no path component of its own to fall back on, so this is the
/// only spelling that can spawn `aws` -- a region-explicit ref never does.
/// Only [`parse_codecommit`] reaches this, so an input outside ghq's pattern
/// reaches neither the destination shape below nor that `aws` fallback.
fn finalize_codecommit(original: &str, codecommit: CodecommitRef<'_>) -> Result<Repo, UrlError> {
    let CodecommitRef { region, user, name: repo_name } = codecommit;
    let host = match region {
        Some(r) => r.to_owned(),
        None => {
            let aws_region = std::env::var("AWS_REGION").ok();
            let aws_default_region = std::env::var("AWS_DEFAULT_REGION").ok();
            resolve_codecommit_region(
                aws_region.as_deref(),
                aws_default_region.as_deref(),
                run_aws_configure_get_region,
            )?
        }
    };
    let owner = String::new();
    let name = repo_name.to_owned();
    let https_url = match &user {
        Some(u) => format!("codecommit://{u}@{host}/{name}"),
        None => format!("codecommit://{host}/{name}"),
    };
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

/// Resolves the region for a codecommit ref that omits `::<region>:`,
/// mirroring ghq's fallback chain (url.go:63-97): `AWS_REGION` if
/// non-empty, else `AWS_DEFAULT_REGION` if non-empty, else the trimmed
/// stdout of `aws configure get region` if that succeeds with non-empty
/// output, else [`UrlError::MissingCodecommitRegion`] with ghq's own
/// message text. `aws_lookup` is called at most once, and only when both
/// env values are absent or empty -- the `aws` subprocess this drives in
/// production ([`run_aws_configure_get_region`]) is never spawned for a
/// region-explicit ref.
///
/// A deliberate simplification from ghq: `os.LookupEnv` treats a variable
/// set to `""` as present and would use that empty string as the region;
/// this checks non-emptiness instead, so an explicitly empty
/// `AWS_REGION=""` falls through to `AWS_DEFAULT_REGION` rather than
/// resolving to an empty host. ghq also forwards the real `aws` CLI's own
/// stderr when it runs but fails; this always reports the same generic
/// message instead, so the failure text does not depend on the installed
/// `aws` CLI's version or locale.
///
/// Pure and env-free by construction (both env values and the `aws`
/// lookup are parameters, not process state), so it is exercised directly
/// in `src/url_tests.rs` without mutating `std::env` -- forbidden here,
/// since `unsafe` is denied crate-wide.
fn resolve_codecommit_region(
    aws_region: Option<&str>,
    aws_default_region: Option<&str>,
    aws_lookup: impl FnOnce() -> Option<String>,
) -> Result<String, UrlError> {
    if let Some(r) = aws_region
        && !r.is_empty()
    {
        return Ok(r.to_owned());
    }
    if let Some(r) = aws_default_region
        && !r.is_empty()
    {
        return Ok(r.to_owned());
    }
    aws_lookup().ok_or(UrlError::MissingCodecommitRegion)
}

/// Production `aws_lookup` for [`resolve_codecommit_region`]: runs `aws
/// configure get region` and returns its trimmed stdout, or `None` if
/// `aws` is not on `PATH`, fails to spawn, exits non-zero, or prints
/// nothing (ghq's own condition for falling through to its final error,
/// url.go:82-96).
fn run_aws_configure_get_region() -> Option<String> {
    let output =
        std::process::Command::new("aws").args(["configure", "get", "region"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let region = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!region.is_empty()).then_some(region)
}

#[cfg(test)]
#[path = "url_tests.rs"]
mod tests;
