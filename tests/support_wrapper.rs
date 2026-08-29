//! Proves the recording wrapper itself before other tests rely on it: a
//! wrapper that silently failed to record, or that shadowed `git` with
//! something that is not `git`, would make every "no spawn" assertion pass
//! vacuously.

use std::process::Command;

mod support;

use support::{RecordingGit, empty_path_dir};

/// Run a command with the wrapper's `PATH` and variables in place.
fn through_wrapper(git: &RecordingGit, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new("git");
    cmd.args(args).env("PATH", git.path_prepend());
    for (k, v) in git.env() {
        cmd.env(k, v);
    }
    cmd.output().expect("run git through the recording wrapper")
}

#[test]
fn wrapper_records_one_line_and_delegates_to_the_real_git() {
    let git = RecordingGit::new();

    let wrapped = through_wrapper(&git, &["--version"]);
    assert!(wrapped.status.success(), "wrapped `git --version` failed: {wrapped:?}");

    let direct =
        Command::new(git.real_git()).arg("--version").output().expect("run the real git directly");

    assert_eq!(
        String::from_utf8_lossy(&wrapped.stdout),
        String::from_utf8_lossy(&direct.stdout),
        "the wrapper must pass the real git's stdout through unchanged"
    );

    assert_eq!(git.lines(), vec!["--version".to_owned()], "expected exactly one recorded line");
}

#[test]
fn wrapper_records_a_real_repository_creation() {
    let git = RecordingGit::new();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).expect("mkdir repo");

    let out = through_wrapper(&git, &["-C", repo.to_str().expect("utf-8 path"), "init", "-q"]);
    assert!(out.status.success(), "wrapped `git init` failed: {out:?}");

    assert!(repo.join(".git").is_dir(), "the delegated `git init` must really create the repo");

    let lines = git.lines();
    assert_eq!(lines.len(), 1, "expected one recorded line, got {lines:?}");
    assert!(lines[0].starts_with("-C "), "the log must record the full argv: {lines:?}");
    assert!(lines[0].contains("init"), "the log must record the subcommand: {lines:?}");
}

#[test]
fn wrapper_accumulates_one_line_per_invocation() {
    let git = RecordingGit::new();

    through_wrapper(&git, &["--version"]);
    through_wrapper(&git, &["--help"]);
    through_wrapper(&git, &["--version"]);

    assert_eq!(
        git.lines(),
        vec!["--version".to_owned(), "--help".to_owned(), "--version".to_owned()]
    );
}

#[test]
fn empty_path_dir_hides_every_binary() {
    let dir = empty_path_dir();
    assert!(
        std::fs::read_dir(dir.path()).expect("read the probe dir").next().is_none(),
        "the empty-PATH probe directory must really be empty"
    );

    // With that directory as the whole PATH, `git` cannot be resolved -- which
    // is exactly the condition the plan's empty-PATH probe relies on.
    let err = Command::new("git").arg("--version").env("PATH", dir.path()).output();
    assert!(err.is_err(), "expected `git` to be unresolvable under the empty PATH");
}
