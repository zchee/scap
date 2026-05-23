use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, bail};
use fs2::FileExt;

use crate::cli::GetArgs;
use crate::config;
use crate::path as scap_path;
use crate::url;
use crate::vcs::git;

const PARALLEL_WORKERS: usize = 6;
const EX_TEMPFAIL: i32 = 75;

pub fn run(args: &GetArgs) -> anyhow::Result<()> {
    validate_vcs(args.vcs.as_deref())?;

    let mut effective = args.clone_lite();
    if effective.parallel {
        effective.silent = true;
    }

    let targets = collect_targets(args)?;
    if targets.is_empty() {
        bail!("no target args specified. see `scap get -h` for more details");
    }

    if effective.parallel {
        run_parallel(&effective, &targets)
    } else {
        for target in &targets {
            process_target(&effective, target)
                .with_context(|| format!("failed to get {target:?}"))?;
        }
        if effective.look
            && let Some(first) = targets.first()
        {
            exec_look(&effective, first)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Effective {
    update: bool,
    ssh: bool,
    shallow: bool,
    look: bool,
    silent: bool,
    no_recursive: bool,
    branch: Option<String>,
    bare: bool,
    partial: Option<String>,
    parallel: bool,
}

impl GetArgs {
    fn clone_lite(&self) -> Effective {
        Effective {
            update: self.update,
            ssh: self.ssh,
            shallow: self.shallow,
            look: self.look,
            silent: self.silent,
            no_recursive: self.no_recursive,
            branch: self.branch.clone(),
            bare: self.bare,
            partial: self.partial.clone(),
            parallel: self.parallel,
        }
    }
}

fn validate_vcs(vcs: Option<&str>) -> anyhow::Result<()> {
    match vcs {
        None | Some("git") | Some("github") | Some("codecommit") => Ok(()),
        Some(other) => bail!(
            "unsupported VCS: {other:?} (v1 supports git only; see issue tracker for non-git)"
        ),
    }
}

fn collect_targets(args: &GetArgs) -> anyhow::Result<Vec<String>> {
    if !args.targets.is_empty() {
        return Ok(args.targets.clone());
    }
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    Ok(out)
}

#[tracing::instrument(name = "scap::cmd::get", skip(args), fields(target = %target))]
fn process_target(args: &Effective, target: &str) -> anyhow::Result<()> {
    tracing::debug!(target, "scap::cmd::get processing target");
    let ghq_user = config::ghq_user()?;
    let complete_user = config::ghq_complete_user()?;
    let repo = url::from_input(target, ghq_user.as_deref(), complete_user)?;

    let remote = if repo.host.is_empty() {
        repo.original_input.clone()
    } else if args.ssh {
        repo.ssh_url.clone()
    } else {
        repo.https_url.clone()
    };

    let root = config::root_for_url(&remote)?;
    let dest = scap_path::dest_path(&root, &repo, args.bare);

    if git::dest_is_git_repo(&dest) {
        if args.update {
            let opts = git::UpdateOptions {
                bare: args.bare,
                recursive: !args.no_recursive,
                silent: args.silent,
            };
            git::update(&remote, &dest, &opts)?;
        } else {
            tracing::info!(dest = ?dest, "destination exists, skipping (use -u to update)");
        }
        return Ok(());
    }

    clone_with_lock(&remote, &dest, args)
}

#[tracing::instrument(skip(args), fields(url = %url, dest = ?dest))]
fn clone_with_lock(url: &str, dest: &Path, args: &Effective) -> anyhow::Result<()> {
    for stale in git::stale_tmp_paths(dest) {
        let _ = std::fs::remove_dir_all(&stale);
    }

    let lock_path = lock_path_for(dest);
    if let Some(parent) = lock_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open lock {}", lock_path.display()))?;
    if FileExt::try_lock_exclusive(&lock_file).is_err() {
        eprintln!("another scap process is cloning this repo; exiting");
        std::process::exit(EX_TEMPFAIL);
    }

    let pid = std::process::id();
    let tmp = tmp_path_for(dest, pid);
    let result = (|| -> anyhow::Result<()> {
        let opts = git::CloneOptions {
            shallow: args.shallow,
            branch: args.branch.clone(),
            recursive: !args.no_recursive,
            bare: args.bare,
            partial: args.partial.clone(),
            silent: args.silent,
        };
        git::clone(url, &tmp, &opts)?;
        if let Some(parent) = dest.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&tmp, dest)
            .with_context(|| format!("rename {} -> {}", tmp.display(), dest.display()))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    let _ = FileExt::unlock(&lock_file);
    let _ = std::fs::remove_file(&lock_path);
    result
}

fn lock_path_for(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("scap");
    parent.join(format!(".scap-lock-{name}"))
}

fn tmp_path_for(dest: &Path, pid: u32) -> PathBuf {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("scap");
    parent.join(format!("{name}.tmp-{pid}"))
}

fn exec_look(args: &Effective, target: &str) -> anyhow::Result<()> {
    let ghq_user = config::ghq_user()?;
    let complete_user = config::ghq_complete_user()?;
    let repo = url::from_input(target, ghq_user.as_deref(), complete_user)?;
    let remote = if repo.host.is_empty() {
        repo.original_input.clone()
    } else if args.ssh {
        repo.ssh_url.clone()
    } else {
        repo.https_url.clone()
    };
    let root = config::root_for_url(&remote)?;
    let dest = scap_path::dest_path(&root, &repo, args.bare);

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let scap_look = format!("{}/{}/{}", repo.host, repo.owner, repo.name);
    let status = std::process::Command::new(shell)
        .env("SCAP_LOOK", &scap_look)
        .current_dir(&dest)
        .status()
        .with_context(|| format!("exec $SHELL in {}", dest.display()))?;
    std::process::exit(status.code().unwrap_or(1));
}

fn run_parallel(args: &Effective, targets: &[String]) -> anyhow::Result<()> {
    let queue: Mutex<Vec<String>> = Mutex::new(targets.iter().rev().cloned().collect());
    std::thread::scope(|scope| {
        let workers = PARALLEL_WORKERS.min(targets.len()).max(1);
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let queue_ref = &queue;
            let args_ref = args;
            handles.push(scope.spawn(move || {
                loop {
                    let next = {
                        let mut q = queue_ref.lock().unwrap();
                        q.pop()
                    };
                    let Some(target) = next else { break };
                    if let Err(e) = process_target(args_ref, &target) {
                        tracing::error!(target = %target, error = %e, "failed to get");
                    }
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
    });
    if args.look
        && let Some(first) = targets.first()
    {
        exec_look(args, first)?;
    }
    Ok(())
}
