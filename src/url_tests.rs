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

    let r2 = from_input("https://motemen@ghe.example.com/motemen/pusheen-explorer", None, false)
        .unwrap();
    assert_eq!(r2.https_url, "https://ghe.example.com/motemen/pusheen-explorer");
    assert_eq!(r2.ssh_url, "ssh://motemen@ghe.example.com/motemen/pusheen-explorer");
}

#[test]
fn parses_codecommit_urls() {
    let r = from_input("codecommit::us-east-1://example-profile@my-repo", None, false).unwrap();
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
    assert!(matches!(from_input("../foo/bar", None, false), Err(UrlError::PathTraversal(_))));
    assert!(matches!(from_input("foo/../bar", None, false), Err(UrlError::PathTraversal(_))));
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
    assert_eq!(r.original_input, "git@github.com:motemen/pusheen-explorer.git");
}

// --- W1.3: `is_codecommit_input` (ghq url.go:25) ---------------------------

/// ghq's pattern, for the differential test below. `[^]]` is spelled `[^\]]`
/// because the `regex` crate does not honour the POSIX "`]` first in a class
/// is literal" quirk, and `\w` is scoped to `(?-u:)` because Go's `\w` is
/// ASCII while the crate's is Unicode by default.
const GHQ_CODECOMMIT_PATTERN: &str =
    r"^(codecommit):(?::([a-z][a-z0-9-]+):)?//(?:([^\]]+)@)?(?-u:[\w.-])+$";

#[test]
fn is_codecommit_input_matches_ghq_acceptance_set() {
    let cases: &[(&str, bool)] = &[
        // §5 table.
        ("codecommit://a/b", false),
        ("codecommit::u://x", false),
        ("codecommit::US-east-1://x", false),
        ("codecommit://", false),
        ("codecommit://user@repo", true),
        ("codecommit://a/b@c", true),
        ("codecommit::us-east-1://repo_1.x-y", true),
        // Divergence-fix rows: ghq's user class is `[^]]+`, not `[^@]+`.
        ("codecommit://a@b@c", true),
        ("codecommit://a]b@c", false),
        // Go's `\w` is ASCII, so a non-ASCII repository name cannot match.
        ("codecommit://répo", false),
        // Empty user / empty host / `/` in the host.
        ("codecommit://a@", false),
        ("codecommit://@host", false),
        ("codecommit::us-east-1://a@b/c", false),
        // Inputs the dispatch in `root_for_url` must keep sending to urlmatch.
        ("https://github.com/foo/bar", false),
        ("git@github.com:foo/bar", false),
        // Regressions the old `[^@]+` pattern already accepted.
        ("codecommit://my-repo", true),
        ("codecommit::us-east-1://my-repo", true),
        ("codecommit://profile@my-repo", true),
    ];

    for &(input, want) in cases {
        assert_eq!(is_codecommit_input(input), want, "is_codecommit_input({input:?})");
    }
}

#[test]
fn is_codecommit_input_agrees_with_ghqs_pattern() {
    let re = regex::Regex::new(GHQ_CODECOMMIT_PATTERN).expect("ghq pattern compiles");

    // Deterministic corpus: every combination of the parts that decide the
    // pattern's four segments, so the two implementations are compared on the
    // boundaries rather than on random noise.
    let prefixes =
        ["codecommit:", "codecommit::us-east-1:", "codecommit::u:", "codecommit::US:", "git:"];
    let separators = ["//", "/", ""];
    let users = ["", "a@", "a@b@", "a]b@", "@", "a/b@"];
    let hosts = ["repo", "repo_1.x-y", "a/b", "répo", "", "x]", "-"];
    let suffixes = ["", "/", "]", "@x"];

    let mut checked = 0usize;
    let mut disagreements = Vec::new();
    for prefix in prefixes {
        for separator in separators {
            for user in users {
                for host in hosts {
                    for suffix in suffixes {
                        let input = format!("{prefix}{separator}{user}{host}{suffix}");
                        checked += 1;
                        let got = is_codecommit_input(&input);
                        let want = re.is_match(&input);
                        if got != want {
                            disagreements.push((input, got, want));
                        }
                    }
                }
            }
        }
    }

    assert!(checked >= 200, "corpus too small: {checked} strings");
    if let Some((input, got, want)) = disagreements.first() {
        panic!(
            "{} of {checked} strings disagree with ghq's pattern; first: \
             {input:?} -> is_codecommit_input {got}, regex {want}",
            disagreements.len()
        );
    }
}
