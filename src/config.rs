use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to invoke `git`: {0}")]
    GitSpawn(#[from] std::io::Error),
    #[error("git config {key:?} failed (status {status}): {stderr}")]
    GitConfigFailed { key: String, status: i32, stderr: String },
    #[error("could not determine home directory")]
    NoHomeDir,
    #[error("invalid output from `git config --get-regexp`: {0:?}")]
    MalformedRegexpResult(String),
    #[error("invalid utf-8 in git config output")]
    InvalidUtf8,
}

// ghq local_repository.go:355-395
pub fn resolve_roots(all: bool) -> Result<Vec<PathBuf>, ConfigError> {
    let env_root = std::env::var_os("SCAP_ROOT");
    let env_root_nonempty = env_root.as_ref().is_some_and(|v| !v.is_empty());

    let mut roots: Vec<PathBuf> = if env_root_nonempty {
        split_path_list(env_root.as_ref().unwrap())
    } else {
        let mut from_git = git_config_get_all_path("scap.root")?
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        from_git.reverse();
        from_git
    };

    if roots.is_empty() {
        let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
        roots.push(home.join("scap"));
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
    let env_root = std::env::var_os("SCAP_ROOT");
    if env_root.as_ref().is_some_and(|v| !v.is_empty()) {
        let mut list = split_path_list(env_root.as_ref().unwrap());
        if !list.is_empty() {
            return Ok(clean_path(&list.remove(0)));
        }
    }

    if !is_codecommit_like(url) {
        let out = run_git_config(&["--path", "--get-urlmatch", "scap.root", url])?;
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

pub fn scap_user() -> Result<Option<String>, ConfigError> {
    run_git_config(&["--get", "scap.user"]).map(|opt| opt.map(|v| v.trim().to_owned()))
}

pub fn scap_complete_user() -> Result<bool, ConfigError> {
    match run_git_config(&["--bool", "--get", "scap.completeUser"])? {
        Some(v) => Ok(v.trim() == "true"),
        None => Ok(false),
    }
}

fn url_match_local_repository_roots() -> Result<Vec<PathBuf>, ConfigError> {
    let out = match run_git_config_multi(&["--path", "--get-regexp", r"^scap\..+\.root$"])? {
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
            if trimmed.is_empty() { Ok(None) } else { Ok(Some(trimmed.to_owned())) }
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
    if out.as_os_str().is_empty() { PathBuf::from(".") } else { out }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
