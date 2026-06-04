use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn isolated_env(cmd: &mut Command, home: &Path, root: &Path) {
    let cfg = home.join("gitconfig");
    if !cfg.exists() {
        fs::File::create(&cfg).unwrap();
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("SCAP_ROOT", root)
        .env("HOME", home);
}

fn pre_init_repo(root: &Path, rel: &str, bare: bool) -> PathBuf {
    let dest = root.join(rel);
    fs::create_dir_all(&dest).unwrap();
    let mut git = std::process::Command::new("git");
    git.arg("init").current_dir(&dest);
    if bare {
        git.arg("--bare");
    }
    let out = git.output().unwrap();
    assert!(
        out.status.success(),
        "git init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    dest
}

#[test]
fn rm_errors_when_target_is_missing() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let missing = root.path().join("github.com/motemen/nonexistent");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated_env(&mut cmd, home.path(), root.path());
    cmd.args(["rm", "motemen/nonexistent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "directory \"{}\" does not exist",
            missing.display()
        )));
}

#[test]
fn rm_dry_run_does_not_remove() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let dest = pre_init_repo(root.path(), "github.com/motemen/foo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated_env(&mut cmd, home.path(), root.path());
    cmd.args(["rm", "--dry-run", "motemen/foo"])
        .assert()
        .success()
        .stdout(predicate::str::diff(format!(
            "Would remove {}\n",
            dest.display()
        )));
    assert!(dest.exists(), "dry-run must not remove");
    assert!(dest.join(".git").exists(), "dry-run must leave .git");
}

#[test]
fn rm_confirm_yes_removes() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let dest = pre_init_repo(root.path(), "github.com/motemen/foo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated_env(&mut cmd, home.path(), root.path());
    cmd.args(["rm", "motemen/foo"])
        .write_stdin("y\n")
        .assert()
        .success()
        .stdout(predicate::str::diff(format!(
            "Removed {}\n",
            dest.display()
        )));
    assert!(!dest.exists(), "confirmed rm must remove dest");
}

#[test]
fn rm_confirm_no_aborts() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let dest = pre_init_repo(root.path(), "github.com/motemen/foo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated_env(&mut cmd, home.path(), root.path());
    cmd.args(["rm", "motemen/foo"])
        .write_stdin("n\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("aborted"));
    assert!(dest.exists(), "rejected rm must leave dest");
    assert!(dest.join(".git").exists(), "rejected rm must leave .git");
}

#[test]
fn rm_empty_input_aborts() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let dest = pre_init_repo(root.path(), "github.com/motemen/foo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated_env(&mut cmd, home.path(), root.path());
    cmd.args(["rm", "motemen/foo"])
        .write_stdin("\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("aborted"));
    assert!(dest.exists(), "empty input must abort, not remove");
}

#[test]
fn rm_bare_removes_dotgit_dest() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let dest = pre_init_repo(root.path(), "github.com/motemen/foo.git", true);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated_env(&mut cmd, home.path(), root.path());
    cmd.args(["rm", "--bare", "motemen/foo"])
        .write_stdin("y\n")
        .assert()
        .success()
        .stdout(predicate::str::diff(format!(
            "Removed {}\n",
            dest.display()
        )));
    assert!(!dest.exists(), "bare rm must remove <path>.git");
}

#[test]
fn rm_uppercase_y_does_not_match() {
    // ghq cmd_rm.go:confirm matches exact "y" only.
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let dest = pre_init_repo(root.path(), "github.com/motemen/foo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated_env(&mut cmd, home.path(), root.path());
    cmd.args(["rm", "motemen/foo"])
        .write_stdin("Y\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("aborted"));
    assert!(dest.exists(), "non-exact 'Y' must abort per ghq parity");
}
