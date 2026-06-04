use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use tempfile::TempDir;

fn isolated(cmd: &mut AssertCommand, home: &Path, root: &Path) {
    let cfg = home.join("gitconfig");
    if !cfg.exists() {
        fs::File::create(&cfg).unwrap();
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("SCAP_ROOT", root)
        .env("HOME", home);
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

fn init_origin_with_commit() -> (TempDir, TempDir) {
    let origin = init_bare_origin();
    let work = TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(work.path())
        .status()
        .unwrap();
    fs::write(work.path().join("README.md"), b"hello\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["branch", "-M", "main"])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            &format!("file://{}", origin.path().display()),
        ])
        .current_dir(work.path())
        .status()
        .unwrap();
    let out = Command::new("git")
        .args(["push", "-q", "origin", "main"])
        .current_dir(work.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "push failed: {out:?}");
    (origin, work)
}

fn scap_dest_for(root: &Path, origin: &Path, bare: bool) -> PathBuf {
    let mut p = root.to_path_buf();
    for comp in origin.components() {
        if let std::path::Component::Normal(s) = comp {
            p.push(s);
        }
    }
    if bare {
        let last = p.file_name().unwrap().to_string_lossy().into_owned();
        p.set_file_name(format!("{last}.git"));
    }
    p
}

#[test]
fn get_fresh_clone_creates_working_dir() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let origin = init_bare_origin();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", &format!("file://{}", origin.path().display())])
        .assert()
        .success();

    let dest = scap_dest_for(root.path(), origin.path(), false);
    assert!(dest.join(".git").is_dir(), "git dir at {}", dest.display());
}

#[test]
fn get_re_clone_is_noop_without_update() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let origin = init_bare_origin();

    let url = format!("file://{}", origin.path().display());
    for _ in 0..2 {
        let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
        isolated(&mut cmd, home.path(), root.path());
        cmd.args(["get", &url]).assert().success();
    }

    let dest = scap_dest_for(root.path(), origin.path(), false);
    assert!(dest.join(".git").is_dir());
}

#[test]
fn get_update_no_upstream_runs_fetch() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let (origin, _work) = init_origin_with_commit();
    let url = format!("file://{}", origin.path().display());

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", &url]).assert().success();

    let dest = scap_dest_for(root.path(), origin.path(), false);
    Command::new("git")
        .args(["config", "--unset-all", "branch.main.remote"])
        .current_dir(&dest)
        .status()
        .ok();
    Command::new("git")
        .args(["config", "--unset-all", "branch.main.merge"])
        .current_dir(&dest)
        .status()
        .ok();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "-u", &url]).assert().success();
}

#[test]
fn get_update_with_upstream_pulls_ff_only() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let (origin, work) = init_origin_with_commit();
    let url = format!("file://{}", origin.path().display());

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", &url]).assert().success();

    fs::write(work.path().join("NEW.md"), b"second\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", "second"])
        .current_dir(work.path())
        .status()
        .unwrap();
    let out = Command::new("git")
        .args(["push", "-q", "origin", "main"])
        .current_dir(work.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "second push failed: {out:?}");

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "-u", &url]).assert().success();

    let dest = scap_dest_for(root.path(), origin.path(), false);
    assert!(dest.join("NEW.md").exists(), "post-pull NEW.md missing");
}

#[test]
fn get_bare_clone_creates_bare_dir() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let (origin, _) = init_origin_with_commit();
    let url = format!("file://{}", origin.path().display());

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "--bare", &url]).assert().success();

    let dest = scap_dest_for(root.path(), origin.path(), true);
    assert!(
        dest.file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".git"),
        "dest should end in .git: {}",
        dest.display()
    );
    assert!(dest.join("HEAD").is_file(), "bare HEAD missing");
}

#[test]
fn get_bare_update_uses_fetch_refspec() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let (origin, work) = init_origin_with_commit();
    let url = format!("file://{}", origin.path().display());

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "--bare", &url]).assert().success();

    fs::write(work.path().join("NEW.md"), b"second\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", "second"])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["push", "-q", "origin", "main"])
        .current_dir(work.path())
        .status()
        .unwrap();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "--bare", "-u", &url]).assert().success();
}

#[test]
fn get_shallow_creates_shallow_marker() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let (origin, _) = init_origin_with_commit();
    let url = format!("file://{}", origin.path().display());

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "--shallow", &url]).assert().success();

    let dest = scap_dest_for(root.path(), origin.path(), false);
    assert!(
        dest.join(".git").join("shallow").is_file(),
        ".git/shallow marker missing at {}",
        dest.display()
    );
}

#[test]
fn get_branch_clones_specified_branch() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let (origin, work) = init_origin_with_commit();
    Command::new("git")
        .args(["checkout", "-q", "-b", "feature"])
        .current_dir(work.path())
        .status()
        .unwrap();
    fs::write(work.path().join("BRANCH.md"), b"feature\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "feature",
        ])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["push", "-q", "origin", "feature"])
        .current_dir(work.path())
        .status()
        .unwrap();

    let url = format!("file://{}", origin.path().display());
    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "--branch", "feature", &url])
        .assert()
        .success();

    let dest = scap_dest_for(root.path(), origin.path(), false);
    assert!(dest.join("BRANCH.md").exists(), "feature file missing");
}

#[test]
fn get_rejects_unsupported_vcs() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "--vcs", "svn", "https://example.com/x/y"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported VCS"))
        .stderr(predicate::str::contains("git only"));
}

#[test]
fn get_concurrent_clone_exits_75() {
    use fs2::FileExt;

    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    let dest = scap_dest_for(root.path(), origin.path(), false);
    let lock_dir = dest.parent().unwrap();
    fs::create_dir_all(lock_dir).unwrap();
    let name = dest.file_name().unwrap().to_string_lossy().into_owned();
    let lock_path = lock_dir.join(format!(".scap-lock-{name}"));
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    FileExt::lock_exclusive(&lock_file).unwrap();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", &url])
        .assert()
        .code(75)
        .stderr(predicate::str::contains("another scap process"));

    FileExt::unlock(&lock_file).unwrap();
}

#[test]
fn get_atomic_clone_cleans_stale_tmp() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    let dest = scap_dest_for(root.path(), origin.path(), false);
    let parent = dest.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    let name = dest.file_name().unwrap().to_string_lossy().into_owned();
    let stale_tmp = parent.join(format!("{name}.tmp-2"));
    fs::create_dir_all(&stale_tmp).unwrap();
    fs::write(stale_tmp.join("leftover.txt"), b"junk").unwrap();
    assert!(stale_tmp.exists(), "fixture should exist before");

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", &url]).assert().success();

    assert!(
        !stale_tmp.exists(),
        "stale tmp dir was not cleaned: {}",
        stale_tmp.display()
    );
    assert!(dest.join(".git").is_dir(), "clone did not complete");
}

#[test]
fn get_parallel_via_stdin_clones_multiple() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();

    let o1 = init_bare_origin();
    let o2 = init_bare_origin();
    let o3 = init_bare_origin();

    let urls = format!(
        "file://{}\nfile://{}\nfile://{}\n",
        o1.path().display(),
        o2.path().display(),
        o3.path().display(),
    );

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", "-P"]).write_stdin(urls).assert().success();

    for o in [&o1, &o2, &o3] {
        let dest = scap_dest_for(root.path(), o.path(), false);
        assert!(
            dest.join(".git").is_dir(),
            "clone missing for {}",
            o.path().display()
        );
    }
}

#[test]
fn get_look_exports_scap_look_env() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    let probe = home.path().join("probe.sh");
    let stamp = home.path().join("captured");
    fs::write(
        &probe,
        format!(
            "#!/bin/sh\nprintf '%s' \"$SCAP_LOOK\" > {}\n",
            stamp.display()
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&probe).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(&probe, perms).unwrap();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.env("SHELL", &probe)
        .args(["get", "--look", "--silent", &url])
        .assert()
        .success();

    let captured = fs::read_to_string(&stamp).unwrap();
    assert!(
        captured.contains("/"),
        "SCAP_LOOK should be host/owner/name; got {captured:?}"
    );
}

#[test]
fn get_spans_emit_with_scap_log_debug() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    let out = cmd
        .env("SCAP_LOG", "trace")
        .args(["get", &url])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("scap::cmd::get") || s.contains("process_target"),
        "expected scap::cmd::get span; got: {s}"
    );
    assert!(
        s.contains("scap::vcs::git") || s.contains("clone"),
        "expected scap::vcs::git::clone span; got: {s}"
    );
}
