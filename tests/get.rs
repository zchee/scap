use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use tempfile::TempDir;

mod support;

use support::RecordingGit;

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
    Command::new("git").args(["init", "-q"]).current_dir(work.path()).status().unwrap();
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
    Command::new("git").args(["add", "."]).current_dir(work.path()).status().unwrap();
    Command::new("git")
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"])
        .current_dir(work.path())
        .status()
        .unwrap();
    Command::new("git").args(["branch", "-M", "main"]).current_dir(work.path()).status().unwrap();
    Command::new("git")
        .args(["remote", "add", "origin", &format!("file://{}", origin.path().display())])
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
    cmd.args(["get", &format!("file://{}", origin.path().display())]).assert().success();

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
    Command::new("git").args(["add", "."]).current_dir(work.path()).status().unwrap();
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
        dest.file_name().unwrap().to_string_lossy().ends_with(".git"),
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
    Command::new("git").args(["add", "."]).current_dir(work.path()).status().unwrap();
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
    Command::new("git").args(["add", "."]).current_dir(work.path()).status().unwrap();
    Command::new("git")
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", "feature"])
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
    cmd.args(["get", "--branch", "feature", &url]).assert().success();

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
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    let dest = scap_dest_for(root.path(), origin.path(), false);
    let lock_dir = dest.parent().unwrap();
    fs::create_dir_all(lock_dir).unwrap();
    let name = dest.file_name().unwrap().to_string_lossy().into_owned();
    let lock_path = lock_dir.join(format!(".scap-lock-{name}"));
    let lock_file =
        fs::OpenOptions::new().create(true).write(true).truncate(false).open(&lock_path).unwrap();
    lock_file.lock().unwrap();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.args(["get", &url])
        .assert()
        .code(75)
        .stderr(predicate::str::contains("another scap process"));

    lock_file.unlock().unwrap();
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

    assert!(!stale_tmp.exists(), "stale tmp dir was not cleaned: {}", stale_tmp.display());
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
        assert!(dest.join(".git").is_dir(), "clone missing for {}", o.path().display());
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
    fs::write(&probe, format!("#!/bin/sh\nprintf '%s' \"$SCAP_LOOK\" > {}\n", stamp.display()))
        .unwrap();
    let mut perms = fs::metadata(&probe).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(&probe, perms).unwrap();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    isolated(&mut cmd, home.path(), root.path());
    cmd.env("SHELL", &probe).args(["get", "--look", "--silent", &url]).assert().success();

    let captured = fs::read_to_string(&stamp).unwrap();
    assert!(captured.contains("/"), "SCAP_LOOK should be host/owner/name; got {captured:?}");
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

// -- spawn accounting (V-3, AC-2/AC-2') -----------------------------------

/// Isolate `cmd` the way the spawn-ledger cases need it: no `SCAP_ROOT`, so
/// the root has to come out of the configuration, supplied by a
/// **url-section-free** `GIT_CONFIG_GLOBAL` fixture. With no `[scap "<url>"]`
/// section in sight, ADR-8 rule (c) answers `root_for_url` in process and
/// `git config --get-urlmatch` is never reached.
fn config_root(cmd: &mut AssertCommand, home: &Path, root: &Path) {
    let cfg = home.join("gitconfig");
    fs::write(&cfg, format!("[scap]\n\troot = {}\n", root.display())).unwrap();
    cmd.env_remove("SCAP_ROOT")
        .env_remove("SCAP_CONFIG_BACKEND")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("HOME", home)
        // Keep repository discovery out of whatever tree the test runner sits
        // in, so the local config of *this* checkout cannot leak in.
        .current_dir(home);
}

/// Put the recording wrapper first on `PATH`, so every `git` scap spawns
/// lands in [`RecordingGit::lines`] before reaching the real binary.
fn through_recording_git(cmd: &mut AssertCommand, git: &RecordingGit) {
    cmd.env("PATH", git.path_prepend());
    for (key, value) in git.env() {
        cmd.env(key, value);
    }
}

/// The 12 `file://` origins AC-2 is stated over, and their newline-joined
/// stdin form.
fn twelve_origins() -> (Vec<TempDir>, String) {
    let origins: Vec<TempDir> = (0..12).map(|_| init_bare_origin()).collect();
    let mut stdin = String::new();
    for o in &origins {
        stdin.push_str(&format!("file://{}\n", o.path().display()));
    }
    (origins, stdin)
}

fn assert_twelve_clones(root: &Path, origins: &[TempDir]) {
    for o in origins {
        let dest = scap_dest_for(root, o.path(), false);
        assert!(dest.join(".git").is_dir(), "clone missing for {}", o.path().display());
    }
}

#[test]
fn get_parallel_spawns_exactly_one_clone_per_target() {
    // AC-2, the A4 default: 12 targets, 12 `git` invocations, every one of
    // them the clone itself. Before W2.1 this log also held 36 `config`
    // lines -- three per target -- and before W2.3 the stale-tmp sweep added
    // a `kill` spawn per candidate.
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let git = RecordingGit::new();
    let (origins, stdin) = twelve_origins();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    config_root(&mut cmd, home.path(), root.path());
    through_recording_git(&mut cmd, &git);
    cmd.args(["get", "-P"]).write_stdin(stdin).assert().success();

    assert_twelve_clones(root.path(), &origins);

    let lines = git.lines();
    assert_eq!(lines.len(), 12, "AC-2: one spawn per target, got {lines:?}");
    for line in &lines {
        assert!(line.starts_with("clone"), "AC-2: only clones may be spawned, got {line:?}");
    }
}

#[test]
fn get_parallel_under_the_git_backend_adds_one_config_list() {
    // AC-2': the A3 fallback pays exactly one `git config --list` for the
    // whole process -- the snapshot is built once and every target reads it.
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let git = RecordingGit::new();
    let (origins, stdin) = twelve_origins();

    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    config_root(&mut cmd, home.path(), root.path());
    through_recording_git(&mut cmd, &git);
    cmd.env("SCAP_CONFIG_BACKEND", "git").args(["get", "-P"]).write_stdin(stdin).assert().success();

    assert_twelve_clones(root.path(), &origins);

    let lines = git.lines();
    let clones = lines.iter().filter(|l| l.starts_with("clone")).count();
    let config_lists = lines.iter().filter(|l| l.starts_with("config --list")).count();
    assert_eq!(clones, 12, "AC-2': one clone per target, got {lines:?}");
    assert!(config_lists <= 1, "AC-2': at most one snapshot spawn, got {lines:?}");
    assert_eq!(
        clones + config_lists,
        lines.len(),
        "AC-2': nothing but clones and the one snapshot spawn, got {lines:?}"
    );
}

#[test]
fn get_look_on_an_existing_repo_spawns_no_git() {
    // `--look` resolves its destination from the same snapshot the targets
    // used, so on an existing clone it runs no VCS operation and reads no
    // configuration out of process: the log stays empty.
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let origin = init_bare_origin();
    let url = format!("file://{}", origin.path().display());

    let mut seed = AssertCommand::cargo_bin("scap").unwrap();
    config_root(&mut seed, home.path(), root.path());
    seed.args(["get", "--silent", &url]).assert().success();

    let probe = home.path().join("shell.sh");
    fs::write(&probe, "#!/bin/sh\nexit 0\n").unwrap();
    let mut perms = fs::metadata(&probe).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(&probe, perms).unwrap();

    let git = RecordingGit::new();
    let mut cmd = AssertCommand::cargo_bin("scap").unwrap();
    config_root(&mut cmd, home.path(), root.path());
    through_recording_git(&mut cmd, &git);
    cmd.env("SHELL", &probe).args(["get", "--look", "--silent", &url]).assert().success();

    assert_eq!(git.lines(), Vec::<String>::new(), "--look on an existing repo must spawn nothing");
}
