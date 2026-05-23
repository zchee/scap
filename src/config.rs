use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to invoke `git`: {0}")]
    GitSpawn(#[from] std::io::Error),
    #[error("git config {key:?} failed (status {status}): {stderr}")]
    GitConfigFailed {
        key: String,
        status: i32,
        stderr: String,
    },
    #[error("could not determine home directory")]
    NoHomeDir,
    #[error("invalid output from `git config --get-regexp`: {0:?}")]
    MalformedRegexpResult(String),
    #[error("invalid utf-8 in git config output")]
    InvalidUtf8,
}

// ghq local_repository.go:355-395
pub fn resolve_roots(all: bool) -> Result<Vec<PathBuf>, ConfigError> {
    let env_root = std::env::var_os("GHQ_ROOT");
    let env_root_nonempty = env_root.as_ref().is_some_and(|v| !v.is_empty());

    let mut roots: Vec<PathBuf> = if env_root_nonempty {
        split_path_list(env_root.as_ref().unwrap())
    } else {
        let mut from_git = git_config_get_all_path("ghq.root")?
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        from_git.reverse();
        from_git
    };

    if roots.is_empty() {
        let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
        roots.push(home.join("ghq"));
    }

    if all && !env_root_nonempty {
        let url_match_roots = url_match_local_repository_roots()?;
        roots.extend(url_match_roots);
    }

    let mut seen = std::collections::HashSet::new();
    let mut deduped = Vec::with_capacity(roots.len());
    for root in roots {
        let cleaned = clean_path(&root);
        let resolved = std::fs::canonicalize(&cleaned).unwrap_or(cleaned);
        if seen.insert(resolved.clone()) {
            deduped.push(resolved);
        }
    }

    Ok(deduped)
}

// ghq local_repository.go:123-135
pub fn root_for_url(url: &str) -> Result<PathBuf, ConfigError> {
    let env_root = std::env::var_os("GHQ_ROOT");
    if env_root.as_ref().is_some_and(|v| !v.is_empty()) {
        let mut list = split_path_list(env_root.as_ref().unwrap());
        if !list.is_empty() {
            return Ok(clean_path(&list.remove(0)));
        }
    }

    if !is_codecommit_like(url) {
        let out = run_git_config(&["--path", "--get-urlmatch", "ghq.root", url])?;
        if let Some(value) = out {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(clean_path(&PathBuf::from(trimmed)));
            }
        }
    }

    let roots = resolve_roots(false)?;
    roots.into_iter().next().ok_or(ConfigError::NoHomeDir)
}

pub fn git_config_get_path(key: &str) -> Result<Option<String>, ConfigError> {
    run_git_config(&["--path", "--get", key])
}

pub fn git_config_get_all_path(key: &str) -> Result<Vec<String>, ConfigError> {
    let out = run_git_config_multi(&["--path", "--get-all", key])?;
    Ok(out
        .lines()
        .map(|line| line.trim_end_matches('\r').to_owned())
        .filter(|line| !line.is_empty())
        .collect())
}

pub fn ghq_user() -> Result<Option<String>, ConfigError> {
    run_git_config(&["--get", "ghq.user"]).map(|opt| opt.map(|v| v.trim().to_owned()))
}

pub fn ghq_complete_user() -> Result<bool, ConfigError> {
    match run_git_config(&["--bool", "--get", "ghq.completeUser"])? {
        Some(v) => Ok(v.trim() == "true"),
        None => Ok(false),
    }
}

fn url_match_local_repository_roots() -> Result<Vec<PathBuf>, ConfigError> {
    let out = match run_git_config_multi(&["--path", "--get-regexp", r"^ghq\..+\.root$"])? {
        s if s.is_empty() => return Ok(Vec::new()),
        s => s,
    };
    let mut paths = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.is_empty() {
            continue;
        }
        let (_key, value) = trimmed
            .split_once(char::is_whitespace)
            .ok_or_else(|| ConfigError::MalformedRegexpResult(trimmed.to_owned()))?;
        let value = value.trim_start();
        if !value.is_empty() {
            paths.push(PathBuf::from(value));
        }
    }
    Ok(paths)
}

fn is_codecommit_like(url: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^(codecommit):(?::([a-z][a-z0-9-]+):)?//(?:([^@]+)@)?([\w\.-]+)$")
            .expect("codecommit regex compiles")
    });
    re.is_match(url)
}

fn run_git_config(args: &[&str]) -> Result<Option<String>, ConfigError> {
    let key = args.last().copied().unwrap_or("").to_owned();
    let output = Command::new("git").arg("config").args(args).output()?;
    match output.status.code() {
        Some(0) => {
            let stdout = String::from_utf8(output.stdout).map_err(|_| ConfigError::InvalidUtf8)?;
            let trimmed = stdout.trim_end_matches('\n');
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(1) => Ok(None),
        Some(status) => Err(ConfigError::GitConfigFailed {
            key,
            status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        None => Err(ConfigError::GitConfigFailed {
            key,
            status: -1,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
    }
}

fn run_git_config_multi(args: &[&str]) -> Result<String, ConfigError> {
    let key = args.last().copied().unwrap_or("").to_owned();
    let output = Command::new("git").arg("config").args(args).output()?;
    match output.status.code() {
        Some(0) => String::from_utf8(output.stdout).map_err(|_| ConfigError::InvalidUtf8),
        Some(1) => Ok(String::new()),
        Some(status) => Err(ConfigError::GitConfigFailed {
            key,
            status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        None => Err(ConfigError::GitConfigFailed {
            key,
            status: -1,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
    }
}

fn split_path_list(value: &std::ffi::OsStr) -> Vec<PathBuf> {
    std::env::split_paths(value).collect()
}

fn clean_path(p: &std::path::Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use std::path::Path;
    use tempfile::TempDir;

    struct EnvGuard {
        keys: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _tmp: TempDir,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in self.keys.drain(..) {
                match v {
                    Some(val) => set_env(k, &val),
                    None => unset_env(k),
                }
            }
        }
    }

    fn set_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
        // SAFETY: tests using EnvGuard are tagged #[serial], so this access
        // is serialized at the harness level.
        unsafe { std::env::set_var(key, value) };
    }

    fn unset_env(key: &str) {
        // SAFETY: serialized via #[serial].
        unsafe { std::env::remove_var(key) };
    }

    fn setup(contents: &str) -> EnvGuard {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = tmp.path().join("gitconfig");
        std::fs::File::create(&cfg)
            .expect("create gitconfig")
            .write_all(contents.as_bytes())
            .expect("write gitconfig");

        let saved = vec![
            (
                "GIT_CONFIG_NOSYSTEM",
                std::env::var_os("GIT_CONFIG_NOSYSTEM"),
            ),
            ("GIT_CONFIG_GLOBAL", std::env::var_os("GIT_CONFIG_GLOBAL")),
            ("GHQ_ROOT", std::env::var_os("GHQ_ROOT")),
            ("HOME", std::env::var_os("HOME")),
            ("XDG_CONFIG_HOME", std::env::var_os("XDG_CONFIG_HOME")),
        ];

        set_env("GIT_CONFIG_NOSYSTEM", "1");
        set_env("GIT_CONFIG_GLOBAL", &cfg);
        unset_env("GHQ_ROOT");
        set_env("HOME", tmp.path());
        set_env("XDG_CONFIG_HOME", tmp.path().join("xdg"));

        EnvGuard {
            keys: saved,
            _tmp: tmp,
        }
    }

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    #[serial]
    fn resolve_roots_uses_ghq_root_env_when_set() {
        let _g = setup("");
        set_env("GHQ_ROOT", "/p/one:/p/two");
        let got = resolve_roots(false).unwrap();
        assert_eq!(got, vec![pb("/p/one"), pb("/p/two")]);
        let got_all = resolve_roots(true).unwrap();
        assert_eq!(got_all, vec![pb("/p/one"), pb("/p/two")]);
    }

    #[test]
    #[serial]
    fn resolve_roots_reverses_multi_root_from_gitconfig() {
        let _g = setup("[ghq]\n\troot = /a\n\troot = /b\n\troot = /c\n");
        let got = resolve_roots(false).unwrap();
        assert_eq!(got, vec![pb("/c"), pb("/b"), pb("/a")]);
    }

    #[test]
    #[serial]
    fn resolve_roots_falls_back_to_home_ghq() {
        let g = setup("");
        let expected = Path::new(&std::env::var_os("HOME").unwrap()).join("ghq");
        let got = resolve_roots(false).unwrap();
        assert_eq!(got, vec![expected]);
        drop(g);
    }

    #[test]
    #[serial]
    fn resolve_roots_all_appends_urlmatch_roots() {
        let _g =
            setup("[ghq]\n\troot = /default\n[ghq \"https://example.com/\"]\n\troot = /custom\n");
        let no_all = resolve_roots(false).unwrap();
        assert_eq!(no_all, vec![pb("/default")]);
        let all = resolve_roots(true).unwrap();
        assert!(all.contains(&pb("/default")), "missing default in {all:?}");
        assert!(all.contains(&pb("/custom")), "missing custom in {all:?}");
    }

    #[test]
    #[serial]
    fn resolve_roots_dedups() {
        let _g = setup("[ghq]\n\troot = /same\n\troot = /same\n");
        let got = resolve_roots(false).unwrap();
        assert_eq!(got, vec![pb("/same")]);
    }

    #[test]
    #[serial]
    fn root_for_url_uses_ghq_root_env_first() {
        let _g = setup("[ghq]\n\troot = /default\n");
        set_env("GHQ_ROOT", "/env-first:/env-second");
        let got = root_for_url("https://github.com/foo/bar").unwrap();
        assert_eq!(got, pb("/env-first"));
    }

    #[test]
    #[serial]
    fn root_for_url_consults_urlmatch_first() {
        let _g = setup(
            "[ghq]\n\troot = /default\n[ghq \"https://special.example.com/\"]\n\troot = /special\n",
        );
        let special = root_for_url("https://special.example.com/foo/bar").unwrap();
        assert_eq!(special, pb("/special"));
        let other = root_for_url("https://other.example.com/foo/bar").unwrap();
        assert_eq!(other, pb("/default"));
    }

    #[test]
    #[serial]
    fn root_for_url_skips_urlmatch_for_codecommit() {
        let _g =
            setup("[ghq]\n\troot = /default\n[ghq \"codecommit\"]\n\troot = /should-be-ignored\n");
        let got = root_for_url("codecommit::us-east-1://my-repo").unwrap();
        assert_eq!(got, pb("/default"));
    }

    #[test]
    #[serial]
    fn ghq_user_returns_value_when_set() {
        let _g = setup("[ghq]\n\tuser = motemen\n");
        assert_eq!(ghq_user().unwrap(), Some("motemen".to_owned()));
    }

    #[test]
    #[serial]
    fn ghq_user_returns_none_when_unset() {
        let _g = setup("");
        assert_eq!(ghq_user().unwrap(), None);
    }

    #[test]
    #[serial]
    fn ghq_complete_user_parses_bool() {
        let _g = setup("[ghq]\n\tcompleteUser = true\n");
        assert!(ghq_complete_user().unwrap());
    }

    #[test]
    #[serial]
    fn ghq_complete_user_defaults_false() {
        let _g = setup("");
        assert!(!ghq_complete_user().unwrap());
    }

    #[test]
    fn is_codecommit_like_matches_explicit_region() {
        assert!(is_codecommit_like("codecommit::us-east-1://my-repo"));
        assert!(is_codecommit_like("codecommit://my-repo"));
        assert!(is_codecommit_like("codecommit://profile@my-repo"));
        assert!(!is_codecommit_like("https://github.com/foo/bar"));
        assert!(!is_codecommit_like("git@github.com:foo/bar"));
    }

    #[test]
    fn clean_path_normalizes_parent_and_current() {
        assert_eq!(clean_path(Path::new("/a/b/../c")), pb("/a/c"));
        assert_eq!(clean_path(Path::new("/a/./b")), pb("/a/b"));
        assert_eq!(clean_path(Path::new("./relative")), pb("relative"));
    }
}
