use std::fs;

use tempfile::TempDir;

use super::*;

#[test]
fn is_not_exist_or_empty_reports_true_for_a_missing_path() {
    let tmp = TempDir::new().expect("tempdir");
    assert!(
        is_not_exist_or_empty(&tmp.path().join("absent")).expect("missing path is not an error")
    );
}

#[test]
fn is_not_exist_or_empty_reports_true_for_an_empty_directory() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path().join("empty");
    fs::create_dir(&dir).expect("mkdir");
    assert!(is_not_exist_or_empty(&dir).expect("empty dir is not an error"));
}

#[test]
fn is_not_exist_or_empty_reports_false_for_a_directory_holding_anything() {
    let tmp = TempDir::new().expect("tempdir");

    let with_file = tmp.path().join("with-file");
    fs::create_dir(&with_file).expect("mkdir");
    fs::write(with_file.join("a"), b"x").expect("write");
    assert!(!is_not_exist_or_empty(&with_file).expect("populated dir is not an error"));

    // A single dotfile counts: read_dir does not hide them.
    let with_dotfile = tmp.path().join("with-dotfile");
    fs::create_dir(&with_dotfile).expect("mkdir");
    fs::write(with_dotfile.join(".hidden"), b"x").expect("write");
    assert!(!is_not_exist_or_empty(&with_dotfile).expect("dotfile counts as an entry"));

    // An empty subdirectory counts too.
    let with_subdir = tmp.path().join("with-subdir");
    fs::create_dir_all(with_subdir.join("child")).expect("mkdir -p");
    assert!(!is_not_exist_or_empty(&with_subdir).expect("subdirectory counts as an entry"));
}

#[test]
fn is_not_exist_or_empty_errors_when_the_path_is_a_file() {
    let tmp = TempDir::new().expect("tempdir");
    let file = tmp.path().join("a-file");
    fs::write(&file, b"x").expect("write");

    let err = is_not_exist_or_empty(&file).expect_err("a file must be an error, not `true`");
    assert!(err.to_string().contains("exists but is not a directory"), "unexpected error: {err}");
    assert!(err.to_string().contains("a-file"), "error must name the path: {err}");
}
