#![deny(unsafe_code)]

pub mod cli;
pub mod cmd;
pub mod config;
pub mod path;
pub mod url;
pub mod vcs;
pub mod walk;

use clap::Parser;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::{EnvFilter, fmt};
pub use url::{Repo, UrlError, from_input as parse_repo_input};

/// Builds the process's `tracing` subscriber without installing it.
///
/// `FmtSpan::CLOSE` and the stderr-shaped writer stay identical on both
/// paths, so ADR-9's span-close counters keep working under
/// `SCAP_LOG=debug`. Only the filter differs:
///
/// - `log_env` is `None`, or holds directives that fail to parse: the
///   subscriber is built with a bare [`LevelFilter::WARN`] and no
///   [`EnvFilter`] is constructed at all. That is the point: on the common
///   no-log-configured path, `EnvFilter`'s directive parsing was measurable
///   startup cost (plan Decision E / ADR-6).
/// - `log_env` holds directives that parse: they become an [`EnvFilter`],
///   matching the previous unconditional behaviour.
pub(crate) fn build_subscriber<W>(
    log_env: Option<&str>,
    writer: W,
) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'w> fmt::MakeWriter<'w> + Send + Sync + 'static,
{
    match log_env.map(EnvFilter::try_new).transpose() {
        Ok(Some(filter)) => Box::new(
            fmt()
                .with_env_filter(filter)
                .with_writer(writer)
                .with_span_events(fmt::format::FmtSpan::CLOSE)
                .finish(),
        ) as Box<dyn tracing::Subscriber + Send + Sync>,
        Ok(None) | Err(_) => Box::new(
            fmt()
                .with_max_level(LevelFilter::WARN)
                .with_writer(writer)
                .with_span_events(fmt::format::FmtSpan::CLOSE)
                .finish(),
        ) as Box<dyn tracing::Subscriber + Send + Sync>,
    }
}

/// Resolves the effective log filter directives from `SCAP_LOG`, falling
/// back to `RUST_LOG`.
///
/// Mirrors the precedence the previous `EnvFilter::try_from_env` /
/// `try_from_default_env` chain had: a variable that is unset, or holds
/// directives that fail to parse, is treated the same as absent and the
/// next source is tried. Returns the resolved directives, if any, alongside
/// the raw value of the first variable that was set but failed to parse, so
/// the caller can report it instead of dropping it silently.
fn resolve_log_env() -> (Option<String>, Option<String>) {
    let mut invalid = None;
    for var in ["SCAP_LOG", "RUST_LOG"] {
        let Ok(value) = std::env::var(var) else {
            continue;
        };
        if EnvFilter::try_new(&value).is_ok() {
            return (Some(value), None);
        }
        invalid.get_or_insert(value);
    }
    (None, invalid)
}

/// Installs the process's `tracing` subscriber as the global default.
pub fn init_tracing() {
    let (log_env, invalid) = resolve_log_env();
    let subscriber = build_subscriber(log_env.as_deref(), std::io::stderr);
    tracing::subscriber::set_global_default(subscriber)
        .expect("tracing subscriber already installed");
    if let Some(directives) = invalid {
        tracing::warn!(%directives, "invalid log filter directive; using WARN level instead");
    }
}

pub fn run() -> anyhow::Result<()> {
    init_tracing();

    let cli = cli::Cli::parse();
    cli::dispatch(cli)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
