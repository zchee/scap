use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

mod support;

use support::{RecordingGit, empty_path_dir};

/// The two configuration backends every case in this file runs under: the
/// A4 in-process default, and the A3 `git config --list` backend selected by
/// `SCAP_CONFIG_BACKEND=git` (ADR-8, R8's escape hatch). Both must produce
/// byte-identical stdout.
const BACKENDS: [Option<&str>; 2] = [None, Some("git")];

struct Fixture {
    _home: TempDir,
    home_path: PathBuf,
    _cfg: TempDir,
    cfg_path: PathBuf,
}

impl Fixture {
    fn new(gitconfig: &str) -> Self {
        let home = TempDir::new().unwrap();
        let cfg = TempDir::new().unwrap();
        let cfg_path = cfg.path().join("gitconfig");
        fs::write(&cfg_path, gitconfig).unwrap();
        Self { home_path: home.path().to_path_buf(), _home: home, cfg_path, _cfg: cfg }
    }

    fn cmd(&self) -> Command {
        self.cmd_with(None)
    }

    fn cmd_with(&self, backend: Option<&str>) -> Command {
        let mut c = Command::cargo_bin("scap").unwrap();
        c.env_remove("SCAP_ROOT")
            .env_remove("SCAP_CONFIG_BACKEND")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.cfg_path)
            .env("HOME", &self.home_path)
            // Keep repository discovery out of whatever tree the test runner
            // happens to sit in.
            .current_dir(&self.home_path);
        if let Some(backend) = backend {
            c.env("SCAP_CONFIG_BACKEND", backend);
        }
        c
    }
}

#[test]
fn root_prints_first_ghq_root_env_entry() {
    let f = Fixture::new("");
    for backend in BACKENDS {
        f.cmd_with(backend)
            .arg("root")
            .env("SCAP_ROOT", "/path1:/path2")
            .assert()
            .success()
            .stdout(predicate::str::diff("/path1\n"));
    }
}

#[test]
fn root_all_prints_every_ghq_root_env_entry() {
    let f = Fixture::new("");
    for backend in BACKENDS {
        f.cmd_with(backend)
            .arg("root")
            .arg("--all")
            .env("SCAP_ROOT", "/path1:/path2")
            .assert()
            .success()
            .stdout(predicate::str::diff("/path1\n/path2\n"));
    }
}

#[test]
fn root_reverses_multi_root_from_gitconfig() {
    let f = Fixture::new("[scap]\n\troot = /a\n\troot = /b\n\troot = /c\n");
    for backend in BACKENDS {
        f.cmd_with(backend).arg("root").assert().success().stdout(predicate::str::diff("/c\n"));
    }
}

#[test]
fn root_all_reverses_multi_root_from_gitconfig() {
    let f = Fixture::new("[scap]\n\troot = /a\n\troot = /b\n\troot = /c\n");
    for backend in BACKENDS {
        f.cmd_with(backend)
            .arg("root")
            .arg("--all")
            .assert()
            .success()
            .stdout(predicate::str::diff("/c\n/b\n/a\n"));
    }
}

#[test]
fn root_all_appends_urlmatch_roots_when_ghq_root_unset() {
    let cfg = "[scap]\n\troot = /default\n\
              [scap \"https://example.com\"]\n\troot = /custom\n";
    let f = Fixture::new(cfg);

    for backend in BACKENDS {
        f.cmd_with(backend)
            .arg("root")
            .assert()
            .success()
            .stdout(predicate::str::diff("/default\n"));

        f.cmd_with(backend)
            .arg("root")
            .arg("--all")
            .assert()
            .success()
            .stdout(predicate::str::contains("/default\n"))
            .stdout(predicate::str::contains("/custom\n"));
    }
}

#[test]
fn root_falls_back_to_home_ghq_when_unconfigured() {
    let f = Fixture::new("");
    let expected = format!("{}/scap\n", f.home_path.display());
    for backend in BACKENDS {
        f.cmd_with(backend)
            .arg("root")
            .assert()
            .success()
            .stdout(predicate::str::diff(expected.clone()));
    }
}

// -- spawn accounting (V-3, AC-1/AC-1') -----------------------------------

#[test]
fn root_spawns_no_git_at_all_by_default() {
    let f = Fixture::new("[scap]\n\troot = /a\n\troot = /b\n");
    let git = RecordingGit::new();

    let mut cmd = f.cmd();
    cmd.env("PATH", git.path_prepend());
    for (key, value) in git.env() {
        cmd.env(key, value);
    }
    cmd.arg("root").assert().success().stdout(predicate::str::diff("/b\n"));

    assert_eq!(git.lines(), Vec::<String>::new(), "the A4 default must spawn nothing");
}

#[test]
fn root_spawns_exactly_one_config_list_under_the_git_backend() {
    let f = Fixture::new("[scap]\n\troot = /a\n\troot = /b\n");
    let git = RecordingGit::new();

    let mut cmd = f.cmd_with(Some("git"));
    cmd.env("PATH", git.path_prepend());
    for (key, value) in git.env() {
        cmd.env(key, value);
    }
    cmd.arg("root").assert().success().stdout(predicate::str::diff("/b\n"));

    let lines = git.lines();
    assert_eq!(lines.len(), 1, "the A3 backend is one spawn per process, got {lines:?}");
    assert!(lines[0].starts_with("config --list"), "unexpected invocation: {lines:?}");
}

#[test]
fn root_resolves_without_any_git_on_path() {
    // The plan's empty-PATH probe: no `SCAP_ROOT`, `scap.root` supplied only
    // through `GIT_CONFIG_GLOBAL`, and nothing executable reachable. It fails
    // on every build before W2.1 because resolving the root meant spawning
    // `git config`.
    let f = Fixture::new("[scap]\n\troot = /from-config\n");
    let empty = empty_path_dir();

    f.cmd()
        .arg("root")
        .env("PATH", empty.path())
        .assert()
        .success()
        .stdout(predicate::str::diff("/from-config\n"));
}

#[test]
fn root_fails_fast_when_a_trigger_needs_git_and_none_is_reachable() {
    // ADR-8 forbids silently falling back to the in-process snapshot when a
    // trigger fired, so this must exit 1 with the trigger named.
    let f = Fixture::new("[scap]\n\troot = /from-config\n");
    let empty = empty_path_dir();

    f.cmd_with(Some("git"))
        .arg("root")
        .env("PATH", empty.path())
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("SCAP_CONFIG_BACKEND"));
}
