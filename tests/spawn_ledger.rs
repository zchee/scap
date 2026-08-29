//! The spawn ledger for the commands that own no VCS operation.
//!
//! Plan §8 V-3 states the whole ledger as an assertion over the recording
//! wrapper's log: `root` and `list` spawn nothing at all by default, and at
//! most one `config --list` under the A3 backend (`SCAP_CONFIG_BACKEND=git`),
//! while `get` spawns one `clone` per target and nothing else (tests/get.rs).
//! `root`'s half landed with W2.1 in tests/root.rs; this file carries
//! `list`'s, which no other file was in a position to assert.

use std::fs;
use std::path::Path;
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use tempfile::TempDir;

mod support;

use support::RecordingGit;

/// A root holding exactly one repository, `github.com/a/b`, created with the
/// real `git` rather than by writing a `.git` directory by hand.
fn root_with_one_repo() -> TempDir {
    let root = TempDir::new().expect("tempdir for the list root");
    let repo = root.path().join("github.com/a/b");
    fs::create_dir_all(&repo).expect("mkdir the repository");
    let out = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .output()
        .expect("run git init for the fixture repository");
    assert!(out.status.success(), "git init failed: {out:?}");
    root
}

/// `scap list` against `root`, with `scap.root` supplied only through a
/// url-section-free `GIT_CONFIG_GLOBAL` fixture and the recording wrapper
/// first on `PATH`.
fn list_through_wrapper(
    home: &Path,
    root: &Path,
    git: &RecordingGit,
    backend: Option<&str>,
) -> AssertCommand {
    let cfg = home.join("gitconfig");
    fs::write(&cfg, format!("[scap]\n\troot = {}\n", root.display())).expect("write the fixture");

    let mut cmd = AssertCommand::cargo_bin("scap").expect("locate the scap binary");
    cmd.env_remove("SCAP_ROOT")
        .env_remove("SCAP_CONFIG_BACKEND")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("HOME", home)
        .env("PATH", git.path_prepend())
        .current_dir(home);
    for (key, value) in git.env() {
        cmd.env(key, value);
    }
    if let Some(backend) = backend {
        cmd.env("SCAP_CONFIG_BACKEND", backend);
    }
    cmd
}

#[test]
fn list_spawns_no_git_at_all_by_default() {
    let home = TempDir::new().unwrap();
    let root = root_with_one_repo();
    let git = RecordingGit::new();

    list_through_wrapper(home.path(), root.path(), &git, None)
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::diff("github.com/a/b\n"));

    assert_eq!(git.lines(), Vec::<String>::new(), "the A4 default must spawn nothing for `list`");
}

#[test]
fn list_spawns_exactly_one_config_list_under_the_git_backend() {
    let home = TempDir::new().unwrap();
    let root = root_with_one_repo();
    let git = RecordingGit::new();

    list_through_wrapper(home.path(), root.path(), &git, Some("git"))
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::diff("github.com/a/b\n"));

    let lines = git.lines();
    assert_eq!(lines.len(), 1, "the A3 backend is one spawn per process, got {lines:?}");
    assert!(lines[0].starts_with("config --list"), "unexpected invocation: {lines:?}");
}
