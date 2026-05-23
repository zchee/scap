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
pub struct GetArgs {}

#[derive(Debug, clap::Args)]
pub struct ListArgs {}

#[derive(Debug, clap::Args)]
pub struct RmArgs {}

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
