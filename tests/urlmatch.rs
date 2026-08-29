//! W2.2: what the ADR-8 rule (d) delegation actually spawns.
//!
//! `git config --path --get-urlmatch` is the only `git` scap still runs for
//! configuration, and only for a user whose gitconfig holds url-scoped
//! `[scap "<url>"]` sections. These tests count the spawns through the
//! recording wrapper -- a real `git`, not a mock -- so "one spawn per
//! distinct URL per process" is an assertion rather than a claim.

use std::fs;
use std::path::Path;
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use tempfile::TempDir;

mod support;

use support::RecordingGit;

/// The one line of the wrapper log a rule (d) delegation writes.
const URLMATCH: &str = "config --path --get-urlmatch";

/// Write the fixture gitconfig into `home`: `scap.root` plus, when
/// `url_sections`, the sections that arm rule (d).
fn write_config(home: &Path, root: &Path, url_sections: bool) {
    let mut cfg = format!("[scap]\n\troot = {}\n", root.display());
    if url_sections {
        cfg.push_str("[scap \"https://one.example.com/\"]\n\troot = /r-one\n");
    }
    fs::write(home.join("gitconfig"), cfg).unwrap();
}

/// A `scap` that resolves `git` through the recording wrapper and reads the
/// fixture gitconfig. `SCAP_ROOT` is deliberately absent: it would take
/// ADR-8 rule (a) and never reach the delegation.
fn scap(git: &RecordingGit, home: &Path) -> AssertCommand {
    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    cmd.env_remove("SCAP_ROOT")
        .env_remove("SCAP_CONFIG_BACKEND")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("XDG_CONFIG_HOME")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("HOME", home)
        .env("PATH", git.path_prepend());
    for (key, value) in git.env() {
        cmd.env(key, value);
    }
    cmd
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

fn urlmatch_lines(git: &RecordingGit) -> Vec<String> {
    git.lines().into_iter().filter(|line| line.starts_with(URLMATCH)).collect()
}

fn clone_lines(git: &RecordingGit) -> Vec<String> {
    git.lines().into_iter().filter(|line| line.starts_with("clone")).collect()
}

#[test]
fn parallel_get_spawns_one_urlmatch_per_distinct_origin() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let git = RecordingGit::new();
    write_config(home.path(), root.path(), true);

    // Real local repositories, so the clone that follows the resolution is
    // real too and the log holds exactly what a `get --parallel` run costs.
    let origins: Vec<TempDir> = (0..4).map(|_| init_bare_origin()).collect();
    let urls: Vec<String> =
        origins.iter().map(|o| format!("file://{}", o.path().display())).collect();

    scap(&git, home.path()).arg("get").arg("--parallel").args(&urls).assert().success();

    let delegations = urlmatch_lines(&git);
    assert_eq!(
        delegations.len(),
        urls.len(),
        "one delegation per distinct URL, six workers notwithstanding: {delegations:?}"
    );
    for url in &urls {
        assert_eq!(
            delegations.iter().filter(|line| line.ends_with(url.as_str())).count(),
            1,
            "{url} must be asked exactly once: {delegations:?}"
        );
    }

    let clones = clone_lines(&git);
    assert_eq!(clones.len(), urls.len(), "one clone per target: {clones:?}");
    assert_eq!(
        git.lines().len(),
        delegations.len() + clones.len(),
        "the delegations and the clones must be the whole log: {:?}",
        git.lines()
    );

    // Now the same four origins, each listed twice, through a fresh log.
    // Every destination exists by now, so the workers skip the clone and no
    // two of them contend for one destination lock -- what is left is eight
    // rule (d) lookups of four distinct URLs, run concurrently.
    let git = RecordingGit::new();
    let mut doubled: Vec<String> = Vec::with_capacity(urls.len() * 2);
    for url in &urls {
        doubled.push(url.clone());
        doubled.push(url.clone());
    }
    scap(&git, home.path()).arg("get").arg("--parallel").args(&doubled).assert().success();

    let delegations = urlmatch_lines(&git);
    assert_eq!(
        delegations.len(),
        urls.len(),
        "eight concurrent lookups of four URLs must spawn four times: {delegations:?}"
    );
    assert!(clone_lines(&git).is_empty(), "nothing left to clone: {:?}", git.lines());
}

#[test]
fn one_origin_asked_twice_spawns_one_urlmatch_and_says_so_in_the_span() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let git = RecordingGit::new();
    write_config(home.path(), root.path(), true);

    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    let output = scap(&git, home.path())
        .env("SCAP_LOG", "debug")
        .args(["get", &url, &url])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let log = String::from_utf8_lossy(&output).into_owned();

    let delegations = urlmatch_lines(&git);
    assert_eq!(delegations.len(), 1, "the second lookup must be memoised: {delegations:?}");

    // The span carries the running spawn count, which is where
    // `urlmatch_spawns` can be recorded honestly: `scap::config::load` has
    // long closed by the time rule (d) runs.
    assert!(
        log.contains("scap::config::urlmatch"),
        "SCAP_LOG=debug must emit the per-lookup span; got: {log}"
    );
    assert!(
        log.contains("spawned=true") && log.contains("spawned=false"),
        "one spawn and one memo hit must be distinguishable; got: {log}"
    );
    assert_eq!(
        log.matches("urlmatch_spawns=1").count(),
        2,
        "both lookups must report the same running count of one; got: {log}"
    );
    assert!(
        log.contains("reason=url_sections"),
        "the load span must name the trigger that armed rule (d); got: {log}"
    );
}

#[test]
fn get_without_url_sections_never_spawns_urlmatch() {
    // The W2.1 guarantee kept as a regression guard: with no url-scoped
    // section visible, ADR-8 rule (c) answers from the in-process snapshot
    // and the only `git` in the log is the clone itself.
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let git = RecordingGit::new();
    write_config(home.path(), root.path(), false);

    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    scap(&git, home.path()).args(["get", &url]).assert().success();

    assert!(urlmatch_lines(&git).is_empty(), "no url sections, no spawn: {:?}", git.lines());
    assert_eq!(clone_lines(&git).len(), 1, "{:?}", git.lines());
    assert_eq!(git.lines().len(), 1, "the clone must be the whole log: {:?}", git.lines());
}
