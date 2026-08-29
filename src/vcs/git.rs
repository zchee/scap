use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to invoke `git`: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("`git {args}` in {dir} failed (status {status})")]
    NonZeroExit { args: String, dir: String, status: i32 },
}

#[derive(Debug, Clone)]
pub struct CloneOptions {
    pub shallow: bool,
    pub branch: Option<String>,
    pub recursive: bool,
    pub bare: bool,
    pub partial: Option<String>,
    pub silent: bool,
}

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub bare: bool,
    pub recursive: bool,
    pub silent: bool,
}

// ghq vcs.go GitBackend.Clone (lines 52-82).
#[tracing::instrument(name = "scap::vcs::git::clone", skip(opts), fields(url = %url, dest = ?dest))]
pub fn clone(url: &str, dest: &Path, opts: &CloneOptions) -> Result<(), GitError> {
    tracing::debug!(url, dest = ?dest, "scap::vcs::git::clone start");
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut args: Vec<String> = vec!["clone".into()];
    if opts.shallow {
        args.push("--depth".into());
        args.push("1".into());
    }
    if let Some(branch) = &opts.branch {
        args.push("--branch".into());
        args.push(branch.clone());
        args.push("--single-branch".into());
    }
    if opts.recursive {
        args.push("--recursive".into());
    }
    if opts.bare {
        args.push("--bare".into());
    }
    match opts.partial.as_deref() {
        Some("blobless") => args.push("--filter=blob:none".into()),
        Some("treeless") => args.push("--filter=tree:0".into()),
        _ => {}
    }
    args.push(url.to_string());
    args.push(dest.display().to_string());
    run(None, opts.silent, &args)
}

// ghq vcs.go GitBackend.Update (lines 82-108).
#[tracing::instrument(skip(opts), fields(url = %url, dest = ?dest, bare = opts.bare))]
pub fn update(url: &str, dest: &Path, opts: &UpdateOptions) -> Result<(), GitError> {
    if opts.bare {
        return run(Some(dest), true, &["fetch", url, "*:*"]);
    }
    let has_upstream = run(Some(dest), true, &["rev-parse", "@{upstream}"]).is_ok();
    if !has_upstream {
        return run(Some(dest), opts.silent, &["fetch"]);
    }
    run(Some(dest), opts.silent, &["pull", "--ff-only"])?;
    if opts.recursive {
        run(Some(dest), opts.silent, &["submodule", "update", "--init", "--recursive"])?;
    }
    Ok(())
}

pub fn dest_is_git_repo(dest: &Path) -> bool {
    if !dest.exists() {
        return false;
    }
    if dest.join(".git").is_dir() {
        return true;
    }
    if dest.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".git"))
        && dest.join("HEAD").is_file()
        && dest.join("objects").is_dir()
        && dest.join("refs").is_dir()
    {
        return true;
    }
    false
}

fn run<S: AsRef<str>>(dir: Option<&Path>, silent: bool, args: &[S]) -> Result<(), GitError> {
    let argv: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();
    let mut cmd = Command::new("git");
    cmd.args(&argv);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    if silent {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    }
    let status = cmd.status()?;
    if !status.success() {
        return Err(GitError::NonZeroExit {
            args: argv.join(" "),
            dir: dir.map(|d| d.display().to_string()).unwrap_or_default(),
            status: status.code().unwrap_or(-1),
        });
    }
    Ok(())
}

pub fn stale_tmp_paths(dest: &Path) -> Vec<PathBuf> {
    let Some(parent) = dest.parent() else {
        return Vec::new();
    };
    let Some(name) = dest.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{name}.tmp-");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(pid_str) = file_name.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<i32>() else {
            continue;
        };
        if !pid_is_alive(pid) {
            out.push(entry.path());
        }
    }
    out
}

/// Whether a process with `pid` exists, as `kill(pid, 0)` reports it.
///
/// Decision D-1: this is `rustix::process::test_kill_process`, a safe
/// wrapper over that syscall, and not a `kill -0` subprocess -- one spawn
/// per stale-tmp candidate was the last non-VCS process `get` created.
///
/// Three cases, and only the first two are "the directory is in use":
///
/// * `Ok(())` -- the process exists and is signalable: alive.
/// * `EPERM` -- the process exists but belongs to another user, so scap may
///   not signal it. It is alive, and its temporary directory is emphatically
///   not this process's to delete.
/// * anything else (`ESRCH` in practice) -- no such process: not alive.
///
/// A suffix that is not a pid this program could have written -- `0`, or a
/// negative number -- is reported as not alive, so the directory becomes a
/// cleanup candidate. `std::process::id()` is always positive, so such a
/// name was not produced by scap. This is a deliberate narrowing of the old
/// probe, under which `kill -0 0` addressed the caller's own process group
/// and therefore reported *alive* for `<name>.tmp-0`.
#[cfg(unix)]
fn pid_is_alive(pid: i32) -> bool {
    // `Pid::from_raw` debug-asserts a non-negative argument, so screen the
    // sign here rather than letting a hostile directory name panic a debug
    // build.
    if pid <= 0 {
        return false;
    }
    let Some(pid) = rustix::process::Pid::from_raw(pid) else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(rustix::io::Errno::PERM) => true,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: i32) -> bool {
    true
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
