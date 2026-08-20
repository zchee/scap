use std::fs;
use std::io::{BufRead, Write};

use anyhow::{Context, bail};

use crate::cli::RmArgs;
use crate::{config, path as scap_path, url as scap_url};

// ghq cmd_rm.go:doRm — no safety gate beyond ghq's: missing/empty errors,
// --dry-run prints, otherwise prompt-then-remove. Critic F6 removed the
// earlier draft's "scap-managed" gate.
pub fn run(args: &RmArgs) -> anyhow::Result<()> {
    if args.target.is_empty() {
        bail!("repository name is required");
    }

    let scap_user = config::scap_user().context("read scap.user")?;
    let complete_user = config::scap_complete_user().context("read scap.completeUser")?;
    let repo = scap_url::from_input(&args.target, scap_user.as_deref(), complete_user)?;

    let root = config::root_for_url(&repo.https_url).context("resolve root")?;
    let dest = scap_path::dest_path(&root, &repo, args.bare);

    if super::is_not_exist_or_empty(&dest)? {
        // ghq cmd_rm.go:38
        bail!("directory \"{}\" does not exist", dest.display());
    }

    if args.dry_run {
        println!("Would remove {}", dest.display());
        return Ok(());
    }

    if !confirm(&format!("Remove {}?", dest.display()))? {
        bail!("aborted");
    }

    fs::remove_dir_all(&dest).with_context(|| format!("remove {}", dest.display()))?;
    println!("Removed {}", dest.display());
    Ok(())
}

// ghq cmd_rm.go:confirm — exact `response == "y"`, no prefix.
fn confirm(message: &str) -> anyhow::Result<bool> {
    let mut stderr = std::io::stderr();
    write!(stderr, "{} [y/N]: ", message)?;
    stderr.flush()?;

    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let trimmed = line.trim_end_matches(['\n', '\r']);
    Ok(trimmed == "y")
}
