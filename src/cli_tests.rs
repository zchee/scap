use clap::Parser;

use super::*;

/// Parse an argv the way `main` does, then route it through [`dispatch`].
///
/// Every case below picks a command whose *own* module rejects the arguments
/// before it reads any configuration, so the assertion proves the routing
/// without touching the environment, spawning `git`, or depending on the
/// developer's `scap.root`.
fn dispatch_argv(argv: &[&str]) -> anyhow::Result<()> {
    dispatch(Cli::parse_from(argv))
}

#[test]
fn dispatch_routes_get_to_the_get_command() {
    // `cmd::get::run` validates --vcs first (src/cmd/get.rs:16).
    let err = dispatch_argv(&["scap", "get", "--vcs", "hg", "zchee/scap"])
        .expect_err("unsupported --vcs must fail");
    assert!(err.to_string().contains("unsupported VCS"), "unexpected error: {err}");
    assert!(err.to_string().contains("hg"), "error does not name the rejected vcs: {err}");
}

#[test]
fn dispatch_routes_the_clone_alias_to_the_get_command() {
    // `clone` is a visible alias for `get` (src/cli.rs Cmd::Get); it must land
    // on the same handler rather than on a command of its own.
    let err = dispatch_argv(&["scap", "clone", "--vcs", "hg", "zchee/scap"])
        .expect_err("unsupported --vcs must fail");
    assert!(err.to_string().contains("unsupported VCS"), "unexpected error: {err}");
}

#[test]
fn dispatch_routes_list_to_the_list_command() {
    // `cmd::list::run` validates --vcs before resolving any root
    // (src/cmd/list.rs:69-77), so this never reads configuration.
    let err =
        dispatch_argv(&["scap", "list", "--vcs", "hg"]).expect_err("unsupported --vcs must fail");
    assert!(err.to_string().contains("unsupported VCS"), "unexpected error: {err}");
}

#[test]
fn dispatch_routes_rm_to_the_rm_command() {
    // `cmd::rm::run` rejects an empty target first (src/cmd/rm.rs:13-15).
    let err = dispatch_argv(&["scap", "rm", ""]).expect_err("empty target must fail");
    assert!(err.to_string().contains("repository name is required"), "unexpected error: {err}");
}

#[test]
fn dispatch_routes_create_to_the_create_command() {
    // `cmd::create::run` rejects an empty target first (src/cmd/create.rs:13-15).
    let err = dispatch_argv(&["scap", "create", ""]).expect_err("empty target must fail");
    assert!(err.to_string().contains("repository name is required"), "unexpected error: {err}");
}

// `Cmd::Root` has no argument-only failure mode -- `cmd::root::run` resolves
// roots immediately -- so routing it hermetically here would mean mutating the
// process environment, which this crate forbids outside src/config_tests.rs.
// It is covered end to end through the compiled binary in tests/root.rs, which
// reaches `root::run` only via `dispatch`.
