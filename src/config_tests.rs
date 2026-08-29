use std::io::Write;
use std::path::Path;

use serial_test::serial;
use tempfile::TempDir;

use super::*;

struct EnvGuard {
    keys: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _tmp: TempDir,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in self.keys.drain(..) {
            match v {
                Some(val) => set_env(k, &val),
                None => unset_env(k),
            }
        }
    }
}

#[expect(unsafe_code, reason = "test-only env mutation, removed in W2.1 by the ADR-8 Env view")]
fn set_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: every test that reaches this helper is tagged `#[serial]`, so
    // `serial_test` serialises them against one another and no other thread
    // reads or writes the environment while this call runs.
    unsafe { std::env::set_var(key, value) };
}

#[expect(unsafe_code, reason = "test-only env mutation, removed in W2.1 by the ADR-8 Env view")]
fn unset_env(key: &str) {
    // SAFETY: as in `set_env` -- `#[serial]` serialises every caller, so this
    // is the only thread touching the environment for the duration.
    unsafe { std::env::remove_var(key) };
}

fn setup(contents: &str) -> EnvGuard {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = tmp.path().join("gitconfig");
    std::fs::File::create(&cfg)
        .expect("create gitconfig")
        .write_all(contents.as_bytes())
        .expect("write gitconfig");

    let saved = vec![
        ("GIT_CONFIG_NOSYSTEM", std::env::var_os("GIT_CONFIG_NOSYSTEM")),
        ("GIT_CONFIG_GLOBAL", std::env::var_os("GIT_CONFIG_GLOBAL")),
        ("SCAP_ROOT", std::env::var_os("SCAP_ROOT")),
        ("HOME", std::env::var_os("HOME")),
        ("XDG_CONFIG_HOME", std::env::var_os("XDG_CONFIG_HOME")),
    ];

    set_env("GIT_CONFIG_NOSYSTEM", "1");
    set_env("GIT_CONFIG_GLOBAL", &cfg);
    unset_env("SCAP_ROOT");
    set_env("HOME", tmp.path());
    set_env("XDG_CONFIG_HOME", tmp.path().join("xdg"));

    EnvGuard { keys: saved, _tmp: tmp }
}

fn pb(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
#[serial]
fn resolve_roots_uses_scap_root_env_when_set() {
    let _g = setup("");
    set_env("SCAP_ROOT", "/p/one:/p/two");
    let got = resolve_roots(false).unwrap();
    assert_eq!(got, vec![pb("/p/one"), pb("/p/two")]);
    let got_all = resolve_roots(true).unwrap();
    assert_eq!(got_all, vec![pb("/p/one"), pb("/p/two")]);
}

#[test]
#[serial]
fn resolve_roots_reverses_multi_root_from_gitconfig() {
    let _g = setup("[scap]\n\troot = /a\n\troot = /b\n\troot = /c\n");
    let got = resolve_roots(false).unwrap();
    assert_eq!(got, vec![pb("/c"), pb("/b"), pb("/a")]);
}

#[test]
#[serial]
fn resolve_roots_falls_back_to_home_scap() {
    let g = setup("");
    let expected = Path::new(&std::env::var_os("HOME").unwrap()).join("scap");
    let got = resolve_roots(false).unwrap();
    assert_eq!(got, vec![expected]);
    drop(g);
}

#[test]
#[serial]
fn resolve_roots_all_appends_urlmatch_roots() {
    let _g =
        setup("[scap]\n\troot = /default\n[scap \"https://example.com/\"]\n\troot = /custom\n");
    let no_all = resolve_roots(false).unwrap();
    assert_eq!(no_all, vec![pb("/default")]);
    let all = resolve_roots(true).unwrap();
    assert!(all.contains(&pb("/default")), "missing default in {all:?}");
    assert!(all.contains(&pb("/custom")), "missing custom in {all:?}");
}

#[test]
#[serial]
fn resolve_roots_dedups() {
    let _g = setup("[scap]\n\troot = /same\n\troot = /same\n");
    let got = resolve_roots(false).unwrap();
    assert_eq!(got, vec![pb("/same")]);
}

#[test]
#[serial]
fn root_for_url_uses_scap_root_env_first() {
    let _g = setup("[scap]\n\troot = /default\n");
    set_env("SCAP_ROOT", "/env-first:/env-second");
    let got = root_for_url("https://github.com/foo/bar").unwrap();
    assert_eq!(got, pb("/env-first"));
}

#[test]
#[serial]
fn root_for_url_consults_urlmatch_first() {
    let _g = setup(concat!(
        "[scap]\n\troot = /default\n",
        "[scap \"https://special.example.com/\"]\n\troot = /special\n",
    ));
    let special = root_for_url("https://special.example.com/foo/bar").unwrap();
    assert_eq!(special, pb("/special"));
    let other = root_for_url("https://other.example.com/foo/bar").unwrap();
    assert_eq!(other, pb("/default"));
}

#[test]
#[serial]
fn root_for_url_skips_urlmatch_for_codecommit() {
    let _g =
        setup("[scap]\n\troot = /default\n[scap \"codecommit\"]\n\troot = /should-be-ignored\n");
    let got = root_for_url("codecommit::us-east-1://my-repo").unwrap();
    assert_eq!(got, pb("/default"));
}

#[test]
#[serial]
fn scap_user_returns_value_when_set() {
    let _g = setup("[scap]\n\tuser = motemen\n");
    assert_eq!(scap_user().unwrap(), Some("motemen".to_owned()));
}

#[test]
#[serial]
fn scap_user_returns_none_when_unset() {
    let _g = setup("");
    assert_eq!(scap_user().unwrap(), None);
}

#[test]
#[serial]
fn scap_complete_user_parses_bool() {
    let _g = setup("[scap]\n\tcompleteUser = true\n");
    assert!(scap_complete_user().unwrap());
}

#[test]
#[serial]
fn scap_complete_user_defaults_false() {
    let _g = setup("");
    assert!(!scap_complete_user().unwrap());
}

#[test]
fn clean_path_normalizes_parent_and_current() {
    assert_eq!(clean_path(Path::new("/a/b/../c")), pb("/a/c"));
    assert_eq!(clean_path(Path::new("/a/./b")), pb("/a/b"));
    assert_eq!(clean_path(Path::new("./relative")), pb("relative"));
}

#[test]
#[serial]
fn git_config_get_path_returns_the_single_value_for_a_key() {
    let _g = setup("[scap]\n\troot = /p/one\n\tuser = zchee\n");

    assert_eq!(git_config_get_path("scap.root").unwrap(), Some("/p/one".to_owned()));
    assert_eq!(git_config_get_path("scap.user").unwrap(), Some("zchee".to_owned()));
}

#[test]
#[serial]
fn git_config_get_path_expands_a_leading_tilde_against_home() {
    // `--path` is not decoration: it is what makes `~/src` in a gitconfig mean
    // the same directory to scap as it does to git. `setup()` points HOME at
    // the temp dir, so the expansion is checked against a known value.
    let _g = setup("[scap]\n\troot = ~/nested\n");
    let home = std::env::var("HOME").expect("HOME set by setup()");

    let got = git_config_get_path("scap.root").unwrap().expect("key is present");
    assert_eq!(got, format!("{home}/nested"));
}

#[test]
#[serial]
fn git_config_get_path_returns_none_for_an_absent_key() {
    let _g = setup("[scap]\n\troot = /p/one\n");

    assert_eq!(git_config_get_path("scap.definitelyAbsent").unwrap(), None);
}

#[test]
#[serial]
fn git_config_get_all_path_returns_every_value_in_file_order() {
    // File order, not the reversed order `resolve_roots` applies afterwards:
    // the reversal is that caller's rule, not this accessor's.
    let _g = setup("[scap]\n\troot = /a\n\troot = /b\n\troot = /c\n");

    assert_eq!(
        git_config_get_all_path("scap.root").unwrap(),
        vec!["/a".to_owned(), "/b".to_owned(), "/c".to_owned()]
    );
}

#[test]
#[serial]
fn git_config_get_all_path_expands_each_value_and_drops_blank_lines() {
    let _g = setup("[scap]\n\troot = ~/one\n\troot = /two\n");
    let home = std::env::var("HOME").expect("HOME set by setup()");

    let got = git_config_get_all_path("scap.root").unwrap();

    assert_eq!(got, vec![format!("{home}/one"), "/two".to_owned()]);
    assert!(got.iter().all(|v| !v.is_empty()), "blank lines must be filtered out: {got:?}");
}

#[test]
#[serial]
fn git_config_get_all_path_returns_an_empty_vec_for_an_absent_key() {
    let _g = setup("[scap]\n\troot = /a\n");

    assert!(git_config_get_all_path("scap.definitelyAbsent").unwrap().is_empty());
}
