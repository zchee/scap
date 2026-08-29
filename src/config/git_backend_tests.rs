//! Unit tests for the A3 backend's listing parser.

use std::path::PathBuf;

use super::*;

/// Build a `--list -z --show-origin` listing the way git frames it: one
/// NUL-terminated field per record, alternating origin and `key\nvalue`.
fn listing(records: &[(&str, Option<&str>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (key, value) in records {
        out.extend_from_slice(b"file:/fixture/gitconfig\0");
        out.extend_from_slice(key.as_bytes());
        if let Some(value) = value {
            out.push(b'\n');
            out.extend_from_slice(value.as_bytes());
        }
        out.push(0);
    }
    out
}

#[test]
fn entries_pairs_origins_with_keys_and_keeps_valueless_keys() {
    let bytes = listing(&[
        ("scap.root", Some("/a")),
        ("scap.completeuser", None),
        ("scap.https://example.com/.root", Some("/custom")),
    ]);

    let parsed = entries(&bytes);

    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].0, "scap.root");
    assert_eq!(parsed[0].1.map(ToString::to_string), Some("/a".to_owned()));
    assert_eq!(parsed[1].0, "scap.completeuser");
    assert!(parsed[1].1.is_none(), "a valueless key carries no newline");
    assert_eq!(parsed[2].0, "scap.https://example.com/.root");
}

#[test]
fn entries_tolerates_an_empty_listing_and_a_trailing_field() {
    assert!(entries(b"").is_empty());
    assert!(entries(b"file:/x\0").is_empty(), "an unpaired origin yields nothing");
}

#[test]
fn both_backends_share_one_truthiness_helper() {
    // The A3 backend must not have a boolean table of its own: `git_boolean`
    // is the single definition both backends read through.
    assert!(git_boolean(None), "a valueless key is true");
    for truthy in ["1", "true", "TRUE", "yes", "on", "42", "-1"] {
        assert!(git_boolean(Some(truthy.into())), "value: {truthy}");
    }
    for falsey in ["", "0", "false", "FALSE", "no", "off"] {
        assert!(!git_boolean(Some(falsey.into())), "value: {falsey}");
    }
    for invalid in ["one", "zero", "nil", "sure", "  true  ", "1 k", "0x", "1z"] {
        assert!(!git_boolean(Some(invalid.into())), "an invalid boolean is false: {invalid}");
    }
    // git's integer syntax: `strtoimax` base 0 plus a k/m/g unit suffix.
    for spelling in ["0x1", "1k", "1K", "1m", "1g", "010", "-2", "+3", " 7"] {
        assert!(git_boolean(Some(spelling.into())), "git reads this as a true integer: {spelling}");
    }
    for zero in ["0x0", "0k", "00", "-0"] {
        assert!(!git_boolean(Some(zero.into())), "a zero integer is false: {zero}");
    }
}

#[test]
fn from_listing_splits_plain_keys_from_url_scoped_roots() {
    let bytes = listing(&[
        ("scap.root", Some("/a")),
        ("scap.root", Some("/b")),
        ("scap.user", Some("  zchee  ")),
        ("scap.completeuser", None),
        ("scap.listexclude", Some("x/y")),
        ("scap.listcache", Some("true")),
        ("scap.https://example.com/.root", Some("/custom")),
        ("scap.https://example.com/.other", Some("ignored")),
        ("core.editor", Some("vi")),
    ]);

    let snapshot = from_listing(&bytes, &Env::default(), Reason::EnvOverride);

    assert_eq!(snapshot.roots(), [PathBuf::from("/a"), PathBuf::from("/b")]);
    assert_eq!(snapshot.url_scoped_roots(), [PathBuf::from("/custom")]);
    assert_eq!(snapshot.user(), Some("zchee"));
    assert!(snapshot.complete_user());
    assert_eq!(snapshot.list_exclude(), ["x/y".to_owned()]);
    assert!(snapshot.list_cache());
    assert_eq!(snapshot.backend(), Backend::Git);
    assert_eq!(snapshot.reason(), Reason::EnvOverride);
}

#[test]
fn load_reads_a_real_gitconfig_through_a_real_git() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let global = tmp.path().join("gitconfig");
    std::fs::write(&global, "[scap]\n\troot = /from-git-backend\n\tcompleteUser = yes\n")
        .expect("write gitconfig");

    let env = Env {
        home: Some(tmp.path().to_path_buf()),
        git_config_global: Some(global),
        git_config_nosystem: Some("1".into()),
        cwd: Some(tmp.path().to_path_buf()),
        path: std::env::var_os("PATH"),
        ..Default::default()
    };

    let snapshot = load(&env, Reason::EnvOverride).expect("spawn git and parse its listing");

    assert_eq!(snapshot.roots(), [PathBuf::from("/from-git-backend")]);
    assert!(snapshot.complete_user());
}

#[test]
fn reason_text_names_the_trigger_in_the_git_required_error() {
    assert!(reason_text(Reason::EnvOverride).contains("SCAP_CONFIG_BACKEND"));
    assert!(reason_text(Reason::GitConfigCount).contains("GIT_CONFIG_COUNT"));
    assert!(reason_text(Reason::GitConfigParameters).contains("GIT_CONFIG_PARAMETERS"));
    assert!(reason_text(Reason::SystemProbeAmbiguous).contains("probe"));
    assert!(reason_text(Reason::IncludeifUnevaluated).contains("includeIf"));
}

#[test]
fn apply_env_mirrors_the_view_onto_the_child() {
    // Observable through git itself: the child must see only what `Env`
    // carries, so an isolated view produces an isolated listing.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let global = tmp.path().join("gitconfig");
    std::fs::write(&global, "[scap]\n\troot = /isolated\n").expect("write gitconfig");

    let env = Env {
        home: Some(tmp.path().to_path_buf()),
        git_config_global: Some(global),
        git_config_nosystem: Some("1".into()),
        cwd: Some(tmp.path().to_path_buf()),
        path: std::env::var_os("PATH"),
        ..Default::default()
    };

    let program = sources::resolve_git_program(&env).expect("git");
    let mut command = Command::new(program);
    command.args(["config", "--get-all", "scap.root"]);
    apply_env(&mut command, &env);
    let output = command.output().expect("run git config");

    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "/isolated");
}
