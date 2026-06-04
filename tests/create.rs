use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as SysCommand;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

struct Env {
    home: TempDir,
    root: TempDir,
}

impl Env {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let cfg = home.path().join("gitconfig");
        fs::File::create(&cfg).unwrap();
        Self { home, root }
    }

    fn scap(&self) -> Command {
        let cfg = self.home.path().join("gitconfig");
        let mut cmd = Command::cargo_bin("scap").unwrap();
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &cfg)
            .env("SCAP_ROOT", self.root.path())
            .env("HOME", self.home.path());
        cmd
    }

    fn root_path(&self) -> &Path {
        self.root.path()
    }
}

fn expected_dest(env: &Env, host: &str, owner: &str, name: &str, bare: bool) -> PathBuf {
    let mut p = env.root_path().to_path_buf();
    p.push(host);
    for seg in owner.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    if bare {
        p.push(format!("{name}.git"));
    } else {
        p.push(name);
    }
    p
}

fn is_bare(dir: &Path) -> bool {
    let out = SysCommand::new("git")
        .current_dir(dir)
        .args(["rev-parse", "--is-bare-repository"])
        .output()
        .expect("git rev-parse");
    out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true"
}

#[test]
fn create_makes_fresh_directory_with_git_init() {
    let env = Env::new();
    env.scap()
        .args(["create", "motemen/ghq"])
        .assert()
        .success();
    let dest = expected_dest(&env, "github.com", "motemen", "ghq", false);
    assert!(
        dest.join(".git").is_dir(),
        "{} should have .git/",
        dest.display()
    );
    assert!(!is_bare(&dest), "should not be bare");
}

#[test]
fn create_accepts_pre_existing_empty_directory() {
    let env = Env::new();
    let dest = expected_dest(&env, "github.com", "motemen", "ghq", false);
    fs::create_dir_all(&dest).unwrap();
    env.scap()
        .args(["create", "motemen/ghq"])
        .assert()
        .success();
    assert!(dest.join(".git").is_dir());
}

#[test]
fn create_rejects_pre_existing_non_empty_directory() {
    let env = Env::new();
    let dest = expected_dest(&env, "github.com", "motemen", "ghq", false);
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("README.md"), "hi").unwrap();

    let expected_msg = format!(
        "directory \"{}\" already exists and not empty",
        dest.display()
    );
    env.scap()
        .args(["create", "motemen/ghq"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(expected_msg));
}

#[test]
fn create_bare_produces_bare_repo_with_git_suffix() {
    let env = Env::new();
    env.scap()
        .args(["create", "--bare", "motemen/ghq"])
        .assert()
        .success();
    let dest = expected_dest(&env, "github.com", "motemen", "ghq", true);
    assert!(
        dest.to_string_lossy().ends_with(".git"),
        "{} should end in .git",
        dest.display()
    );
    assert!(is_bare(&dest), "{} should be bare", dest.display());
}

#[test]
fn create_vcs_git_alias_accepted() {
    let env = Env::new();
    env.scap()
        .args(["create", "--vcs", "git", "motemen/ghq"])
        .assert()
        .success();
    let dest = expected_dest(&env, "github.com", "motemen", "ghq", false);
    assert!(dest.join(".git").is_dir());
}

#[test]
fn create_vcs_svn_rejected() {
    let env = Env::new();
    env.scap()
        .args(["create", "--vcs", "svn", "motemen/ghq"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported VCS"))
        .stderr(predicate::str::contains("git only"));
}

#[test]
fn create_stdout_emits_destination_path() {
    let env = Env::new();
    let dest = expected_dest(&env, "github.com", "motemen", "ghq", false);
    let expected_stdout = format!("{}\n", dest.display());
    env.scap()
        .args(["create", "motemen/ghq"])
        .assert()
        .success()
        .stdout(predicate::str::diff(expected_stdout));
}
