use std::path::{Path, PathBuf};

use crate::url::Repo;

// ghq local_repository.go:75-86
pub fn dest_path(root: &Path, repo: &Repo, bare: bool) -> PathBuf {
    root.join(rel_path(repo, bare))
}

pub fn rel_path(repo: &Repo, bare: bool) -> PathBuf {
    debug_assert_eq!(
        repo.host,
        repo.host.to_ascii_lowercase(),
        "Repo.host must already be lowercased upstream",
    );

    let mut path = PathBuf::new();
    path.push(&repo.host);
    for segment in repo.owner.split('/').filter(|s| !s.is_empty()) {
        path.push(segment);
    }
    if bare {
        path.push(format!("{}.git", repo.name));
    } else {
        path.push(&repo.name);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(host: &str, owner: &str, name: &str) -> Repo {
        Repo {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
            vcs_hint: None,
            https_url: format!("https://{host}/{owner}/{name}"),
            ssh_url: format!("git@{host}:{owner}/{name}"),
            original_input: format!("{host}/{owner}/{name}"),
        }
    }

    struct Case<'a> {
        repo: Repo,
        bare: bool,
        want: &'a str,
    }

    #[test]
    fn computes_destination_paths() {
        let root = Path::new("/tmp/ghqroot");
        let cases: &[(&str, Case)] = &[
            (
                "github_plain",
                Case {
                    repo: mk("github.com", "motemen", "ghq"),
                    bare: false,
                    want: "/tmp/ghqroot/github.com/motemen/ghq",
                },
            ),
            (
                "github_https_stripped_git_in_name",
                Case {
                    repo: mk("github.com", "motemen", "ghq"),
                    bare: false,
                    want: "/tmp/ghqroot/github.com/motemen/ghq",
                },
            ),
            (
                "github_bare_appends_git",
                Case {
                    repo: mk("github.com", "motemen", "ghq"),
                    bare: true,
                    want: "/tmp/ghqroot/github.com/motemen/ghq.git",
                },
            ),
            (
                "stash_multi_segment_owner",
                Case {
                    repo: mk("stash.com", "scm/motemen", "ghq"),
                    bare: false,
                    want: "/tmp/ghqroot/stash.com/scm/motemen/ghq",
                },
            ),
            (
                "stash_multi_segment_owner_bare",
                Case {
                    repo: mk("stash.com", "scm/motemen", "ghq"),
                    bare: true,
                    want: "/tmp/ghqroot/stash.com/scm/motemen/ghq.git",
                },
            ),
            (
                "gitlab_subgroup",
                Case {
                    repo: mk("gitlab.com", "group/subgroup", "proj"),
                    bare: false,
                    want: "/tmp/ghqroot/gitlab.com/group/subgroup/proj",
                },
            ),
            (
                "gitlab_deep_subgroup",
                Case {
                    repo: mk("gitlab.com", "a/b/c/d", "proj"),
                    bare: false,
                    want: "/tmp/ghqroot/gitlab.com/a/b/c/d/proj",
                },
            ),
            (
                "sourceforge_svn_trunk",
                Case {
                    repo: mk("svn.code.sf.net", "p/ghq/code", "trunk"),
                    bare: false,
                    want: "/tmp/ghqroot/svn.code.sf.net/p/ghq/code/trunk",
                },
            ),
            (
                "sourceforge_jp_gitroot",
                Case {
                    repo: mk("scm.sourceforge.jp", "gitroot/ghq", "ghq"),
                    bare: false,
                    want: "/tmp/ghqroot/scm.sourceforge.jp/gitroot/ghq/ghq",
                },
            ),
            (
                "assembla_git",
                Case {
                    repo: mk("git.assembla.com", "", "ghq"),
                    bare: false,
                    want: "/tmp/ghqroot/git.assembla.com/ghq",
                },
            ),
            (
                "lowercase_host_preserved_by_repo",
                Case {
                    repo: mk("github.com", "Foo", "Bar"),
                    bare: false,
                    want: "/tmp/ghqroot/github.com/Foo/Bar",
                },
            ),
            (
                "name_with_dot_kept",
                Case {
                    repo: mk("github.com", "user", "site.com"),
                    bare: false,
                    want: "/tmp/ghqroot/github.com/user/site.com",
                },
            ),
            (
                "codecommit_shape",
                Case {
                    repo: mk(
                        "git-codecommit.us-east-1.amazonaws.com",
                        "v1/repos",
                        "myrepo",
                    ),
                    bare: false,
                    want: "/tmp/ghqroot/git-codecommit.us-east-1.amazonaws.com/v1/repos/myrepo",
                },
            ),
        ];

        for (name, c) in cases {
            let got = dest_path(root, &c.repo, c.bare);
            assert_eq!(got, Path::new(c.want), "{name}: dest_path mismatch");
        }
    }

    #[test]
    fn root_with_trailing_slash_is_normalized() {
        let root = Path::new("/tmp/ghqroot/");
        let repo = mk("github.com", "motemen", "ghq");
        let got = dest_path(root, &repo, false);
        assert_eq!(got, Path::new("/tmp/ghqroot/github.com/motemen/ghq"));
    }

    #[test]
    fn rel_path_has_no_root_prefix() {
        let repo = mk("github.com", "motemen", "ghq");
        assert_eq!(rel_path(&repo, false), Path::new("github.com/motemen/ghq"));
        assert_eq!(
            rel_path(&repo, true),
            Path::new("github.com/motemen/ghq.git"),
        );
    }

    #[test]
    fn rel_path_preserves_multi_segment_owner() {
        let repo = mk("stash.com", "scm/motemen", "ghq");
        assert_eq!(
            rel_path(&repo, false),
            Path::new("stash.com/scm/motemen/ghq")
        );
    }

    #[test]
    fn rel_path_skips_empty_owner_segments() {
        let repo = mk("git.assembla.com", "", "ghq");
        assert_eq!(rel_path(&repo, false), Path::new("git.assembla.com/ghq"));
    }

    #[test]
    fn name_with_existing_git_suffix_in_struct_is_preserved() {
        // Step 1a guarantees Repo.name has no trailing ".git", but if a caller
        // constructs a Repo by hand with name ending in ".git" (e.g. when
        // intentionally targeting a bare clone), bare=true must not double-append.
        let repo = mk("github.com", "motemen", "ghq.git");
        let bare_path = dest_path(Path::new("/tmp"), &repo, true);
        assert_eq!(bare_path, Path::new("/tmp/github.com/motemen/ghq.git.git"));
        let non_bare = dest_path(Path::new("/tmp"), &repo, false);
        assert_eq!(non_bare, Path::new("/tmp/github.com/motemen/ghq.git"));
    }
}
