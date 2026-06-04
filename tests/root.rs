use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

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
        Self {
            home_path: home.path().to_path_buf(),
            _home: home,
            cfg_path,
            _cfg: cfg,
        }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("scap").unwrap();
        c.env_remove("SCAP_ROOT")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.cfg_path)
            .env("HOME", &self.home_path);
        c
    }
}

#[test]
fn root_prints_first_ghq_root_env_entry() {
    let f = Fixture::new("");
    f.cmd()
        .arg("root")
        .env("SCAP_ROOT", "/path1:/path2")
        .assert()
        .success()
        .stdout(predicate::str::diff("/path1\n"));
}

#[test]
fn root_all_prints_every_ghq_root_env_entry() {
    let f = Fixture::new("");
    f.cmd()
        .arg("root")
        .arg("--all")
        .env("SCAP_ROOT", "/path1:/path2")
        .assert()
        .success()
        .stdout(predicate::str::diff("/path1\n/path2\n"));
}

#[test]
fn root_reverses_multi_root_from_gitconfig() {
    let f = Fixture::new("[scap]\n\troot = /a\n\troot = /b\n\troot = /c\n");
    f.cmd()
        .arg("root")
        .assert()
        .success()
        .stdout(predicate::str::diff("/c\n"));
}

#[test]
fn root_all_reverses_multi_root_from_gitconfig() {
    let f = Fixture::new("[scap]\n\troot = /a\n\troot = /b\n\troot = /c\n");
    f.cmd()
        .arg("root")
        .arg("--all")
        .assert()
        .success()
        .stdout(predicate::str::diff("/c\n/b\n/a\n"));
}

#[test]
fn root_all_appends_urlmatch_roots_when_ghq_root_unset() {
    let cfg = "[scap]\n\troot = /default\n\
              [scap \"https://example.com\"]\n\troot = /custom\n";
    let f = Fixture::new(cfg);

    f.cmd()
        .arg("root")
        .assert()
        .success()
        .stdout(predicate::str::diff("/default\n"));

    f.cmd()
        .arg("root")
        .arg("--all")
        .assert()
        .success()
        .stdout(predicate::str::contains("/default\n"))
        .stdout(predicate::str::contains("/custom\n"));
}

#[test]
fn root_falls_back_to_home_ghq_when_unconfigured() {
    let f = Fixture::new("");
    let expected = format!("{}/scap\n", f.home_path.display());
    f.cmd()
        .arg("root")
        .assert()
        .success()
        .stdout(predicate::str::diff(expected));
}
