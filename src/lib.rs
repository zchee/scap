#![deny(unsafe_code)]

pub mod cli;
pub mod cmd;
pub mod config;
pub mod path;
pub mod url;
pub mod vcs;

pub use url::{Repo, UrlError, from_input as parse_repo_input};

use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

pub fn run() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_env("SCAP_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("warn"));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_span_events(fmt::format::FmtSpan::CLOSE)
        .init();

    let cli = cli::Cli::parse();
    cli::dispatch(cli)
}
