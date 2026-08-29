use std::fs;
#[cfg(unix)]
use std::os::unix::fs as unix_fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn isolated(cmd: &mut Command, home: &Path, root: &Path) {
    let cfg = home.join("gitconfig");
    if !cfg.exists() {
        fs::File::create(&cfg).unwrap();
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("SCAP_ROOT", root)
        .env("HOME", home)
        .env_remove("SCAP_CONFIG_BACKEND")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("GIT_DIR")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("SCAP_LIST_EXCLUDE")
        // The two walker knobs. `SCAP_LIST_DETECT` selects the `.git`
        // detection strategy and `SCAP_LIST_THREADS` the worker count;
        // neither changes what is printed, but an ambient value would make
        // the counter assertions below read a tree the test did not choose.
        .env_remove("SCAP_LIST_DETECT")
        .env_remove("SCAP_LIST_THREADS")
        .env_remove("SCAP_LOG")
        .env_remove("RUST_LOG")
        // Repository discovery reads the configuration of whatever
        // repository contains the working directory, and the test runner's
        // is the scap checkout itself. Pin it to the fixture.
        .current_dir(home);
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
    assert!(out.status.success(), "git init --separate-git-dir failed: {:?}", out);
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
fn list_empty_root_produces_no_output() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(""));
}

#[test]
fn list_prints_relative_paths_sorted() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/b/y", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\ngithub.com/b/y\n"));
}

#[test]
fn list_direct_root_repo() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "direct", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("direct\n"));
}

#[test]
fn list_prunes_nested_repos_below_repo_root() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/a/x/nested/child", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
fn list_hidden_repo_path_is_not_filtered() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden/repo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(".hidden/repo\n"));
}

#[test]
fn list_ignores_gitignore_patterns_when_listing_repos() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "ignored/repo", false);
    write_text(&root.path().join(".gitignore"), "ignored\n");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("ignored/repo\n"));
}

/// ADR-9 rule (iii): a symlink to a directory that is *not itself* a
/// repository is never descended, so the repositories under it are not
/// listed at all.
///
/// Was `list_symlinked_repo_is_discovered_once`, expecting `real/repo` --
/// the repository reached *through* the link. Both the name and the
/// expectation predate anyone asking the oracle. W0.4 case 14a asked it: on
/// exactly this fixture `ghq list` prints nothing, because ghq's walker
/// refuses to recurse through a symlink (walker.go:85-90). Nothing is
/// discovered here, once or otherwise, so the name said something untrue
/// about the very behaviour the test now pins.
#[cfg(unix)]
#[test]
fn list_does_not_descend_a_symlinked_non_repo_dir() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    init_repo(target.path(), "real/repo", false);
    std::os::unix::fs::symlink(target.path(), root.path().join("linked")).unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(""));
}

#[test]
fn list_reports_dot_when_root_is_a_repo() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(".\n"));
}

#[test]
fn list_prunes_direct_root_repo_contents() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".", false);
    fs::create_dir_all(root.path().join("pkg/level/depth")).unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(".\n"));
}

#[test]
fn list_includes_hidden_and_ignored_repo_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden", false);
    fs::write(root.path().join(".gitignore"), b"*.hidden\n").unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(".hidden\n"));
}

#[test]
#[cfg(unix)]
fn list_reports_real_and_symlinked_repos() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "real", false);
    let symlink_path = root.path().join("repo_link");
    symlink("real", &symlink_path).unwrap();

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("real\nrepo_link\n"));
}

#[test]
fn list_detects_git_file_repositories() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo_with_dot_git(root.path(), "repo");

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("repo\n"));
}

#[test]
fn list_full_path_prints_absolute_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    let real_root = fs::canonicalize(root.path()).unwrap();
    let expected = format!("{}/github.com/a/x\n", real_root.display());
    cmd.args(["list", "-p"]).assert().success().stdout(predicate::eq(expected));
}

#[test]
fn list_unique_prints_shortest_unambiguous_subpath() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/b/y", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["list", "--unique"]).assert().success().stdout(predicate::eq("x\ny\n"));
}

#[test]
fn list_substring_query_filters() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/alpha/foo", false);
    init_repo(root.path(), "github.com/beta/bar", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["list", "alpha"]).assert().success().stdout(predicate::eq("github.com/alpha/foo\n"));
}

#[test]
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
        .args(["config", "--file", cfg_path, "--add", "scap.root", r1_path])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "--file", cfg_path, "--add", "scap.root", r2_path])
        .output()
        .unwrap();

    let mut cmd = Command::cargo_bin("scap").unwrap();
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env_remove("SCAP_ROOT")
        .env("HOME", home.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\ngithub.com/b/y\n"));
}

#[test]
fn list_prunes_subtrees_beneath_repositories() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/a/x/nested", false);

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
fn list_keeps_hidden_paths_in_results() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden/github.com/a/x", false);

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(".hidden/github.com/a/x\n"));
}

#[test]
fn list_does_not_filter_ignored_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    write_gitignore(root.path(), "", "github.com/a/x\n");

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
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
fn list_prunes_nested_repositories_under_repo() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/a/x/sub/inner", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
fn list_keeps_hidden_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden/github.com/a/x", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(".hidden/github.com/a/x\n"));
}

#[test]
fn list_does_not_filter_gitignored_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    fs::write(root.path().join(".gitignore"), "*\n").unwrap();
    init_repo(root.path(), "ignored/github.com/a/x", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("ignored/github.com/a/x\n"));
}

#[test]
fn list_recognizes_gitfile_markers() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo_with_gitfile_marker(root.path(), "github.com/a/x");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
fn list_prunes_nested_repo_subtree() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/outer", false);
    init_repo(root.path(), "github.com/a/outer/nested/inner", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/outer\n"));
}

#[test]
fn list_includes_hidden_repo_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), ".hidden/org/proj", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq(".hidden/org/proj\n"));
}

#[test]
fn list_does_not_filter_by_parent_gitignore() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    fs::write(root.path().join(".gitignore"), "github.com/\n").unwrap();
    init_repo(root.path(), "github.com/ignored/repo", false);
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/ignored/repo\n"));
}

#[test]
fn list_detects_git_file_markers() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo_with_gitfile_marker(root.path(), "github.com/a/worktree");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/worktree\n"));
}

#[cfg(unix)]
#[test]
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
fn list_keeps_hidden_repositories_and_nonrepo_hidden_paths() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/.hidden", false);
    fs::create_dir_all(root.path().join("github.com/a/.cache")).unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/.hidden\n"));
}

#[test]
fn list_does_not_filter_by_gitignore_contents() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    fs::write(root.path().join(".gitignore"), "github.com/a/x\n").unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\n"));
}

/// ADR-9 rule (iii): a symlink whose target *is* a repository is emitted,
/// at the link's own path, alongside the target.
///
/// The counterpart of `list_does_not_descend_a_symlinked_non_repo_dir`,
/// and the second expectation W0.4 rewrote (case 14b). The old one named
/// `github.com/a/x`; ghq prints `mirror` too, because it resolves the link
/// for `IsDir`, stats `<link>/.git`, and then calls back with the link path
/// (local_repository.go:268-299). scap already emitted both lines when this
/// test was written -- the assertion, not the code, was the stale half.
#[cfg(unix)]
#[test]
fn list_includes_symlinked_repository_target() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    let link = root.path().join("mirror");
    unix_fs::symlink(root.path().join("github.com/a/x"), &link).unwrap();
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\nmirror\n"));
}

#[test]
fn list_detects_gitdir_marker_repositories() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo_with_gitdir_marker(root.path(), "github.com/a/x");
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("github.com/a/x\n"));
}

// -- ADR-9 rule (viii): opt-in root-relative exclusions ---------------------

/// Read one `name=<digits>` field back out of a `scap::walk::root` span close
/// line, the way plan §5 says the counters are observed: run the binary with
/// `SCAP_LOG=debug`, capture stderr, and match the field.
fn span_field(stderr: &str, field: &str) -> Option<usize> {
    let needle = format!("{field}=");
    let start = stderr.find(&needle)? + needle.len();
    stderr[start..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

/// A root holding two repositories plus a deep non-repository subtree under
/// `github.com/zchee/big.bak`, which is the shape corpus a has: one
/// user-identifiable directory that dominates the directory reads without
/// containing anything `list` would print.
fn exclusion_fixture(root: &Path) {
    init_repo(root, "github.com/a/x", false);
    init_repo(root, "bar/foo", false);
    fs::create_dir_all(root.join("github.com/zchee/big.bak/deep/deeper")).unwrap();
    init_repo(root, "github.com/zchee/big.bak/inner", false);
}

fn write_scap_config(home: &Path, body: &str) {
    fs::write(home.join("gitconfig"), body).unwrap();
}

#[test]
fn list_exclude_env_prunes_the_named_subtree() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.env("SCAP_LIST_EXCLUDE", "github.com/zchee/big.bak")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("bar/foo\ngithub.com/a/x\n"));
}

#[test]
fn list_exclude_records_dirs_read_and_excluded_on_the_walk_span() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    let stderr_of = |detect: &str, exclude: Option<&str>| {
        let mut cmd = Command::cargo_bin("scap").unwrap();
        isolated(&mut cmd, home.path(), root.path());
        cmd.env("SCAP_LOG", "debug").env("SCAP_LIST_DETECT", detect);
        if let Some(pattern) = exclude {
            cmd.env("SCAP_LIST_EXCLUDE", pattern);
        }
        let out = cmd.arg("list").assert().success();
        String::from_utf8_lossy(&out.get_output().stderr).into_owned()
    };

    // `big.bak`, `big.bak/deep` and `big.bak/deep/deeper` are read in the
    // baseline and not in the excluded run under either strategy. The fourth
    // directory is the repository `big.bak/inner`: open-and-scan opens it and
    // looks for `.git` among the entries it read, while stat-first asks for
    // `<dir>/.git` directly and never opens it. That difference is the whole
    // cost W3.0b is choosing between, and it is why `dirs_read` compares
    // within a strategy and not across them. The repositories found are the
    // same either way, which the stdout assertions around this test pin.
    for (detect, saved) in [("open", 4), ("stat", 3)] {
        let baseline = stderr_of(detect, None);
        let excluded = stderr_of(detect, Some("github.com/zchee/big.bak"));

        let baseline_dirs = span_field(&baseline, "dirs_read")
            .unwrap_or_else(|| panic!("no dirs_read on the close line: {baseline}"));
        let excluded_dirs = span_field(&excluded, "dirs_read")
            .unwrap_or_else(|| panic!("no dirs_read on the close line: {excluded}"));

        assert_eq!(span_field(&baseline, "excluded"), Some(0), "stderr: {baseline}");
        assert_eq!(span_field(&excluded, "excluded"), Some(1), "stderr: {excluded}");
        assert_eq!(
            excluded_dirs + saved,
            baseline_dirs,
            "detect={detect}: excluded run read {excluded_dirs} directories, \
             baseline {baseline_dirs}"
        );
    }
}

#[test]
fn list_exclude_non_matching_pattern_changes_nothing() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    let mut baseline = Command::cargo_bin("scap").unwrap();
    isolated(&mut baseline, home.path(), root.path());
    let baseline = baseline.arg("list").assert().success();
    let baseline = baseline.get_output().stdout.clone();

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    let out = cmd
        .env("SCAP_LOG", "debug")
        .env("SCAP_LIST_EXCLUDE", "no/such/directory")
        .arg("list")
        .assert()
        .success();
    assert_eq!(out.get_output().stdout, baseline);
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert_eq!(span_field(&stderr, "excluded"), Some(0), "stderr: {stderr}");
}

#[test]
fn list_exclude_pattern_is_anchored_at_the_root() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    // `bar/foo` exists, `foo` at the root does not: an unanchored match
    // would drop it, an anchored one leaves it alone.
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.env("SCAP_LIST_EXCLUDE", "foo")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("bar/foo\ngithub.com/a/x\ngithub.com/zchee/big.bak/inner\n"));
}

#[test]
fn list_exclude_single_star_does_not_cross_a_separator() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    // `*` stops at `/` (git's WM_PATHNAME), so a one-segment pattern cannot
    // reach a two-segment path; `**` can.
    let mut narrow = Command::cargo_bin("scap").unwrap();
    isolated(&mut narrow, home.path(), root.path());
    narrow.env("SCAP_LIST_EXCLUDE", "*").arg("list").assert().success().stdout(predicate::eq(""));

    let mut wide = Command::cargo_bin("scap").unwrap();
    isolated(&mut wide, home.path(), root.path());
    wide.env("SCAP_LIST_EXCLUDE", "github.com/*/big.bak")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("bar/foo\ngithub.com/a/x\n"));
}

#[test]
fn list_exclude_accepts_several_colon_separated_patterns() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.env("SCAP_LIST_EXCLUDE", "github.com/zchee/big.bak:bar")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/x\n"));
}

#[test]
fn list_exclude_can_name_a_repository_directory() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    // A repository directory is queued like any other, so a pattern that
    // names one drops it from the listing without touching its siblings.
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.env("SCAP_LIST_EXCLUDE", "github.com/a/x")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("bar/foo\ngithub.com/zchee/big.bak/inner\n"));
}

#[test]
fn list_exclude_folds_a_trailing_slash_in_a_pattern() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    // `.gitignore` habit: a trailing slash spells "directory". Every
    // exclusion candidate already is one, so both spellings must prune the
    // same subtree and report the same `excluded` count.
    for pattern in ["github.com/zchee/big.bak", "github.com/zchee/big.bak/"] {
        let mut cmd = Command::cargo_bin("scap").unwrap();
        isolated(&mut cmd, home.path(), root.path());
        let out = cmd
            .env("SCAP_LOG", "debug")
            .env("SCAP_LIST_EXCLUDE", pattern)
            .arg("list")
            .assert()
            .success();
        assert_eq!(
            String::from_utf8_lossy(&out.get_output().stdout),
            "bar/foo\ngithub.com/a/x\n",
            "pattern {pattern:?}"
        );
        let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
        assert_eq!(span_field(&stderr, "excluded"), Some(1), "pattern {pattern:?}: {stderr}");
    }
}

#[test]
fn list_exclude_config_form_matches_the_env_form() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());

    let mut via_env = Command::cargo_bin("scap").unwrap();
    isolated(&mut via_env, home.path(), root.path());
    let via_env = via_env
        .env("SCAP_LIST_EXCLUDE", "github.com/zchee/big.bak:bar")
        .arg("list")
        .assert()
        .success();
    let via_env = via_env.get_output().stdout.clone();

    write_scap_config(
        home.path(),
        "[scap]\n\tlistExclude = github.com/zchee/big.bak\n\tlistExclude = bar\n",
    );
    let mut via_config = Command::cargo_bin("scap").unwrap();
    isolated(&mut via_config, home.path(), root.path());
    let via_config = via_config.arg("list").assert().success();

    assert_eq!(via_config.get_output().stdout, via_env);
    assert_eq!(String::from_utf8_lossy(&via_env), "github.com/a/x\n");
}

#[test]
fn list_exclude_env_replaces_the_configured_patterns() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    exclusion_fixture(root.path());
    write_scap_config(home.path(), "[scap]\n\tlistExclude = bar\n");

    // The variable is the whole exclusion set, as `SCAP_ROOT` is the whole
    // root list: `bar` comes back even though the gitconfig excludes it.
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.env("SCAP_LIST_EXCLUDE", "github.com/zchee/big.bak")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("bar/foo\ngithub.com/a/x\n"));
}

// -- ADR-9 rules (v) and (vi): unreadable directories and roots -------------

/// Restores `dir`'s original mode when dropped.
///
/// A guard rather than a straight-line call, so that a panic inside the
/// closure -- an assertion firing, or `Command` failing -- cannot leave a
/// mode-000 directory behind in `$TMPDIR`, which `TempDir` would then be
/// unable to remove.
#[cfg(unix)]
struct RestoreMode<'a> {
    dir: &'a Path,
    original: fs::Permissions,
}

#[cfg(unix)]
impl Drop for RestoreMode<'_> {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.dir, self.original.clone());
    }
}

/// Run `run` with `dir` at mode 000.
///
/// Returns `None` when mode 000 does not actually deny this user, which is
/// the case for root: the test that called it has nothing to observe and
/// should skip rather than fail on a true negative.
#[cfg(unix)]
fn with_unreadable_dir(
    dir: &Path,
    run: impl FnOnce() -> std::process::Output,
) -> Option<std::process::Output> {
    use std::os::unix::fs::PermissionsExt;

    let original = fs::metadata(dir).unwrap().permissions();
    let _guard = RestoreMode { dir, original: original.clone() };
    fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(dir).is_ok() {
        return None;
    }
    Some(run())
}

/// The message a chmod-000 test prints when it skips.
#[cfg(unix)]
fn skip_not_denied(test: &str) {
    eprintln!("{test}: skipped -- mode 000 does not deny this user (running as root?)");
}

#[cfg(unix)]
#[test]
fn list_warns_and_continues_past_an_unreadable_directory() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    init_repo(root.path(), "github.com/b/y", false);
    let locked = root.path().join("locked");
    fs::create_dir_all(locked.join("deep")).unwrap();

    let Some(out) = with_unreadable_dir(&locked, || {
        let mut cmd = Command::cargo_bin("scap").unwrap();
        isolated(&mut cmd, home.path(), root.path());
        cmd.arg("list").output().unwrap()
    }) else {
        skip_not_denied("list_warns_and_continues_past_an_unreadable_directory");
        return;
    };

    // AC-8b: exit 0, the readable part of the tree listed in full, and the
    // skipped path named on stderr under default settings -- no log
    // variable, so this is the WARN subscriber scap always builds.
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "github.com/a/x\ngithub.com/b/y\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Permission denied"), "stderr: {stderr}");
    assert!(stderr.contains(&locked.display().to_string()), "stderr: {stderr}");
}

#[test]
fn list_skips_a_non_existent_root_without_a_word() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let missing = root.path().join("not-created");

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    let out = cmd.env("SCAP_ROOT", &missing).arg("list").output().unwrap();

    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

#[cfg(unix)]
#[test]
fn list_warns_and_skips_an_unreadable_root() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let locked = root.path().join("locked");
    fs::create_dir_all(&locked).unwrap();

    let Some(out) = with_unreadable_dir(&locked, || {
        let mut cmd = Command::cargo_bin("scap").unwrap();
        isolated(&mut cmd, home.path(), root.path());
        cmd.env("SCAP_ROOT", &locked).arg("list").output().unwrap()
    }) else {
        skip_not_denied("list_warns_and_skips_an_unreadable_root");
        return;
    };

    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Permission denied"), "stderr: {stderr}");
    assert!(stderr.contains(&locked.display().to_string()), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn list_warns_and_skips_a_root_whose_stat_fails_for_another_reason() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let locked = root.path().join("locked");
    let inner = locked.join("inner");
    fs::create_dir_all(&inner).unwrap();

    // `stat(<locked>/inner)` fails with EACCES rather than ENOENT because
    // the parent cannot be searched. ghq dereferences a nil `FileInfo`
    // there and panics, so this expectation is scap's own (ADR-13).
    let Some(out) = with_unreadable_dir(&locked, || {
        let mut cmd = Command::cargo_bin("scap").unwrap();
        isolated(&mut cmd, home.path(), root.path());
        cmd.env("SCAP_ROOT", &inner).arg("list").output().unwrap()
    }) else {
        skip_not_denied("list_warns_and_skips_a_root_whose_stat_fails_for_another_reason");
        return;
    };

    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Permission denied"), "stderr: {stderr}");
    assert!(stderr.contains(&inner.display().to_string()), "stderr: {stderr}");
}

#[test]
fn list_warns_and_skips_a_root_below_a_regular_file() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let file = root.path().join("file");
    fs::write(&file, "not a directory\n").unwrap();
    let below = file.join("dir");

    // The W0.4 oracle fixture for ADR-9 (vi)'s third case: `stat` fails
    // with ENOTDIR. ghq panics on it (`local_repository.go:321`, nil
    // `FileInfo`), so scap's behaviour is asserted directly rather than
    // against the oracle (ADR-13).
    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    let out = cmd.env("SCAP_ROOT", &below).arg("list").output().unwrap();

    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&below.display().to_string()), "stderr: {stderr}");
    assert!(stderr.contains("Not a directory"), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn list_is_silent_about_a_dangling_symlink_by_default() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    // `follow_links(true)` resolves every symlink's metadata, so a link to
    // a file that no longer exists is a walk error. ghq prints nothing for
    // it, and one stale link would otherwise put a line on every run, so
    // rule (v) reserves stderr for permission errors and sends the rest to
    // the debug level. W2b.2's `follow_links(false)` removes the class.
    symlink(root.path().join("github.com/a/x/gone.txt"), root.path().join("dangling")).unwrap();

    let mut quiet = Command::cargo_bin("scap").unwrap();
    isolated(&mut quiet, home.path(), root.path());
    let quiet = quiet.arg("list").output().unwrap();

    assert!(quiet.status.success(), "expected exit 0, got {:?}", quiet.status);
    assert_eq!(String::from_utf8_lossy(&quiet.stdout), "github.com/a/x\n");
    assert_eq!(
        String::from_utf8_lossy(&quiet.stderr),
        "",
        "a dangling symlink must not reach stderr under default settings"
    );

    // Non-silent, though: `SCAP_LOG=debug` still names it.
    let mut loud = Command::cargo_bin("scap").unwrap();
    isolated(&mut loud, home.path(), root.path());
    let loud = loud.env("SCAP_LOG", "debug").arg("list").output().unwrap();

    assert!(loud.status.success(), "expected exit 0, got {:?}", loud.status);
    assert_eq!(String::from_utf8_lossy(&loud.stdout), "github.com/a/x\n");
    let stderr = String::from_utf8_lossy(&loud.stderr);
    assert!(stderr.contains("dangling"), "stderr: {stderr}");
    assert!(stderr.contains("No such file or directory"), "stderr: {stderr}");
}

// -- ADR-9 rules (iii), (iv) and (vii): symlink, `.git` and root semantics --
//
// Each of these is pinned against the real `ghq` in tests/parity_ghq.rs as
// well. The pair is deliberate: the parity test proves scap agrees with the
// oracle and skips when no oracle is installed, and the test here states
// what the agreed answer *is*, so the semantics stay covered on a machine
// with no `ghq` and a diff shows which rule a change moved.

/// ADR-9 rule (iii): a link that resolves to nothing is not an entry, and
/// says nothing on stderr (W0.4 case 4 -- ghq is silent for a loop too).
#[cfg(unix)]
#[test]
fn list_is_silent_about_a_symlink_loop() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    init_repo(root.path(), "github.com/a/x", false);
    symlink("loop-b", root.path().join("loop-a")).unwrap();
    symlink("loop-a", root.path().join("loop-b")).unwrap();

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    let out = cmd.arg("list").output().unwrap();

    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "github.com/a/x\n");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "", "a symlink loop must not reach stderr");
}

/// ADR-9 rule (iv): the `.git` entry may be a symlink, and then it has to
/// resolve. A dangling one leaves an ordinary directory (W0.4 case 5); one
/// pointing at a real git directory makes a repository (case 6).
#[cfg(unix)]
#[test]
fn list_requires_a_dot_git_symlink_to_resolve() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();

    let dangling = root.path().join("dangling-git");
    fs::create_dir_all(&dangling).unwrap();
    symlink(root.path().join("nowhere"), dangling.join(".git")).unwrap();

    let donor = root.path().join("github.com/a/donor");
    init_repo_at(&donor, false);
    let borrowed = root.path().join("github.com/a/borrowed");
    fs::create_dir_all(&borrowed).unwrap();
    symlink(donor.join(".git"), borrowed.join(".git")).unwrap();

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list")
        .assert()
        .success()
        .stdout(predicate::eq("github.com/a/borrowed\ngithub.com/a/donor\n"));
}

/// ADR-9 rules (ii) and (iii): the `.git` *suffix* test is applied to the
/// entry's own name, so it reads a symlink's name and not its target's.
/// `link -> upstream.git` is therefore not a repository and `link.git ->
/// upstream.git` is (W0.4 case 7).
#[cfg(unix)]
#[test]
fn list_reads_the_git_suffix_off_the_link_name_not_the_target() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let upstream = store.path().join("upstream.git");
    init_repo_at(&upstream, true);

    symlink(&upstream, root.path().join("link-to-bare")).unwrap();
    symlink(&upstream, root.path().join("named.git")).unwrap();

    let mut cmd = Command::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.arg("list").assert().success().stdout(predicate::eq("named.git\n"));
}

/// ADR-9 rule (vii): no cross-root de-duplication. A relative path present
/// in three roots is printed three times by `list`, once per root by `list
/// -p`, and collapses only under `--unique`, which needs those duplicates to
/// decide anything (cmd_list.go:78-110).
#[test]
fn list_prints_a_duplicated_relative_path_once_per_root() {
    let home = TempDir::new().unwrap();
    let roots: Vec<TempDir> = (0..3).map(|_| TempDir::new().unwrap()).collect();
    for r in &roots {
        init_repo(r.path(), "github.com/a/dup", false);
    }
    init_repo(roots[1].path(), "github.com/b/only", false);

    let joined = std::env::join_paths(roots.iter().map(TempDir::path)).unwrap();
    let run = |flags: &[&str]| {
        let mut cmd = Command::cargo_bin("scap").unwrap();
        isolated(&mut cmd, home.path(), roots[0].path());
        let out = cmd.env("SCAP_ROOT", &joined).args(flags).output().unwrap();
        assert!(out.status.success(), "{flags:?} exited {:?}", out.status);
        String::from_utf8(out.stdout).unwrap()
    };

    assert_eq!(
        run(&["list"]),
        "github.com/a/dup\ngithub.com/a/dup\ngithub.com/a/dup\ngithub.com/b/only\n"
    );

    // `-p` distinguishes them, so every root contributes its own line.
    let full = run(&["list", "-p"]);
    for r in &roots {
        let real = fs::canonicalize(r.path()).unwrap();
        assert!(
            full.contains(&format!("{}/github.com/a/dup\n", real.display())),
            "missing {} in {full}",
            real.display()
        );
    }
    assert_eq!(full.lines().count(), 4, "{full}");

    // `--unique` keeps the shortest unambiguous subpath, once.
    assert_eq!(run(&["list", "--unique"]), "dup\nonly\n");
}
