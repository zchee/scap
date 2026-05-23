use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_lists_all_subcommands() {
    Command::cargo_bin("scap")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("rm"))
        .stdout(predicate::str::contains("root"))
        .stdout(predicate::str::contains("create"));
}

#[test]
fn clone_is_alias_for_get() {
    Command::cargo_bin("scap")
        .unwrap()
        .args(["clone", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--update").or(predicate::str::contains("-u")));
}

#[test]
fn get_help_lists_documented_flags() {
    let assert = Command::cargo_bin("scap")
        .unwrap()
        .args(["get", "--help"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in [
        "--update",
        "--shallow",
        "--look",
        "--branch",
        "--bare",
        "--partial",
        "--parallel",
        "--silent",
        "--no-recursive",
    ] {
        assert!(
            stdout.contains(flag),
            "scap get --help missing flag: {flag}"
        );
    }
}

#[test]
fn list_help_lists_documented_flags() {
    let assert = Command::cargo_bin("scap")
        .unwrap()
        .args(["list", "--help"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in ["--exact", "--full-path", "--unique", "--bare", "--vcs"] {
        assert!(
            stdout.contains(flag),
            "scap list --help missing flag: {flag}"
        );
    }
}

#[test]
fn rm_help_lists_documented_flags() {
    let assert = Command::cargo_bin("scap")
        .unwrap()
        .args(["rm", "--help"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in ["--dry-run", "--bare"] {
        assert!(stdout.contains(flag), "scap rm --help missing flag: {flag}");
    }
}

#[test]
fn create_help_lists_documented_flags() {
    let assert = Command::cargo_bin("scap")
        .unwrap()
        .args(["create", "--help"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in ["--vcs", "--bare"] {
        assert!(
            stdout.contains(flag),
            "scap create --help missing flag: {flag}"
        );
    }
}

#[test]
fn root_help_lists_documented_flags() {
    let assert = Command::cargo_bin("scap")
        .unwrap()
        .args(["root", "--help"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("--all"), "scap root --help missing --all");
}
