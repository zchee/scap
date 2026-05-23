use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail};

use crate::cli::CreateArgs;
use crate::{config, path as scap_path, url as scap_url};

const GIT_VCS_ALIASES: &[&str] = &["git", "github", "codecommit"];

pub fn run(args: &CreateArgs) -> anyhow::Result<()> {
    if args.target.is_empty() {
        bail!("repository name is required");
    }

    validate_vcs(args.vcs.as_deref())?;

    let ghq_user = config::ghq_user().context("read ghq.user")?;
    let complete_user = config::ghq_complete_user().context("read ghq.completeUser")?;
    let repo = scap_url::from_input(&args.target, ghq_user.as_deref(), complete_user)?;

    let root = config::root_for_url(&repo.https_url).context("resolve root")?;
    let dest = scap_path::dest_path(&root, &repo, args.bare);

    if !is_not_exist_or_empty(&dest)? {
        // ghq cmd_create.go:42
        bail!(
            "directory \"{}\" already exists and not empty",
            dest.display()
        );
    }

    fs::create_dir_all(&dest).with_context(|| format!("mkdir -p {}", dest.display()))?;
    git_init(&dest, args.bare)?;

    println!("{}", dest.display());
    Ok(())
}

fn validate_vcs(vcs: Option<&str>) -> anyhow::Result<()> {
    let Some(v) = vcs else { return Ok(()) };
    if GIT_VCS_ALIASES.contains(&v) {
        return Ok(());
    }
    // ADR-2 intentional-divergence: ghq supports svn/hg/darcs/fossil/bzr.
    bail!(
        "unsupported VCS: \"{}\" (v1 supports git only; see issue tracker for non-git)",
        v
    );
}

fn is_not_exist_or_empty(path: &Path) -> anyhow::Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => Err(anyhow::anyhow!(
            "{} exists but is not a directory",
            path.display()
        )),
        Err(e) => Err(e).with_context(|| format!("inspect {}", path.display())),
    }
}

// ghq cmdutil/run.go RunInDir: the child's stdout is routed to our stderr,
// so only scap's own `println!(dest)` lands on stdout.
fn git_init(dest: &Path, bare: bool) -> anyhow::Result<()> {
    use std::os::fd::AsFd;

    let mut cmd = Command::new("git");
    cmd.current_dir(dest).arg("init");
    if bare {
        cmd.arg("--bare");
    }
    let stderr_dup = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context("dup stderr fd")?;
    cmd.stdout(stderr_dup);

    let status = cmd
        .status()
        .with_context(|| format!("spawn git init in {}", dest.display()))?;
    if !status.success() {
        bail!("git init exited with {}", status);
    }
    Ok(())
}
