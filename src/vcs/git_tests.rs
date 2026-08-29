use std::fs;

use tempfile::TempDir;

use super::*;

/// Build the four entries a bare repository is recognised by
/// (`dest_is_git_repo`'s second arm): HEAD, objects/, refs/.
fn make_bare_repo(path: &std::path::Path) {
    fs::create_dir_all(path.join("objects")).expect("objects");
    fs::create_dir_all(path.join("refs")).expect("refs");
    fs::write(path.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD");
}

#[test]
fn dest_is_git_repo_accepts_a_worktree_with_a_dot_git_directory() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).expect("mkdir .git");

    assert!(dest_is_git_repo(&repo));
}

#[test]
fn dest_is_git_repo_rejects_a_plain_directory_and_a_missing_path() {
    let tmp = TempDir::new().expect("tempdir");
    let plain = tmp.path().join("plain");
    fs::create_dir_all(&plain).expect("mkdir");

    assert!(!dest_is_git_repo(&plain), "a directory without .git is not a repository");
    assert!(!dest_is_git_repo(&tmp.path().join("absent")), "a missing path is not a repository");
}

#[test]
fn dest_is_git_repo_rejects_a_dot_git_file_because_the_arm_requires_a_directory() {
    // A linked worktree stores `.git` as a *file* containing `gitdir: ...`.
    // Today's implementation only accepts a directory, so this pins the
    // current behaviour rather than an aspiration.
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("worktree");
    fs::create_dir_all(&repo).expect("mkdir");
    fs::write(repo.join(".git"), b"gitdir: /elsewhere/.git/worktrees/wt\n").expect("write .git");

    assert!(!dest_is_git_repo(&repo));
}

#[test]
fn dest_is_git_repo_accepts_a_complete_bare_repository_named_dot_git() {
    let tmp = TempDir::new().expect("tempdir");
    let bare = tmp.path().join("project.git");
    make_bare_repo(&bare);

    assert!(dest_is_git_repo(&bare));
}

#[test]
fn dest_is_git_repo_requires_both_the_dot_git_suffix_and_the_bare_layout() {
    let tmp = TempDir::new().expect("tempdir");

    // Right layout, wrong name: the suffix test fails.
    let unsuffixed = tmp.path().join("project");
    make_bare_repo(&unsuffixed);
    assert!(!dest_is_git_repo(&unsuffixed), "bare layout without the .git suffix");

    // Right name, incomplete layout: each missing entry alone is disqualifying.
    let no_head = tmp.path().join("no-head.git");
    make_bare_repo(&no_head);
    fs::remove_file(no_head.join("HEAD")).expect("rm HEAD");
    assert!(!dest_is_git_repo(&no_head), "missing HEAD");

    let no_objects = tmp.path().join("no-objects.git");
    make_bare_repo(&no_objects);
    fs::remove_dir_all(no_objects.join("objects")).expect("rm objects");
    assert!(!dest_is_git_repo(&no_objects), "missing objects/");

    let no_refs = tmp.path().join("no-refs.git");
    make_bare_repo(&no_refs);
    fs::remove_dir_all(no_refs.join("refs")).expect("rm refs");
    assert!(!dest_is_git_repo(&no_refs), "missing refs/");
}

#[test]
fn stale_tmp_paths_returns_only_siblings_whose_pid_is_dead() {
    let tmp = TempDir::new().expect("tempdir");
    let dest = tmp.path().join("repo");

    // A pid that is certainly not running: pid 0 is never a live user process,
    // and `kill -0 0` addresses the process group rather than succeeding as a
    // liveness probe would. Use a high, unallocated pid instead so the probe is
    // unambiguous, and this process's own pid for the live case.
    let dead_pid = 4_194_303_i32; // above the default kern.maxproc pid ceiling
    let live_pid = std::process::id();

    let dead = tmp.path().join(format!("repo.tmp-{dead_pid}"));
    let live = tmp.path().join(format!("repo.tmp-{live_pid}"));
    let other_repo = tmp.path().join("other.tmp-1");
    let not_a_pid = tmp.path().join("repo.tmp-notanumber");
    for p in [&dead, &live, &other_repo, &not_a_pid] {
        fs::create_dir_all(p).expect("mkdir tmp dir");
    }
    fs::create_dir_all(&dest).expect("mkdir dest");

    let got = stale_tmp_paths(&dest);

    assert_eq!(got, vec![dead.clone()], "expected only the dead-pid sibling, got {got:?}");
    assert!(!got.contains(&live), "a live pid's directory must be kept");
    assert!(!got.contains(&other_repo), "another repository's tmp dir must be ignored");
    assert!(!got.contains(&not_a_pid), "a non-numeric suffix must be ignored");
}

#[test]
fn stale_tmp_paths_is_empty_when_the_parent_cannot_be_read() {
    let tmp = TempDir::new().expect("tempdir");
    // Parent exists but holds nothing matching the prefix.
    assert!(stale_tmp_paths(&tmp.path().join("repo")).is_empty());
    // Parent does not exist at all: read_dir fails and the function yields
    // nothing rather than propagating.
    assert!(stale_tmp_paths(&tmp.path().join("absent/repo")).is_empty());
    // A path without a parent yields nothing.
    assert!(stale_tmp_paths(std::path::Path::new("/")).is_empty());
}
