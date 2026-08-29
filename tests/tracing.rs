//! E2E gate for W1.4 (plan §9 AC-6 (b), ADR-6): the subscriber built by
//! `build_subscriber`/`init_tracing` (src/lib.rs) must stay silent under
//! default settings and must be demonstrably live under `SCAP_LOG=debug`.

use std::fs;
use std::path::Path;
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use tempfile::TempDir;

/// Point `scap` at an isolated `HOME`/config and `SCAP_ROOT` so it never
/// touches the real user's git config or repo tree (mirrors tests/get.rs).
fn isolated(cmd: &mut AssertCommand, home: &Path, root: &Path) {
    let cfg = home.join("gitconfig");
    if !cfg.exists() {
        fs::File::create(&cfg).unwrap();
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("SCAP_ROOT", root)
        .env("HOME", home);
}

fn init_bare_origin() -> TempDir {
    let dir = TempDir::new().unwrap();
    let out = Command::new("git")
        .args(["init", "-q", "--bare"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "git init --bare failed: {out:?}");
    dir
}

/// Which case applies: `scap --version` cannot exercise this gate. clap's
/// `--version` handling exits the process from inside `Cli::parse()` (called
/// after `init_tracing()` in `scap::run`, src/lib.rs), so `cli::dispatch` --
/// and therefore every span/event in the codebase -- never runs; verified by
/// running the built binary directly with and without `SCAP_LOG=debug`, both
/// producing zero stderr bytes. `list`/`root`/`rm`/`create` have no `tracing`
/// calls yet (Phase 2b instrumentation for `list` has not landed), and no
/// default-settings WARN path exists yet either (that is AC-8b, W2b.1). So
/// the only command that reaches instrumented code today is `get`
/// (`process_target`, src/cmd/get.rs:102-104), used here against a local
/// `file://` origin (no network) with `--silent` (suppresses `git`'s own
/// stdout/stderr so only scap's own output can appear on stderr).
#[test]
fn default_settings_are_silent_and_scap_log_debug_is_live() {
    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let mut quiet_cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut quiet_cmd, home.path(), root.path());
    let quiet = quiet_cmd.args(["get", "--silent", &url]).assert().success();
    let quiet_stderr = quiet.get_output().stderr.clone();
    assert!(
        quiet_stderr.is_empty(),
        "no log env must leave stderr empty on a clean run; got: {}",
        String::from_utf8_lossy(&quiet_stderr)
    );

    let home2 = TempDir::new().unwrap();
    let root2 = TempDir::new().unwrap();
    let mut verbose_cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut verbose_cmd, home2.path(), root2.path());
    let verbose =
        verbose_cmd.env("SCAP_LOG", "debug").args(["get", "--silent", &url]).assert().success();
    let verbose_stderr = verbose.get_output().stderr.clone();
    let s = String::from_utf8_lossy(&verbose_stderr);
    assert!(!s.is_empty(), "SCAP_LOG=debug must produce stderr output (the subscriber is live)");
    assert!(
        s.contains("scap::cmd::get") || s.contains("processing target"),
        "expected the process_target debug event/span on stderr; got: {s}"
    );
}
