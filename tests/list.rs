use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serial_test::serial;
use tempfile::TempDir;

fn isolated(cmd: &mut Command, home: &Path, root: &Path) {
    let cfg = home.join("gitconfig");
    if !cfg.exists() {
        fs::File::create(&cfg).unwrap();
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("GHQ_ROOT", root)
        .env("HOME", home);
}

fn init_repo(root: &Path, rel: &str, bare: bool) {
    let dest = root.join(rel);
    fs::create_dir_all(&dest).unwrap();
    let mut g = std::process::Command::new("git");
    g.arg("init").arg("-q").current_dir(&dest);
    if bare {
        g.arg("--bare");
    }
    let out = g.output().unwrap();
    assert!(out.status.success(), "git init failed: {:?}", out);
}

#[test]
#[serial]
fn list_empty_root_produces_no_output() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(""));
}

#[test]
#[serial]
fn list_prints_relative_paths_sorted() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/b/y", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\ngithub.com/b/y\n"));
}

#[test]
#[serial]
fn list_full_path_prints_absolute_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    let real_root = fs::canonicalize(root.path()).unwrap();
    let expected = format!("{}/github.com/a/x\n", real_root.display());
    cmd.args(["list", "-p"])
        .assert()
        .success()
        .stdout(predicate::eq(expected));
}

#[test]
#[serial]
fn list_unique_prints_shortest_unambiguous_subpath() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/b/y", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["list", "--unique"])
        .assert()
        .success()
        .stdout(predicate::eq("x\ny\n"));
}

#[test]
#[serial]
fn list_substring_query_filters() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/alpha/foo", false);
    init_repo(root.path(), "github.com/beta/bar", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["list", "alpha"])
        .assert()
        .success()
        .stdout(predicate::eq("github.com/alpha/foo\n"));
}

#[test]
#[serial]
fn list_exact_query_matches_subpath_only() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/alpha/foo", false);
    init_repo(root.path(), "github.com/beta/foobar", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["list", "-e", "foo"])
        .assert()
        .success()
        .stdout(predicate::eq("github.com/alpha/foo\n"));
}

#[test]
#[serial]
fn list_includes_bare_repo_in_default_output() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/c/z.git", true);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\ngithub.com/c/z.git\n"));
}

#[test]
#[serial]
fn list_bare_flag_does_not_filter_result_set() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/c/z.git", true);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["list", "--bare"])
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\ngithub.com/c/z.git\n"));
}

#[test]
#[serial]
fn list_walks_multiple_roots() {
    let home = TempDir::new().unwrap();
    let r1 = TempDir::new().unwrap();
    let r2 = TempDir::new().unwrap();
    init_repo(r1.path(), "github.com/a/x", false);
    init_repo(r2.path(), "github.com/b/y", false);

    let cfg = home.path().join("gitconfig");
    fs::File::create(&cfg).unwrap();
    let cfg_path = cfg.to_str().unwrap();
    let r1_path = r1.path().to_str().unwrap();
    let r2_path = r2.path().to_str().unwrap();
    std::process::Command::new("git")
        .args(["config", "--file", cfg_path, "--add", "ghq.root", r1_path])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "--file", cfg_path, "--add", "ghq.root", r2_path])
        .output()
        .unwrap();

    let mut cmd = Command::cargo_bin("scap").unwrap();
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env_remove("GHQ_ROOT")
        .env("HOME", home.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\ngithub.com/b/y\n"));
}

#[test]
#[serial]
fn list_rejects_non_git_vcs() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["list", "--vcs", "svn"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("git only"));
}
