//! Unit tests for the in-process configuration loader (ADR-8).
//!
//! Every case drives [`load`] through an injected [`Env`], so nothing here
//! mutates the process environment: that is what let W2.1 delete
//! `serial_test` and the last two `unsafe` blocks in the tree. Fixtures are
//! real files in a real temp directory, and the expectations that need an
//! oracle run the real `git`.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;

/// A temp tree plus the [`Env`] view that points at it.
struct Fixture {
    _tmp: TempDir,
    /// The temp directory's physical spelling. On macOS `TempDir` hands back
    /// a `/var/...` path whose real location is `/private/var/...`, and
    /// `resolve_roots` canonicalises, so the fixture works in physical paths
    /// throughout and the expectations stay literal.
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize the tempdir");
        std::fs::create_dir_all(root.join("home")).expect("mkdir home");
        std::fs::create_dir_all(root.join("cwd")).expect("mkdir cwd");
        Self { _tmp: tmp, root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn cwd(&self) -> PathBuf {
        self.root.join("cwd")
    }

    /// Write `contents` to `rel` inside the fixture, creating parents.
    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p fixture parent");
        }
        std::fs::write(&path, contents).expect("write fixture file");
        path
    }

    /// An isolated view: no system config (the probe list is empty), `HOME`
    /// and the working directory inside the fixture, and the real `PATH` so
    /// the A3 backend can still find a real `git`.
    fn env(&self) -> Env {
        Env {
            home: Some(self.home()),
            cwd: Some(self.cwd()),
            path: std::env::var_os("PATH"),
            ..Default::default()
        }
    }

    /// A real repository at `cwd`, created by the real `git`, then given
    /// `local_config` as its repository-level configuration.
    fn init_repo(&self, local_config: &str) {
        let out = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(self.cwd())
            .output()
            .expect("run git init");
        assert!(out.status.success(), "git init failed: {out:?}");
        self.write("cwd/.git/config", local_config);
    }
}

fn load_ok(env: &Env) -> ConfigSnapshot {
    load(env).expect("load the fixture configuration")
}

/// Ask the real `git` the same question, through the same [`Env`].
fn git_path_values(env: &Env, key: &str) -> Vec<PathBuf> {
    let program = sources::resolve_git_program(env).expect("a real git on PATH");
    let mut command = std::process::Command::new(program);
    command.args(["config", "--path", "--get-all", key]);
    git_backend::apply_env(&mut command, env);
    let output = command.output().expect("run git config");
    String::from_utf8(output.stdout)
        .expect("utf-8 git output")
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn pb(s: &str) -> PathBuf {
    PathBuf::from(s)
}

// -- source enumeration and precedence -----------------------------------

#[test]
fn load_reads_system_xdg_user_and_local_in_git_order() {
    let f = Fixture::new();
    let system = f.write("system/gitconfig", "[scap]\n\troot = /from-system\n");
    f.write("home/.config/git/config", "[scap]\n\troot = /from-xdg\n");
    f.write("home/.gitconfig", "[scap]\n\troot = /from-user\n");
    f.init_repo("[scap]\n\troot = /from-local\n");

    let env = Env { git_config_system: Some(system), ..f.env() };
    let snapshot = load_ok(&env);

    assert_eq!(
        snapshot.roots(),
        [pb("/from-system"), pb("/from-xdg"), pb("/from-user"), pb("/from-local")],
        "sources must be read in git's own precedence order"
    );
    assert_eq!(snapshot.roots(), git_path_values(&env, "scap.root"));
}

#[test]
fn git_config_global_replaces_both_xdg_and_user_files() {
    let f = Fixture::new();
    f.write("home/.config/git/config", "[scap]\n\troot = /from-xdg\n");
    f.write("home/.gitconfig", "[scap]\n\troot = /from-user\n");
    let global = f.write("global/gitconfig", "[scap]\n\troot = /from-global\n");

    let env = Env { git_config_global: Some(global), ..f.env() };
    let snapshot = load_ok(&env);

    assert_eq!(snapshot.roots(), [pb("/from-global")]);
    assert_eq!(snapshot.roots(), git_path_values(&env, "scap.root"));
}

#[test]
fn an_empty_git_config_global_suppresses_the_global_level() {
    // An empty value is a real, unopenable path to git, not "unset": the
    // XDG and user files stay suppressed rather than coming back.
    let f = Fixture::new();
    f.write("home/.config/git/config", "[scap]\n\troot = /from-xdg\n");
    f.write("home/.gitconfig", "[scap]\n\troot = /from-user\n");

    let env = Env { git_config_global: Some(PathBuf::new()), ..f.env() };

    assert!(git_path_values(&env, "scap.root").is_empty(), "the oracle must see nothing");
    assert!(load_ok(&env).roots().is_empty(), "and neither must scap");
}

#[test]
fn one_global_file_reachable_by_two_paths_is_parsed_once() {
    // `$XDG_CONFIG_HOME/git/config` is a symlink to `~/.gitconfig`, so both
    // global-level candidates name the same file. Without the canonical-path
    // dedup its single `scap.root` line would appear twice.
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = /only-once\n");
    std::fs::create_dir_all(f.path().join("home/.config/git")).expect("mkdir xdg");
    std::os::unix::fs::symlink(
        f.home().join(".gitconfig"),
        f.path().join("home/.config/git/config"),
    )
    .expect("symlink xdg config at the user file");

    let snapshot = load_ok(&f.env());

    assert_eq!(snapshot.roots(), [pb("/only-once")]);
}

#[test]
fn include_path_is_followed() {
    let f = Fixture::new();
    f.write("home/included.gitconfig", "[scap]\n\troot = /from-include\n");
    f.write(
        "home/.gitconfig",
        "[scap]\n\troot = /from-user\n[include]\n\tpath = included.gitconfig\n",
    );

    let env = f.env();
    let snapshot = load_ok(&env);

    assert_eq!(snapshot.roots(), [pb("/from-user"), pb("/from-include")]);
    assert_eq!(snapshot.roots(), git_path_values(&env, "scap.root"));
}

#[test]
fn include_if_gitdir_condition_is_evaluated_against_the_discovered_repository() {
    let f = Fixture::new();
    f.init_repo("");
    f.write("home/matching.gitconfig", "[scap]\n\troot = /matched\n");
    f.write("home/other.gitconfig", "[scap]\n\troot = /not-matched\n");
    let cwd = f.cwd();
    f.write(
        "home/.gitconfig",
        &format!(
            "[includeIf \"gitdir:{}/\"]\n\tpath = matching.gitconfig\n\
             [includeIf \"gitdir:/definitely/elsewhere/\"]\n\tpath = other.gitconfig\n",
            cwd.display()
        ),
    );

    let snapshot = load_ok(&f.env());

    assert_eq!(snapshot.roots(), [pb("/matched")]);
}

// -- `--path` interpolation ----------------------------------------------

#[test]
fn path_values_expand_a_leading_tilde_against_home() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = ~/nested\n");

    let env = f.env();
    let snapshot = load_ok(&env);

    assert_eq!(snapshot.roots(), [f.home().join("nested")]);
    assert_eq!(snapshot.roots(), git_path_values(&env, "scap.root"));
}

#[test]
fn path_values_expand_tilde_user_the_way_git_does() {
    // `~root/` resolves through the password database on every unix, so the
    // real `git` is the oracle rather than a hard-coded path.
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = ~root/src\n");

    let env = f.env();
    let expected = git_path_values(&env, "scap.root");
    assert!(!expected.is_empty(), "git must resolve ~root/ for this oracle to mean anything");

    assert_eq!(load_ok(&env).roots(), expected);
}

#[test]
fn quoted_and_continued_values_match_git() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = \"/quoted path/one\"\n\troot = /two\\\n/three\n");

    let env = f.env();
    let expected = git_path_values(&env, "scap.root");

    assert_eq!(load_ok(&env).roots(), expected);
}

// -- values ---------------------------------------------------------------

#[test]
fn user_is_read_and_trimmed() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\tuser = motemen\n");
    assert_eq!(load_ok(&f.env()).user(), Some("motemen"));

    let g = Fixture::new();
    g.write("home/.gitconfig", "");
    assert_eq!(load_ok(&g.env()).user(), None);
}

#[test]
fn complete_user_accepts_every_git_bool_spelling_including_a_valueless_key() {
    for (spelling, expected) in [
        ("completeUser = true", true),
        ("completeUser = yes", true),
        ("completeUser = on", true),
        ("completeUser = 1", true),
        ("completeUser = false", false),
        ("completeUser = no", false),
        ("completeUser = off", false),
        ("completeUser = 0", false),
        // A valueless key is `true` -- what `git config --bool` prints.
        ("completeUser", true),
    ] {
        let f = Fixture::new();
        f.write("home/.gitconfig", &format!("[scap]\n\t{spelling}\n"));
        assert_eq!(load_ok(&f.env()).complete_user(), expected, "spelling: {spelling}");
    }

    let unset = Fixture::new();
    unset.write("home/.gitconfig", "");
    assert!(!load_ok(&unset.env()).complete_user(), "an absent key is false");

    // git exits fatally on a boolean it cannot parse. scap cannot exit over
    // a key it merely happens to read, so it takes the conservative value --
    // in both backends, which is what makes them interchangeable.
    for invalid in ["one", "zero", "nil", "sure", "1 k", "0x"] {
        let f = Fixture::new();
        f.write("home/.gitconfig", &format!("[scap]\n\tcompleteUser = {invalid}\n"));
        assert!(!load_ok(&f.env()).complete_user(), "in process: {invalid}");
        let via_git = load_ok(&Env { scap_config_backend: Some("git".into()), ..f.env() });
        assert!(!via_git.complete_user(), "via git: {invalid}");
    }

    // git reads the integer with `strtoimax` base 0 plus a `k`/`m`/`g` unit
    // suffix, so these are true booleans to it and a plain decimal parse
    // would wrongly call them invalid.
    for spelling in ["0x1", "1k", "-2", "010", "+3", "1M", "1g"] {
        let f = Fixture::new();
        f.write("home/.gitconfig", &format!("[scap]\n\tcompleteUser = {spelling}\n"));
        assert!(load_ok(&f.env()).complete_user(), "in process: {spelling}");
        let via_git = load_ok(&Env { scap_config_backend: Some("git".into()), ..f.env() });
        assert!(via_git.complete_user(), "via git: {spelling}");
    }
    for zero in ["0x0", "0k", "00"] {
        let f = Fixture::new();
        f.write("home/.gitconfig", &format!("[scap]\n\tcompleteUser = {zero}\n"));
        assert!(!load_ok(&f.env()).complete_user(), "in process: {zero}");
    }
}

#[test]
fn boolean_of_separates_a_valueless_key_from_an_empty_one_and_takes_the_last() {
    let f = Fixture::new();
    f.write(
        "home/.gitconfig",
        "[scap]\n\tcompleteUser = true\n[other]\n\tflag = true\n\
         [scap]\n\tcompleteUser = false\n\tlistCache\n\tuser = x\n\
         [scap \"https://example.com/\"]\n\tcompleteUser = true\n",
    );
    let list = sources::enumerate(&f.env());
    let file = parse(&list, &f.env()).expect("parse").expect("a file exists");

    assert_eq!(
        boolean_of(&file, "scap", "completeUser"),
        Some(false),
        "the last plain occurrence wins, and a subsection is not a plain key"
    );
    assert_eq!(boolean_of(&file, "scap", "listCache"), Some(true), "a valueless key is true");
    assert_eq!(boolean_of(&file, "scap", "absent"), None);
    assert_eq!(boolean_of(&file, "other", "flag"), Some(true), "the section name is a parameter");

    let empty = Fixture::new();
    empty.write("home/.gitconfig", "[scap]\n\tcompleteUser =\n");
    let list = sources::enumerate(&empty.env());
    let file = parse(&list, &empty.env()).expect("parse").expect("a file exists");
    assert_eq!(
        boolean_of(&file, "scap", "completeUser"),
        Some(false),
        "an empty value is false, unlike a valueless key"
    );
}

#[test]
fn list_exclude_and_list_cache_are_read_from_flat_keys() {
    let f = Fixture::new();
    f.write(
        "home/.gitconfig",
        "[scap]\n\tlistExclude = a/b\n\tlistExclude = c\n\tlistCache = true\n",
    );

    let snapshot = load_ok(&f.env());

    assert_eq!(snapshot.list_exclude(), ["a/b".to_owned(), "c".to_owned()]);
    assert!(snapshot.list_cache());
}

#[test]
fn scap_list_exclude_replaces_the_configured_patterns() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\tlistExclude = from-config\n");

    // Set and non-empty: the variable is the whole exclusion set, the way
    // `SCAP_ROOT` is the whole root list. Empty segments are dropped so a
    // stray separator cannot introduce a pattern that matches nothing.
    let mut env = f.env();
    env.scap_list_exclude = Some("a/b::c".into());
    assert_eq!(load_ok(&env).list_exclude(), ["a/b".to_owned(), "c".to_owned()]);

    // Empty counts as unset, again as for `SCAP_ROOT`, so it is not a way
    // to suppress a configured pattern.
    let mut empty = f.env();
    empty.scap_list_exclude = Some("".into());
    assert_eq!(load_ok(&empty).list_exclude(), ["from-config".to_owned()]);

    assert_eq!(load_ok(&f.env()).list_exclude(), ["from-config".to_owned()]);

    // The two backends must agree: the override is folded in where the
    // snapshot is built, not in one parser.
    let mut via_git = f.env();
    via_git.scap_list_exclude = Some("a/b::c".into());
    via_git.scap_config_backend = Some("git".into());
    assert_eq!(load_ok(&via_git).list_exclude(), ["a/b".to_owned(), "c".to_owned()]);
}

#[test]
fn list_exclude_folds_one_trailing_slash() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\tlistExclude = node_modules/\n\tlistExclude = /\n");

    // `.gitignore` spells a directory with a trailing slash and every
    // exclusion candidate is a directory, so the suffix carries nothing and
    // is folded away rather than silently matching nothing. A pattern that
    // is only that slash is dropped.
    assert_eq!(load_ok(&f.env()).list_exclude(), ["node_modules".to_owned()]);

    let mut via_env = f.env();
    via_env.scap_list_exclude = Some("node_modules/:/:keep".into());
    assert_eq!(load_ok(&via_env).list_exclude(), ["node_modules".to_owned(), "keep".to_owned()]);

    // Only one slash goes: `foo//` still names an empty component.
    let mut doubled = f.env();
    doubled.scap_list_exclude = Some("foo//".into());
    assert_eq!(load_ok(&doubled).list_exclude(), ["foo/".to_owned()]);
}

#[test]
fn has_url_sections_and_url_scoped_roots_report_scap_subsections() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = /plain\n");
    let plain = load_ok(&f.env());
    assert!(!plain.has_url_sections());
    assert!(plain.url_scoped_roots().is_empty());

    let g = Fixture::new();
    g.write(
        "home/.gitconfig",
        "[scap]\n\troot = /plain\n\
         [scap \"https://example.com/\"]\n\troot = /custom\n\
         [scap \"https://other.example.com/\"]\n\troot = /other\n",
    );
    let scoped = load_ok(&g.env());
    assert!(scoped.has_url_sections());
    assert_eq!(scoped.url_scoped_roots(), [pb("/custom"), pb("/other")]);
    assert_eq!(scoped.roots(), [pb("/plain")], "a subsection root is not a plain root");
}

#[test]
fn backend_and_reason_describe_the_in_process_path() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = /plain\n");
    let plain = load_ok(&f.env());
    assert_eq!(plain.backend(), Backend::InProcess);
    assert_eq!(plain.reason(), Reason::InProcess);
    assert_eq!(plain.backend().to_string(), "in_process");

    let g = Fixture::new();
    g.write("home/.gitconfig", "[scap \"https://example.com/\"]\n\troot = /custom\n");
    let scoped = load_ok(&g.env());
    assert_eq!(scoped.backend(), Backend::InProcess, "url sections alone keep the snapshot local");
    assert_eq!(scoped.reason(), Reason::UrlSections);
    assert_eq!(scoped.reason().to_string(), "url_sections");
}

// -- spawn triggers -------------------------------------------------------

#[test]
fn needs_git_backend_fires_once_per_trigger() {
    let f = Fixture::new();
    let base = f.env();

    assert_eq!(needs_git_backend(&base, 0, false), None, "the common case stays in process");
    assert_eq!(needs_git_backend(&base, 1, false), None, "exactly one system file is unambiguous");

    let env = Env { scap_config_backend: Some("git".into()), ..base.clone() };
    assert_eq!(needs_git_backend(&env, 0, false), Some(Reason::EnvOverride));

    let env = Env { scap_config_backend: Some("gix".into()), ..base.clone() };
    assert_eq!(needs_git_backend(&env, 0, false), None, "only `git` selects the A3 backend");

    let env = Env { git_config_count: Some("1".into()), ..base.clone() };
    assert_eq!(needs_git_backend(&env, 0, false), Some(Reason::GitConfigCount));

    let env = Env { git_config_parameters: Some("'scap.root=/x'".into()), ..base.clone() };
    assert_eq!(needs_git_backend(&env, 0, false), Some(Reason::GitConfigParameters));

    let env = Env { git_config_count: Some("0".into()), ..base.clone() };
    assert_eq!(needs_git_backend(&env, 0, false), None, "a count of zero adds no keys");

    let env = Env { git_config_count: Some("".into()), ..base.clone() };
    assert_eq!(
        needs_git_backend(&env, 0, false),
        None,
        "git's strtoul reads an empty count as zero without an error"
    );

    for unparseable in ["abc", "-1"] {
        let env = Env { git_config_count: Some(unparseable.into()), ..base.clone() };
        assert_eq!(
            needs_git_backend(&env, 0, false),
            Some(Reason::GitConfigCount),
            "git would die on {unparseable:?}, so git must be the one to say so"
        );
    }

    assert_eq!(needs_git_backend(&base, 2, false), Some(Reason::SystemProbeAmbiguous));
    assert_eq!(needs_git_backend(&base, 0, true), Some(Reason::IncludeifUnevaluated));
}

#[test]
fn an_unevaluable_include_if_routes_the_snapshot_to_git() {
    for condition in ["onbranch:main", "hasconfig:remote.*.url:https://example.com/**"] {
        let f = Fixture::new();
        f.write("home/extra.gitconfig", "[scap]\n\troot = /from-include\n");
        f.write(
            "home/.gitconfig",
            &format!(
                "[scap]\n\troot = /plain\n[includeIf \"{condition}\"]\n\tpath = extra.gitconfig\n"
            ),
        );

        let snapshot = load_ok(&f.env());

        assert_eq!(snapshot.backend(), Backend::Git, "condition: {condition}");
        assert_eq!(snapshot.reason(), Reason::IncludeifUnevaluated);
        assert_eq!(snapshot.roots(), [pb("/plain")], "git is the parser of record here");
    }
}

#[test]
fn an_ambiguous_system_probe_routes_the_snapshot_to_git() {
    let f = Fixture::new();
    let first = f.write("etc-a/gitconfig", "[scap]\n\troot = /from-a\n");
    let second = f.write("etc-b/gitconfig", "[scap]\n\troot = /from-b\n");
    f.write("home/.gitconfig", "[scap]\n\troot = /from-user\n");

    let env = Env { system_probe_candidates: vec![first, second], ..f.env() };
    let snapshot = load_ok(&env);

    assert_eq!(snapshot.backend(), Backend::Git);
    assert_eq!(snapshot.reason(), Reason::SystemProbeAmbiguous);
    assert_eq!(snapshot.reason().to_string(), "system_probe_ambiguous");
}

#[test]
fn a_trigger_without_git_on_path_is_fatal_rather_than_a_silent_fallback() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = /plain\n");
    std::fs::create_dir_all(f.path().join("empty-bin")).expect("mkdir empty-bin");

    let env = Env {
        scap_config_backend: Some("git".into()),
        path: Some(f.path().join("empty-bin").into_os_string()),
        ..f.env()
    };

    let err = load(&env).expect_err("no git on PATH must not fall back to the in-process snapshot");
    assert!(
        matches!(err, ConfigError::GitRequired { reason } if reason.contains("SCAP_CONFIG_BACKEND")),
        "the error must name the trigger, got: {err}"
    );
}

#[test]
fn the_git_backend_reproduces_the_in_process_snapshot() {
    let f = Fixture::new();
    f.write(
        "home/.gitconfig",
        "[scap]\n\troot = /a\n\troot = ~/b\n\tuser = zchee\n\tcompleteUser\n\
         \tlistExclude = x/y\n\tlistCache = yes\n\
         [scap \"https://example.com/\"]\n\troot = /custom\n",
    );

    let in_process = load_ok(&f.env());
    let via_git = load_ok(&Env { scap_config_backend: Some("git".into()), ..f.env() });

    assert_eq!(via_git.backend(), Backend::Git);
    assert_eq!(via_git.roots(), in_process.roots());
    assert_eq!(via_git.url_scoped_roots(), in_process.url_scoped_roots());
    assert_eq!(via_git.user(), in_process.user());
    assert_eq!(via_git.complete_user(), in_process.complete_user());
    assert_eq!(via_git.list_exclude(), in_process.list_exclude());
    assert_eq!(via_git.list_cache(), in_process.list_cache());
}

// -- resolve_roots --------------------------------------------------------

#[test]
fn resolve_roots_reverses_multi_root_and_dedups() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = /a\n\troot = /b\n\troot = /c\n\troot = /a\n");

    let roots = load_ok(&f.env()).resolve_roots(false).expect("resolve");

    assert_eq!(roots, [pb("/a"), pb("/c"), pb("/b")]);
}

#[test]
fn resolve_roots_falls_back_to_home_scap() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "");

    let roots = load_ok(&f.env()).resolve_roots(false).expect("resolve");

    assert_eq!(roots, [f.home().join("scap")]);
}

#[test]
fn resolve_roots_uses_scap_root_env_when_set() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = /ignored\n");

    let env = Env { scap_root: Some("/p/one:/p/two".into()), ..f.env() };
    let snapshot = load_ok(&env);

    assert_eq!(snapshot.resolve_roots(false).expect("resolve"), [pb("/p/one"), pb("/p/two")]);
    assert_eq!(snapshot.resolve_roots(true).expect("resolve"), [pb("/p/one"), pb("/p/two")]);
}

#[test]
fn resolve_roots_all_appends_url_scoped_roots() {
    let f = Fixture::new();
    f.write(
        "home/.gitconfig",
        "[scap]\n\troot = /default\n[scap \"https://example.com/\"]\n\troot = /custom\n",
    );
    let snapshot = load_ok(&f.env());

    assert_eq!(snapshot.resolve_roots(false).expect("resolve"), [pb("/default")]);
    assert_eq!(snapshot.resolve_roots(true).expect("resolve"), [pb("/default"), pb("/custom")]);
}

// -- root_for_url ---------------------------------------------------------

#[test]
fn root_for_url_returns_the_last_plain_root_raw_when_no_url_section_exists() {
    // ADR-8 rule (c): `git config --path --get-urlmatch` prints the last
    // plain value through a symlinked component without resolving it, and
    // ghq uses that output raw. Routing through `resolve_roots` would
    // canonicalise `lnk` away.
    let f = Fixture::new();
    std::fs::create_dir_all(f.path().join("real/last")).expect("mkdir real/last");
    std::os::unix::fs::symlink(f.path().join("real"), f.path().join("lnk")).expect("symlink lnk");
    let via_link = f.path().join("lnk").join("last");
    f.write(
        "home/.gitconfig",
        &format!("[scap]\n\troot = /first\n\troot = {}\n", via_link.display()),
    );

    let env = f.env();
    let snapshot = load_ok(&env);
    let url = "https://github.com/x/y";

    assert_eq!(snapshot.root_for_url(url).expect("root_for_url"), via_link);
    assert_ne!(
        snapshot.resolve_roots(false).expect("resolve")[0],
        via_link,
        "the canonicalising path is what rule (c) must not take"
    );

    // The oracle: what git itself prints for the same question.
    let program = sources::resolve_git_program(&env).expect("git");
    let mut command = std::process::Command::new(program);
    command.args(["config", "--path", "--get-urlmatch", "scap.root", url]);
    git_backend::apply_env(&mut command, &env);
    let output = command.output().expect("run git config --get-urlmatch");
    assert!(output.status.success(), "urlmatch must succeed when scap.root is set");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        via_link.to_string_lossy(),
        "rule (c) must byte-match git's own urlmatch fallback"
    );
}

#[test]
fn root_for_url_uses_scap_root_env_first() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = /default\n");

    let env = Env { scap_root: Some("/env-first:/env-second".into()), ..f.env() };

    assert_eq!(
        load_ok(&env).root_for_url("https://github.com/foo/bar").expect("root"),
        pb("/env-first")
    );
}

#[test]
fn root_for_url_skips_urlmatch_for_codecommit_inputs() {
    // ADR-8 rule (b): a codecommit input skips urlmatch and then takes
    // ghq's primary root, which is *canonicalised* -- rule (e), not the raw
    // last plain value of rule (c). Two roots whose last entry traverses a
    // symlink is what makes the two rules distinguishable.
    let f = Fixture::new();
    std::fs::create_dir_all(f.path().join("real/last")).expect("mkdir real/last");
    std::os::unix::fs::symlink(f.path().join("real"), f.path().join("lnk")).expect("symlink lnk");
    let via_link = f.path().join("lnk").join("last");
    let resolved = std::fs::canonicalize(&via_link).expect("the symlink target exists");
    assert_ne!(resolved, via_link, "the fixture must actually traverse a symlink");
    f.write(
        "home/.gitconfig",
        &format!(
            "[scap]\n\troot = /first\n\troot = {}\n\
             [scap \"codecommit\"]\n\troot = /should-be-ignored\n",
            via_link.display()
        ),
    );

    let snapshot = load_ok(&f.env());

    // The first spelling is what every call site passes: `from_input`
    // normalises a codecommit target through `url::finalize_codecommit`
    // into `codecommit://<region>/<owner>/<name>`, whose authority holds a
    // `/` and which `is_codecommit_input` therefore rejects. The second is
    // the raw reference a caller that has not normalised would hold. Both
    // must take rule (b).
    for url in ["codecommit://us-east-1/codecommit/my-repo", "codecommit::us-east-1://my-repo"] {
        let got = snapshot.root_for_url(url).expect("root");
        assert_eq!(got, resolved, "rule (b) must land on rule (e) for {url}");
        assert_ne!(got, via_link, "rule (c)'s raw last value is not what ghq uses: {url}");
        assert_eq!(got, snapshot.resolve_roots(false).expect("resolve")[0], "{url}");
    }

    // A non-codecommit target still takes rule (c), raw, so the two rules
    // do not collapse into one behaviour.
    assert_eq!(snapshot.root_for_url("https://github.com/x/y").expect("root"), via_link);
}

#[test]
fn root_for_url_delegates_to_urlmatch_when_a_url_section_is_visible() {
    let f = Fixture::new();
    f.write(
        "home/.gitconfig",
        "[scap]\n\troot = /default\n\
         [scap \"https://special.example.com/\"]\n\troot = /special\n",
    );

    let snapshot = load_ok(&f.env());

    assert_eq!(
        snapshot.root_for_url("https://special.example.com/foo/bar").expect("root"),
        pb("/special")
    );
    assert_eq!(
        snapshot.root_for_url("https://other.example.com/foo/bar").expect("root"),
        pb("/default")
    );
}

#[test]
fn root_for_url_falls_back_to_the_home_root_when_nothing_is_configured() {
    let f = Fixture::new();
    f.write("home/.gitconfig", "");

    let snapshot = load_ok(&f.env());

    assert_eq!(
        snapshot.root_for_url("https://github.com/x/y").expect("root"),
        f.home().join("scap")
    );
}

// -- rule (d): the memoised urlmatch delegation (W2.2) --------------------

/// The six url-scoped section kinds git's `urlmatch.c` distinguishes:
/// an exact host, a host with a path prefix, a longer path prefix that must
/// beat the shorter one, a `*.` wildcard host, a scheme-specific section,
/// and a `user@host` one.
const SIX_SECTIONS: &str = "[scap]\n\troot = /plain-first\n\troot = /plain-last\n\
     [scap \"https://exact.example.com/\"]\n\troot = /r-exact\n\
     [scap \"https://pathy.example.com/team\"]\n\troot = /r-path-short\n\
     [scap \"https://pathy.example.com/team/deep\"]\n\troot = /r-path-long\n\
     [scap \"https://*.wild.example.com/\"]\n\troot = /r-wild\n\
     [scap \"ssh://git@sshy.example.com/\"]\n\troot = /r-scheme-user\n\
     [scap \"https://user@auth.example.com/\"]\n\troot = /r-user\n";

/// A recording `git`: a script that appends its arguments to a log and then
/// `exec`s the real binary, so the delegation genuinely runs and its spawns
/// stay countable. Both paths are baked into the script text, so nothing has
/// to be exported into this process's environment -- which is what keeps
/// these tests free of the `set_var` W2.1 deleted.
struct RecordingGit {
    log: PathBuf,
}

impl RecordingGit {
    /// Every recorded invocation, in order, one entry per `git` call.
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .expect("read the recording log")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Only the rule (d) delegations.
    fn urlmatch_lines(&self) -> Vec<String> {
        self.lines()
            .into_iter()
            .filter(|line| line.starts_with("config --path --get-urlmatch"))
            .collect()
    }
}

impl Fixture {
    /// A recording `git` in this fixture, and the [`Env`] whose `PATH` holds
    /// that wrapper and nothing else, so every `git` scap runs is recorded.
    fn recording_git(&self) -> (RecordingGit, Env) {
        use std::os::unix::fs::PermissionsExt;

        let real = sources::resolve_git_program(&self.env()).expect("a real git on PATH");
        let bin = self.root.join("recording-bin");
        std::fs::create_dir_all(&bin).expect("mkdir the recording bin dir");
        let log = self.root.join("git-invocations.log");
        std::fs::write(&log, "").expect("create the recording log");

        let script = bin.join("git");
        std::fs::write(
            &script,
            // `"$*"` joins on the first character of IFS -- a space -- and
            // writes exactly one line per invocation.
            format!(
                "#!/bin/bash\nprintf '%s\\n' \"$*\" >> \"{}\"\nexec \"{}\" \"$@\"\n",
                log.display(),
                real.display()
            ),
        )
        .expect("write the recording script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x the recording script");

        let env = Env { path: Some(bin.into_os_string()), ..self.env() };
        (RecordingGit { log }, env)
    }
}

/// Ask the real `git` the rule (d) question, through the same [`Env`].
fn git_urlmatch(env: &Env, url: &str) -> Option<PathBuf> {
    let program = sources::resolve_git_program(env).expect("a real git on PATH");
    let mut command = std::process::Command::new(program);
    command.args(["config", "--path", "--get-urlmatch", "scap.root", url]);
    git_backend::apply_env(&mut command, env);
    let output = command.output().expect("run git config --get-urlmatch");
    match output.status.code() {
        Some(0) => {
            let stdout = String::from_utf8(output.stdout).expect("utf-8 git output");
            let trimmed = stdout.trim();
            (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
        }
        // git exits 1 when `scap.root` is set nowhere at all.
        Some(1) => None,
        other => panic!("git config --get-urlmatch exited {other:?} for {url}"),
    }
}

#[test]
fn root_for_url_matches_git_for_every_url_section_kind() {
    // The two spellings only `get --ssh` produces -- an `ssh://` scheme and
    // a `user@host` authority -- are unreachable through `rm`/`create`,
    // which normalise to `https://<host>/<owner>/<name>`, so the integration
    // matrix in tests/config_oracle.rs covers those two sections only
    // negatively. Here they are matched positively, with git as the oracle.
    let f = Fixture::new();
    f.write("home/.gitconfig", SIX_SECTIONS);
    let env = f.env();
    let snapshot = load_ok(&env);

    for (url, expected) in [
        ("https://exact.example.com/o/n", "/r-exact"),
        ("https://pathy.example.com/team/repo", "/r-path-short"),
        ("https://pathy.example.com/team/deep", "/r-path-long"),
        ("https://foo.wild.example.com/o/n", "/r-wild"),
        ("ssh://git@sshy.example.com/o/n", "/r-scheme-user"),
        ("https://user@auth.example.com/o/n", "/r-user"),
        // The same two sections, asked without the scheme or the user that
        // makes them match: git falls back to the plain key, so scap must.
        ("https://sshy.example.com/o/n", "/plain-last"),
        ("https://auth.example.com/o/n", "/plain-last"),
        ("https://nomatch.example.com/o/n", "/plain-last"),
    ] {
        // The fixture is only meaningful if each section really is the one
        // git picks; that is what makes an all-fallback fixture impossible.
        assert_eq!(
            git_urlmatch(&env, url),
            Some(pb(expected)),
            "the oracle must exercise the section under test for {url}"
        );
        assert_eq!(snapshot.root_for_url(url).expect("root"), pb(expected), "{url}");
    }
}

#[test]
fn urlmatch_spawns_once_per_distinct_url() {
    let f = Fixture::new();
    f.write(
        "home/.gitconfig",
        "[scap]\n\troot = /plain\n\
         [scap \"https://one.example.com/\"]\n\troot = /r-one\n\
         [scap \"https://two.example.com/\"]\n\troot = /r-two\n",
    );
    let (git, env) = f.recording_git();
    let snapshot = load_ok(&env);

    let urls = [
        ("https://one.example.com/o/n", "/r-one"),
        ("https://two.example.com/o/n", "/r-two"),
        // No section matches this one: git's answer is the plain key, and
        // that answer is memoised too.
        ("https://three.example.com/o/n", "/plain"),
    ];

    // Interleaved rather than grouped, so a memo that only remembered the
    // most recent question would still spawn six times.
    for _ in 0..3 {
        for (url, expected) in urls {
            assert_eq!(snapshot.root_for_url(url).expect("root"), pb(expected), "{url}");
        }
    }

    assert_eq!(
        git.urlmatch_lines().len(),
        urls.len(),
        "one spawn per distinct URL, not per lookup: {:?}",
        git.urlmatch_lines()
    );
    assert_eq!(
        git.lines().len(),
        urls.len(),
        "the delegations must be the only `git` this process ran: {:?}",
        git.lines()
    );
}

#[test]
fn concurrent_lookups_of_one_url_spawn_git_once() {
    // `get --parallel` runs six workers over one queue, so two of them can
    // reach rule (d) with the same URL at the same time.
    let f = Fixture::new();
    f.write(
        "home/.gitconfig",
        "[scap]\n\troot = /plain\n[scap \"https://one.example.com/\"]\n\troot = /r-one\n",
    );
    let (git, env) = f.recording_git();
    let snapshot = load_ok(&env);
    let url = "https://one.example.com/o/n";

    std::thread::scope(|scope| {
        for _ in 0..6 {
            let snapshot = &snapshot;
            scope.spawn(move || {
                assert_eq!(snapshot.root_for_url(url).expect("root"), pb("/r-one"));
            });
        }
    });

    assert_eq!(
        git.urlmatch_lines().len(),
        1,
        "six concurrent lookups of one URL must share one answer: {:?}",
        git.urlmatch_lines()
    );
}

#[test]
fn root_for_url_never_spawns_git_without_url_sections() {
    // The W2.1 guarantee, kept as a regression test: rule (c) answers from
    // the in-process snapshot, so nothing is delegated at all.
    let f = Fixture::new();
    f.write("home/.gitconfig", "[scap]\n\troot = /first\n\troot = /plain-last\n");
    let (git, env) = f.recording_git();
    let snapshot = load_ok(&env);

    for url in ["https://one.example.com/o/n", "https://two.example.com/o/n"] {
        assert_eq!(snapshot.root_for_url(url).expect("root"), pb("/plain-last"), "{url}");
    }

    assert!(git.lines().is_empty(), "no url sections must mean no spawn: {:?}", git.lines());
}

#[test]
fn rule_d_without_git_on_path_is_fatal_rather_than_a_silent_fallback() {
    let f = Fixture::new();
    f.write(
        "home/.gitconfig",
        "[scap]\n\troot = /plain\n[scap \"https://one.example.com/\"]\n\troot = /r-one\n",
    );
    std::fs::create_dir_all(f.path().join("empty-bin")).expect("mkdir empty-bin");

    let env = Env { path: Some(f.path().join("empty-bin").into_os_string()), ..f.env() };
    let snapshot = load_ok(&env);

    // The snapshot itself still loads in process; only rule (d) needs git.
    assert_eq!(snapshot.backend(), Backend::InProcess);
    assert_eq!(snapshot.reason(), Reason::UrlSections);
    assert_eq!(snapshot.reason().to_string(), "url_sections");

    let url = "https://one.example.com/o/n";
    for attempt in 0..2 {
        let err = snapshot.root_for_url(url).expect_err("rule (d) cannot be answered without git");
        assert!(
            matches!(err, ConfigError::GitRequired { reason } if reason.contains("url-scoped")),
            "attempt {attempt} must name the trigger, got: {err}"
        );
    }
}

// -- process-wide snapshot and helpers ------------------------------------

#[test]
fn snapshot_is_built_once_and_reused() {
    let first = snapshot();
    let second = snapshot();
    assert!(std::ptr::eq(first, second), "the OnceLock must hand back one value");
    assert!(matches!(first.backend(), Backend::InProcess | Backend::Git));
}

#[test]
fn env_from_process_sees_the_default_system_probe_list() {
    let env = Env::from_process();
    assert_eq!(env.system_probe_candidates, sources::default_system_candidates());
}

#[test]
fn clean_path_normalizes_parent_and_current() {
    assert_eq!(clean_path(Path::new("/a/b/../c")), pb("/a/c"));
    assert_eq!(clean_path(Path::new("/a/./b")), pb("/a/b"));
    assert_eq!(clean_path(Path::new("./relative")), pb("relative"));
}

#[test]
fn user_and_complete_user_read_the_process_snapshot() {
    // The free functions are the command-facing API; they must agree with
    // the snapshot they read.
    assert_eq!(user().expect("user"), snapshot().user().map(str::to_owned));
    assert_eq!(complete_user().expect("completeUser"), snapshot().complete_user());
}

#[test]
fn resolve_roots_and_root_for_url_read_the_process_snapshot() {
    let from_module = resolve_roots(false).expect("resolve_roots");
    let from_snapshot = snapshot().resolve_roots(false).expect("resolve");
    assert_eq!(from_module, from_snapshot);

    let url = "https://github.com/zchee/scap";
    assert_eq!(
        root_for_url(url).expect("root_for_url"),
        snapshot().root_for_url(url).expect("root")
    );
}
