use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "scap", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    #[command(visible_alias = "clone")]
    Get(GetArgs),
    List(ListArgs),
    Rm(RmArgs),
    Root(RootArgs),
    Create(CreateArgs),
}

#[derive(Debug, clap::Args)]
pub struct GetArgs {
    /// Update local repository if cloned already.
    #[arg(long, short = 'u')]
    pub update: bool,

    /// Clone with SSH.
    #[arg(short = 'p')]
    pub ssh: bool,

    /// Do a shallow clone.
    #[arg(long)]
    pub shallow: bool,

    /// Exec $SHELL in the destination after clone. Exports SCAP_LOOK=<host/owner/name>
    /// (intentional divergence from ghq's GHQ_LOOK — no fallback; users with existing
    /// ghq shell hooks must update them; see ADR for plan §6 Step 5).
    #[arg(long, short = 'l')]
    pub look: bool,

    /// VCS backend. v1 accepts `git`/`github`/`codecommit` only; others rejected
    /// (intentional divergence from ghq, see ADR-2).
    #[arg(long, value_name = "vcs")]
    pub vcs: Option<String>,

    /// Clone or update silently.
    #[arg(long, short = 's')]
    pub silent: bool,

    /// Prevent recursive fetching.
    #[arg(long)]
    pub no_recursive: bool,

    /// Specify branch name (implies --single-branch on Git).
    #[arg(long, short = 'b', value_name = "branch")]
    pub branch: Option<String>,

    /// Import in parallel (fixed pool of 6 workers, forces --silent).
    #[arg(long, short = 'P')]
    pub parallel: bool,

    /// Do a bare clone.
    #[arg(long)]
    pub bare: bool,

    /// Do a partial clone.
    #[arg(long, value_name = "value", value_parser = ["blobless", "treeless"])]
    pub partial: Option<String>,

    /// Repository targets. Empty = read from stdin (one per line).
    pub targets: Vec<String>,
}

pub use crate::cmd::list::ListArgs;

#[derive(Debug, clap::Args)]
pub struct RmArgs {
    /// Do not actually remove; print what would be removed.
    #[arg(long)]
    pub dry_run: bool,

    /// Remove a bare repository (target dest ends in .git).
    #[arg(long)]
    pub bare: bool,

    /// Repository spec: <project>, <user>/<project>, <host>/<user>/<project>, or full URL.
    pub target: String,
}

#[derive(Debug, clap::Args)]
pub struct RootArgs {
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, clap::Args)]
pub struct CreateArgs {
    /// VCS backend. v1 accepts `git`/`github`/`codecommit` only;
    /// other values (svn, hg, darcs, fossil, bzr) are rejected
    /// (intentional divergence from ghq, see ADR-2).
    #[arg(long, value_name = "vcs")]
    pub vcs: Option<String>,

    /// Create a bare repository.
    #[arg(long)]
    pub bare: bool,

    /// Repository spec: <project>, <user>/<project>, <host>/<user>/<project>, or full URL.
    pub target: String,
}

pub fn dispatch(cli: Cli) -> anyhow::Result<()> {
    match cli.cmd {
        Cmd::Get(args) => crate::cmd::get::run(&args),
        Cmd::List(args) => crate::cmd::list::run(&args),
        Cmd::Rm(args) => crate::cmd::rm::run(&args),
        Cmd::Root(args) => crate::cmd::root::run(&args),
        Cmd::Create(args) => crate::cmd::create::run(&args),
    }
}
