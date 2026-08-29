//! W3.5c profile harness: runs scap's own `list` command in-process many
//! times so a sampling profiler has a long-lived process to attach to.
//!
//! Never committed. It calls the same `cli::dispatch` the shipped binary
//! calls, so the reader, the pool and the post-processing are the shipped
//! ones; only the process lifetime differs.

use clap::Parser;

fn main() -> anyhow::Result<()> {
    // `init_tracing` installs a global subscriber and panics on a second
    // call, so it stays outside the loop.
    scap::init_tracing();
    let iters: usize = std::env::var("W35C_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    for _ in 0..iters {
        let cli = scap::cli::Cli::parse();
        scap::cli::dispatch(cli)?;
    }
    Ok(())
}
