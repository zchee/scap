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

    let parsed =
        gix_url::parse(normalized.value.as_bytes().as_bstr()).map_err(|e| UrlError::Malformed {
            input: original_input.clone(),
            reason: e.to_string(),
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

    Ok(Repo {
        host,
        owner,
        name,
        vcs_hint,
        https_url,
        ssh_url,
        original_input,
    })
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
        return Ok(Normalized {
            value: input.to_owned(),
            kind: NormalizedKind::HasScheme,
        });
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
    Ok(Normalized {
        value: input.to_owned(),
        kind: NormalizedKind::Bare,
    })
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
                let user = scap_user.ok_or_else(|| UrlError::UnknownUser {
                    input: original.to_owned(),
                })?;
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
        if let Some(end) = after.find("://") {
            (Some(after[..end].to_owned()), &after[end + 3..])
        } else {
            return None;
        }
    } else if let Some(after) = rest.strip_prefix("//") {
        (None, after)
    } else {
        return None;
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
mod tests {
    use super::*;

    struct Case {
        input: &'static str,
        scap_user: Option<&'static str>,
        complete_user: bool,
        want_host: &'static str,
        want_owner: &'static str,
        want_name: &'static str,
    }

    fn run_cases(cases: &[(&'static str, Case)]) {
        for (name, c) in cases {
            let got = from_input(c.input, c.scap_user, c.complete_user)
                .unwrap_or_else(|e| panic!("{name}: unexpected error: {e}"));
            assert_eq!(got.host, c.want_host, "{name}: host");
            assert_eq!(got.owner, c.want_owner, "{name}: owner");
            assert_eq!(got.name, c.want_name, "{name}: name");
        }
    }

    #[test]
    fn parses_github_url_shapes() {
        let cases: &[(&str, Case)] = &[
            (
                "github_https",
                Case {
                    input: "https://github.com/motemen/ghq",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_https_dot_git",
                Case {
                    input: "https://github.com/motemen/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_https_trailing_slash",
                Case {
                    input: "https://github.com/motemen/ghq/",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_http_plain",
                Case {
                    input: "http://github.com/motemen/ghq",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_ssh_explicit",
                Case {
                    input: "ssh://git@github.com/motemen/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_ssh_no_user",
                Case {
                    input: "ssh://github.com/motemen/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_git_protocol",
                Case {
                    input: "git://github.com/motemen/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_scp",
                Case {
                    input: "git@github.com:motemen/pusheen-explorer.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "pusheen-explorer",
                },
            ),
            (
                "github_scp_root_slash",
                Case {
                    input: "git@github.com:/motemen/pusheen-explorer.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "pusheen-explorer",
                },
            ),
            (
                "github_scp_no_user",
                Case {
                    input: "github.com:motemen/pusheen-explorer.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "pusheen-explorer",
                },
            ),
            (
                "github_https_with_user_in_url",
                Case {
                    input: "https://octocat@github.com/octo/repo",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "octo",
                    want_name: "repo",
                },
            ),
            (
                "github_ssh_explicit_no_dot_git",
                Case {
                    input: "ssh://git@github.com/motemen/ghq",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_scp_dash_name",
                Case {
                    input: "git@github.com:zchee/scap-cli.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "zchee",
                    want_name: "scap-cli",
                },
            ),
            (
                "github_https_underscore_name",
                Case {
                    input: "https://github.com/zchee/foo_bar.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "zchee",
                    want_name: "foo_bar",
                },
            ),
            (
                "github_ssh_plus_git",
                Case {
                    input: "ssh+git://git@github.com/motemen/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "github_git_plus_ssh",
                Case {
                    input: "git+ssh://git@github.com/motemen/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
        ];
        run_cases(cases);
    }

    #[test]
    fn parses_bare_inputs_and_ghq_user_fillin() {
        let cases: &[(&str, Case)] = &[
            (
                "two_segment_default_github",
                Case {
                    input: "motemen/ghq",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "one_segment_complete_false_repeats_name",
                Case {
                    input: "peco",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "peco",
                    want_name: "peco",
                },
            ),
            (
                "one_segment_complete_true_uses_ghq_user",
                Case {
                    input: "same-name-ghq",
                    scap_user: Some("ghq-test"),
                    complete_user: true,
                    want_host: "github.com",
                    want_owner: "ghq-test",
                    want_name: "same-name-ghq",
                },
            ),
            (
                "host_bare_three_segments",
                Case {
                    input: "github.com/motemen/gore",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "motemen",
                    want_name: "gore",
                },
            ),
            (
                "host_bare_golang_x",
                Case {
                    input: "golang.org/x/crypto",
                    scap_user: None,
                    complete_user: false,
                    want_host: "golang.org",
                    want_owner: "x",
                    want_name: "crypto",
                },
            ),
        ];
        run_cases(cases);
    }

    #[test]
    fn parses_self_hosted_and_other_hosts() {
        let cases: &[(&str, Case)] = &[
            (
                "ghe_https",
                Case {
                    input: "https://ghe.example.com/team/proj",
                    scap_user: None,
                    complete_user: false,
                    want_host: "ghe.example.com",
                    want_owner: "team",
                    want_name: "proj",
                },
            ),
            (
                "ghe_https_with_user",
                Case {
                    input: "https://motemen@ghe.example.com/motemen/pusheen-explorer",
                    scap_user: None,
                    complete_user: false,
                    want_host: "ghe.example.com",
                    want_owner: "motemen",
                    want_name: "pusheen-explorer",
                },
            ),
            (
                "bitbucket_https",
                Case {
                    input: "https://bitbucket.org/zchee/scap",
                    scap_user: None,
                    complete_user: false,
                    want_host: "bitbucket.org",
                    want_owner: "zchee",
                    want_name: "scap",
                },
            ),
            (
                "bitbucket_with_port",
                Case {
                    input: "https://bitbucket.local:8888/motemen/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "bitbucket.local",
                    want_owner: "motemen",
                    want_name: "ghq",
                },
            ),
            (
                "gitlab_https",
                Case {
                    input: "https://gitlab.com/group/subgroup/proj",
                    scap_user: None,
                    complete_user: false,
                    want_host: "gitlab.com",
                    want_owner: "group/subgroup",
                    want_name: "proj",
                },
            ),
            (
                "gitlab_ssh",
                Case {
                    input: "git@gitlab.com:group/subgroup/proj.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "gitlab.com",
                    want_owner: "group/subgroup",
                    want_name: "proj",
                },
            ),
            (
                "stash_ssh",
                Case {
                    input: "ssh://git@stash.com/scm/motemen/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "stash.com",
                    want_owner: "scm/motemen",
                    want_name: "ghq",
                },
            ),
            (
                "assembla_git",
                Case {
                    input: "https://git.assembla.com/zchee/ghq.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "git.assembla.com",
                    want_owner: "zchee",
                    want_name: "ghq",
                },
            ),
        ];
        run_cases(cases);
    }

    #[test]
    fn lowercases_host_component() {
        let cases: &[(&str, Case)] = &[
            (
                "uppercase_host_https",
                Case {
                    input: "https://GitHub.com/Foo/bar",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "Foo",
                    want_name: "bar",
                },
            ),
            (
                "mixed_case_ssh",
                Case {
                    input: "ssh://git@GHE.Example.COM/team/proj",
                    scap_user: None,
                    complete_user: false,
                    want_host: "ghe.example.com",
                    want_owner: "team",
                    want_name: "proj",
                },
            ),
            (
                "uppercase_scp",
                Case {
                    input: "git@GitHub.com:Foo/Bar.git",
                    scap_user: None,
                    complete_user: false,
                    want_host: "github.com",
                    want_owner: "Foo",
                    want_name: "Bar",
                },
            ),
        ];
        run_cases(cases);
    }

    #[test]
    fn parses_subversion_and_sourceforge_shapes() {
        let cases: &[(&str, Case)] = &[
            (
                "svn_sourceforge",
                Case {
                    input: "http://svn.code.sf.net/p/ghq/code/trunk",
                    scap_user: None,
                    complete_user: false,
                    want_host: "svn.code.sf.net",
                    want_owner: "p/ghq/code",
                    want_name: "trunk",
                },
            ),
            (
                "git_sourceforge",
                Case {
                    input: "http://git.code.sf.net/p/ghq/code",
                    scap_user: None,
                    complete_user: false,
                    want_host: "git.code.sf.net",
                    want_owner: "p/ghq",
                    want_name: "code",
                },
            ),
            (
                "svn_assembla",
                Case {
                    input: "https://subversion.assembla.com/svn/ghq/",
                    scap_user: None,
                    complete_user: false,
                    want_host: "subversion.assembla.com",
                    want_owner: "svn",
                    want_name: "ghq",
                },
            ),
        ];
        run_cases(cases);
    }

    #[test]
    fn detects_vcs_hint() {
        let svn = from_input("http://svn.code.sf.net/p/ghq/code/trunk", None, false).unwrap();
        assert_eq!(svn.vcs_hint.as_deref(), Some("svn"));
        let svn2 = from_input("https://subversion.assembla.com/svn/ghq/", None, false).unwrap();
        assert_eq!(svn2.vcs_hint.as_deref(), Some("svn"));
        let git = from_input("https://github.com/motemen/ghq", None, false).unwrap();
        assert_eq!(git.vcs_hint, None);
        let svn_scheme = from_input("svn://example.com/repo/trunk", None, false).unwrap();
        assert_eq!(svn_scheme.vcs_hint.as_deref(), Some("svn"));
    }

    #[test]
    fn emits_canonical_https_and_ssh_urls() {
        let r = from_input("git@github.com:motemen/pusheen-explorer.git", None, false).unwrap();
        assert_eq!(r.https_url, "https://github.com/motemen/pusheen-explorer");
        assert_eq!(r.ssh_url, "ssh://git@github.com/motemen/pusheen-explorer");

        let r2 = from_input(
            "https://motemen@ghe.example.com/motemen/pusheen-explorer",
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            r2.https_url,
            "https://ghe.example.com/motemen/pusheen-explorer"
        );
        assert_eq!(
            r2.ssh_url,
            "ssh://motemen@ghe.example.com/motemen/pusheen-explorer"
        );
    }

    #[test]
    fn parses_codecommit_urls() {
        let r = from_input(
            "codecommit::us-east-1://example-profile@my-repo",
            None,
            false,
        )
        .unwrap();
        assert_eq!(r.host, "us-east-1");
        assert_eq!(r.owner, "example-profile");
        assert_eq!(r.name, "my-repo");
        assert_eq!(r.vcs_hint.as_deref(), Some("git"));

        let r2 = from_input("codecommit://my-repo", None, false).unwrap();
        assert_eq!(r2.host, "codecommit");
        assert_eq!(r2.name, "my-repo");
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(matches!(
            from_input("../foo/bar", None, false),
            Err(UrlError::PathTraversal(_))
        ));
        assert!(matches!(
            from_input("foo/../bar", None, false),
            Err(UrlError::PathTraversal(_))
        ));
        assert!(matches!(
            from_input("https://github.com/../etc/passwd", None, false),
            Err(UrlError::PathTraversal(_))
        ));
    }

    #[test]
    fn rejects_empty_input() {
        assert!(matches!(from_input("", None, false), Err(UrlError::Empty)));
    }

    #[test]
    fn returns_unknown_user_when_complete_user_lacks_scap_user() {
        let err = from_input("peco", None, true).unwrap_err();
        match err {
            UrlError::UnknownUser { input } => assert_eq!(input, "peco"),
            other => panic!("expected UnknownUser, got {other:?}"),
        }
    }

    #[test]
    fn preserves_original_input() {
        let r = from_input("git@github.com:motemen/pusheen-explorer.git", None, false).unwrap();
        assert_eq!(
            r.original_input,
            "git@github.com:motemen/pusheen-explorer.git"
        );
    }
}
