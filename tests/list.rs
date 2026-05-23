use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serial_test::serial;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs as unix_fs;

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

fn init_repo_with_dot_git(root: &Path, rel: &str) {
    let dest = root.join(rel);
    fs::create_dir_all(&dest).unwrap();
    let git_dir = root.join(format!("{rel}-gitdir"));
    let out = std::process::Command::new("git")
        .arg("init")
        .arg("--separate-git-dir")
        .arg(&git_dir)
        .arg(&dest)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git init --separate-git-dir failed: {:?}",
        out
    );
}

fn write_gitignore(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel).join(".gitignore");
    fs::write(path, contents).unwrap();
}

fn init_repo_with_gitfile_marker(root: &Path, rel: &str) {
    let dest = root.join(rel);
    fs::create_dir_all(&dest).unwrap();
    let out = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&dest)
        .output()
        .unwrap();
    assert!(out.status.success(), "git init failed: {:?}", out);

    let git_dir = dest.join(".git");
    let real_git_dir = root.join(format!("{}.gitdir", rel.replace('/', "_")));
    fs::rename(&git_dir, &real_git_dir).unwrap();
    fs::write(&git_dir, format!("gitdir: {}\n", real_git_dir.display())).unwrap();
}

fn init_repo_at(path: &Path, bare: bool) {
    fs::create_dir_all(path).unwrap();
    let mut g = std::process::Command::new("git");
    g.arg("init").arg("-q");
    if bare {
        g.arg("--bare");
    }
    let out = g.current_dir(path).output().unwrap();
    assert!(out.status.success(), "git init failed: {:?}", out);
}

fn write_text(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
}

fn init_repo_with_gitdir_marker(root: &Path, rel: &str) {
    let dest = root.join(rel);
    fs::create_dir_all(&dest).unwrap();
    let gitdir = dest.join(".git-real");
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(dest.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
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
#[ignore]
fn list_direct_root_repo() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "direct", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("direct\n"));
}

#[test]
#[serial]
fn list_prunes_nested_repos_below_repo_root() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/a/x/nested/child", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
#[serial]
fn list_hidden_repo_path_is_not_filtered() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden/repo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq(".hidden/repo\n"));
}

#[test]
#[serial]
fn list_ignores_gitignore_patterns_when_listing_repos() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "ignored/repo", false);
    write_text(&root.path().join(".gitignore"), "ignored\n");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("ignored/repo\n"));
}

#[test]
#[serial]
#[ignore]
fn list_symlinked_repo_is_discovered_once() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    init_repo(target.path(), "real/repo", false);
    #[cfg(unix)]
    std::os::unix::fs::symlink(target.path(), root.path().join("linked")).unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    #[cfg(unix)]
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("real/repo\n"));
    #[cfg(not(unix))]
    cmd.arg("list").assert().success().stdout(predicate::eq(""));
}

#[test]
#[serial]
fn list_reports_dot_when_root_is_a_repo() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq(".\n"));
}

#[test]
#[serial]
fn list_prunes_direct_root_repo_contents() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".", false);
    fs::create_dir_all(root.path().join("pkg/level/depth")).unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq(".\n"));
}

#[test]
#[serial]
fn list_includes_hidden_and_ignored_repo_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden", false);
    fs::write(root.path().join(".gitignore"), b"*.hidden\n").unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq(".hidden\n"));
}

#[test]
#[cfg(unix)]
#[serial]
#[ignore]
fn list_reports_real_and_symlinked_repos() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "real", false);
    let symlink_path = root.path().join("repo_link");
    symlink("real", &symlink_path).unwrap();

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("real\nrepo_link\n"));
}

#[test]
#[serial]
#[ignore]
fn list_detects_git_file_repositories() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo_with_dot_git(root.path(), "repo");

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("repo\n"));
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
fn list_prunes_subtrees_beneath_repositories() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/a/x/nested", false);

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
#[serial]
fn list_keeps_hidden_paths_in_results() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden/github.com/a/x", false);

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq(".hidden/github.com/a/x\n"));
}

#[test]
#[serial]
fn list_does_not_filter_ignored_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    write_gitignore(root.path(), "", "github.com/a/x\n");

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
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

#[test]
#[serial]
fn list_prunes_nested_repositories_under_repo() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/a/x/sub/inner", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
#[serial]
fn list_keeps_hidden_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden/github.com/a/x", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq(".hidden/github.com/a/x\n"));
}

#[test]
#[serial]
fn list_does_not_filter_gitignored_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    fs::write(root.path().join(".gitignore"), "*\n").unwrap();
    init_repo(root.path(), "ignored/github.com/a/x", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("ignored/github.com/a/x\n"));
}

#[test]
#[serial]
#[ignore]
fn list_recognizes_gitfile_markers() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo_with_gitfile_marker(root.path(), "github.com/a/x");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
#[serial]
fn list_prunes_nested_repo_subtree() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/outer", false);
    init_repo(root.path(), "github.com/a/outer/nested/inner", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/outer\n"));
}

#[test]
#[serial]
fn list_includes_hidden_repo_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden/org/proj", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq(".hidden/org/proj\n"));
}

#[test]
#[serial]
fn list_does_not_filter_by_parent_gitignore() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    fs::write(root.path().join(".gitignore"), "github.com/\n").unwrap();
    init_repo(root.path(), "github.com/ignored/repo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/ignored/repo\n"));
}

#[test]
#[serial]
#[ignore]
fn list_detects_git_file_markers() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo_with_gitfile_marker(root.path(), "github.com/a/worktree");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/worktree\n"));
}

#[cfg(unix)]
#[test]
#[serial]
#[ignore]
fn list_includes_symlinked_repo_path_when_present() {
    use std::os::unix::fs::symlink;

    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let target = root.path().join("github.com/a/real");
    init_repo_at(&target, false);
    let link = root.path().join("github.com/a/link");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&target, &link).unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/link\ngithub.com/a/real\n"));
}

#[test]
#[serial]
fn list_keeps_hidden_repositories_and_nonrepo_hidden_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/.hidden", false);
    fs::create_dir_all(root.path().join("github.com/a/.cache")).unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/.hidden\n"));
}

#[test]
#[serial]
fn list_does_not_filter_by_gitignore_contents() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    fs::write(root.path().join(".gitignore"), "github.com/a/x\n").unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
#[serial]
#[ignore]
fn list_includes_symlinked_repository_target() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    #[cfg(unix)]
    {
        let link = root.path().join("mirror");
        unix_fs::symlink(root.path().join("github.com/a/x"), &link).unwrap();
    }
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
#[serial]
#[ignore]
fn list_detects_gitdir_marker_repositories() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo_with_gitdir_marker(root.path(), "github.com/a/x");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
}
