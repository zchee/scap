#![forbid(unsafe_code)]

pub mod cli;
pub mod cmd;
pub mod config;
pub mod path;
pub mod url;
pub mod vcs;

use clap::Parser;
use tracing_subscriber::{fmt, EnvFilter};

pub fn run() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_env("SCAP_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("warn"));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let cli = cli::Cli::parse();
    cli::dispatch(cli)
}
