//! ADR-8 configuration oracles (i)-(vi).
//!
//! `git` itself is the oracle: every expectation is computed by running the
//! real `git config` over the same fixture and the same environment, never
//! by restating scap's parsing in the test. Each case runs under both
//! configuration backends -- the A4 in-process default and the A3
//! `SCAP_CONFIG_BACKEND=git` backend -- because ADR-8 makes them
//! interchangeable by contract.
//!
//! Oracle (iv) -- the system-config probe over an injected candidate list --
//! is a unit test in `src/config/sources_tests.rs`, where the injection
//! point lives.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use tempfile::TempDir;

/// The two backends every oracle runs under.
const BACKENDS: [Option<&str>; 2] = [None, Some("git")];

fn backend_name(backend: Option<&str>) -> &'static str {
    match backend {
        None => "in-process (A4)",
        Some(_) => "git (A3)",
    }
}

/// A gitconfig fixture plus the environment both scap and the oracle see.
struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    cfg: PathBuf,
    /// Extra variables applied to both scap and the oracle `git`.
    extra: Vec<(String, String)>,
    /// Set instead of `GIT_CONFIG_GLOBAL` when the fixture drives the system
    /// level (oracle iii).
    system: Option<PathBuf>,
}

impl Fixture {
    fn new(gitconfig: &str) -> Self {
        let tmp = TempDir::new().expect("tempdir");
        // Physical spelling: `resolve_roots` canonicalises, and on macOS the
        // temp dir is reached through the `/var` -> `/private/var` symlink.
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
        std::fs::create_dir_all(root.join("home")).expect("mkdir home");
        let cfg = root.join("gitconfig");
        std::fs::write(&cfg, gitconfig).expect("write gitconfig");
        Self { _tmp: tmp, root, cfg, extra: Vec::new(), system: None }
    }

    fn with_env(mut self, key: &str, value: &str) -> Self {
        self.extra.push((key.to_owned(), value.to_owned()));
        self
    }

    /// Drive the fixture through `GIT_CONFIG_SYSTEM` instead of
    /// `GIT_CONFIG_GLOBAL` (oracle iii).
    fn at_the_system_level(mut self) -> Self {
        self.system = Some(self.cfg.clone());
        self
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn apply(&self, set: &mut dyn FnMut(&str, &str)) {
        set("GIT_CONFIG_NOSYSTEM", "1");
        match &self.system {
            Some(system) => {
                // `GIT_CONFIG_NOSYSTEM` would suppress it, and the global
                // level must stay empty so only the system file speaks.
                set("GIT_CONFIG_NOSYSTEM", "0");
                set("GIT_CONFIG_SYSTEM", &system.to_string_lossy());
                set("GIT_CONFIG_GLOBAL", &self.root.join("empty-gitconfig").to_string_lossy());
                std::fs::write(self.root.join("empty-gitconfig"), "").expect("empty global");
            }
            None => set("GIT_CONFIG_GLOBAL", &self.cfg.to_string_lossy()),
        }
        set("HOME", &self.home().to_string_lossy());
        for (key, value) in &self.extra {
            set(key, value);
        }
    }

    fn scap(&self, backend: Option<&str>) -> Command {
        let mut cmd = Command::cargo_bin("scap").expect("the scap binary");
        cmd.env_remove("SCAP_ROOT")
            .env_remove("SCAP_CONFIG_BACKEND")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("XDG_CONFIG_HOME")
            .current_dir(self.path());
        self.apply(&mut |key, value| {
            cmd.env(key, value);
        });
        if let Some(backend) = backend {
            cmd.env("SCAP_CONFIG_BACKEND", backend);
        }
        cmd
    }

    /// Run the real `git` over the same fixture: the oracle.
    fn git(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = StdCommand::new("git");
        cmd.args(args)
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("XDG_CONFIG_HOME")
            .current_dir(self.path());
        self.apply(&mut |key, value| {
            cmd.env(key, value);
        });
        cmd.output().expect("run the oracle git")
    }

    fn git_lines(&self, args: &[&str]) -> Vec<String> {
        let out = self.git(args);
        String::from_utf8(out.stdout)
            .expect("utf-8 git output")
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The root list `scap root --all` must print, computed from git's own
    /// answers plus the two documented rules (reversal of the plain values,
    /// then the url-scoped ones appended) and scap's dedup/canonicalisation.
    fn expected_roots(&self, all: bool) -> Vec<String> {
        let mut roots: Vec<PathBuf> = self
            .git_lines(&["config", "--path", "--get-all", "scap.root"])
            .into_iter()
            .map(PathBuf::from)
            .collect();
        roots.reverse();
        if roots.is_empty() {
            roots.push(self.home().join("scap"));
        }
        if all {
            for line in self.git_lines(&["config", "--path", "--get-regexp", r"^scap\..+\.root$"]) {
                let (_key, value) = line.split_once(char::is_whitespace).expect("key and value");
                roots.push(PathBuf::from(value.trim_start()));
            }
        }
        let mut seen = std::collections::HashSet::new();
        roots
            .into_iter()
            .map(|root| std::fs::canonicalize(&root).unwrap_or(root))
            .filter(|root| seen.insert(root.clone()))
            .map(|root| root.display().to_string())
            .collect()
    }

    /// The destination `scap` resolves for `target`, read back from the
    /// message `rm` prints for a path that does not exist. This observes
    /// `scap.user`, `scap.completeUser` and `root_for_url` together without
    /// adding any CLI surface.
    fn resolved_dest(&self, backend: Option<&str>, target: &str) -> String {
        dest_message(&mut self.scap(backend), target)
    }

    /// The `/<host>/<owner>/<name>` tail scap appends to whichever root it
    /// picked, read back by resolving `target` against a root this test
    /// dictates (`SCAP_ROOT`, ADR-8 rule (a)).
    ///
    /// That keeps the oracle's expectation free of any restatement of
    /// scap's URL normalisation: the tail comes from scap, the root comes
    /// from git, and the assertion is that the two are concatenated.
    fn dest_tail(&self, target: &str) -> String {
        const SENTINEL: &str = "/scap-oracle-sentinel";
        let mut cmd = self.scap(None);
        cmd.env("SCAP_ROOT", SENTINEL);
        let dest = dest_message(&mut cmd, target);
        dest.strip_prefix(SENTINEL)
            .unwrap_or_else(|| panic!("{dest} does not start with the sentinel root"))
            .to_owned()
    }
}

/// Run `rm --dry-run <target>` and return the destination path it names.
///
/// Both spellings are `rm` reporting what `root_for_url` resolved: a
/// destination that does not exist is quoted in the `does not exist` error
/// on stderr, and one that does exist is printed by `--dry-run` itself as
/// `Would remove <path>` on stdout. A fixture can produce either -- the ghq
/// oracle creates the directory it prints -- so both are accepted.
fn dest_message(cmd: &mut Command, target: &str) -> String {
    let output = cmd.args(["rm", "--dry-run", target]).output().expect("run scap rm --dry-run");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if let Some(start) = stderr.find('"') {
        let end = stderr[start + 1..]
            .find('"')
            .unwrap_or_else(|| panic!("unterminated quoted path in: {stderr}"));
        return stderr[start + 1..start + 1 + end].to_owned();
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    stdout
        .strip_prefix("Would remove ")
        .map(|path| path.trim_end().to_owned())
        .unwrap_or_else(|| panic!("no destination in stdout {stdout:?} / stderr {stderr:?}"))
}

fn stdout_of(cmd: &mut Command) -> String {
    let output = cmd.output().expect("run scap");
    assert!(output.status.success(), "scap failed: {output:?}");
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

// -- oracle (i): the snapshot equals what git reads -----------------------

#[test]
fn oracle_i_roots_match_git_over_the_fixture_matrix() {
    let interesting = [
        ("plain multi-root", "[scap]\n\troot = /a\n\troot = /b\n\troot = /c\n".to_owned()),
        ("duplicate values", "[scap]\n\troot = /same\n\troot = /same\n".to_owned()),
        ("tilde home", "[scap]\n\troot = ~/one\n\troot = /two\n".to_owned()),
        ("tilde user", "[scap]\n\troot = ~root/src\n".to_owned()),
        ("quoted value", "[scap]\n\troot = \"/quoted path/one\"\n".to_owned()),
        ("line continuation", "[scap]\n\troot = /two\\\n/three\n".to_owned()),
        (
            "url-scoped sections",
            "[scap]\n\troot = /default\n\
             [scap \"https://example.com/\"]\n\troot = /custom\n"
                .to_owned(),
        ),
        ("nothing configured", String::new()),
    ];

    for (name, gitconfig) in interesting {
        let f = Fixture::new(&gitconfig);
        let expect_all = f.expected_roots(true);
        let expect_first = f.expected_roots(false);

        for backend in BACKENDS {
            let all = stdout_of(f.scap(backend).arg("root").arg("--all"));
            assert_eq!(
                all,
                format!("{}\n", expect_all.join("\n")),
                "`root --all` disagreed with git for {name} under {}",
                backend_name(backend)
            );

            let first = stdout_of(f.scap(backend).arg("root"));
            assert_eq!(
                first,
                format!("{}\n", expect_first[0]),
                "`root` disagreed with git for {name} under {}",
                backend_name(backend)
            );
        }
    }
}

#[test]
fn oracle_i_includes_are_followed_the_way_git_follows_them() {
    let f = Fixture::new("[scap]\n\troot = /from-global\n[include]\n\tpath = included\n");
    std::fs::write(f.path().join("included"), "[scap]\n\troot = /from-include\n")
        .expect("write the included file");

    let expected = f.expected_roots(true);
    assert_eq!(expected, ["/from-include".to_owned(), "/from-global".to_owned()]);

    for backend in BACKENDS {
        assert_eq!(
            stdout_of(f.scap(backend).arg("root").arg("--all")),
            format!("{}\n", expected.join("\n")),
            "under {}",
            backend_name(backend)
        );
    }
}

#[test]
fn oracle_i_user_and_complete_user_truthiness_matches_git() {
    for (spelling, complete) in [
        ("completeUser = true", true),
        ("completeUser = yes", true),
        ("completeUser = on", true),
        ("completeUser = 1", true),
        ("completeUser = false", false),
        ("completeUser = no", false),
        ("completeUser = off", false),
        ("completeUser = 0", false),
        ("completeUser", true),
    ] {
        let f = Fixture::new(&format!("[scap]\n\troot = /r\n\tuser = motemen\n\t{spelling}\n"));

        // git's own `--bool` reading of the same line is the oracle.
        let oracle = f.git_lines(&["config", "--bool", "--get", "scap.completeUser"]);
        assert_eq!(oracle, [complete.to_string()], "the oracle disagrees for {spelling}");

        let expected =
            if complete { "/r/github.com/motemen/myproj" } else { "/r/github.com/myproj/myproj" };
        for backend in BACKENDS {
            assert_eq!(
                f.resolved_dest(backend, "myproj"),
                expected,
                "{spelling} under {}",
                backend_name(backend)
            );
        }
    }
}

#[test]
fn oracle_i_git_config_parameters_routes_to_the_git_backend_byte_equal() {
    // git's highest-precedence source, the one `git -c key=value` populates.
    // ADR-8 does not emulate it in process; it routes the whole snapshot to
    // the A3 backend, so both backends must still answer identically.
    let f = Fixture::new("[scap]\n\troot = /from-file\n")
        .with_env("GIT_CONFIG_PARAMETERS", "'scap.root=/from-parameters'");

    let expected = f.expected_roots(false);
    assert_eq!(expected[0], "/from-parameters", "git must prefer the parameter");

    for backend in BACKENDS {
        assert_eq!(
            stdout_of(f.scap(backend).arg("root")),
            format!("{}\n", expected[0]),
            "under {}",
            backend_name(backend)
        );
        assert_eq!(
            stdout_of(f.scap(backend).arg("root").arg("--all")),
            format!("{}\n", expected.join("\n")),
            "under {}",
            backend_name(backend)
        );
    }
}

#[test]
fn oracle_i_git_config_count_routes_to_the_git_backend_byte_equal() {
    let f = Fixture::new("[scap]\n\troot = /from-file\n")
        .with_env("GIT_CONFIG_COUNT", "1")
        .with_env("GIT_CONFIG_KEY_0", "scap.root")
        .with_env("GIT_CONFIG_VALUE_0", "/from-count");

    let expected = f.expected_roots(false);
    assert_eq!(expected[0], "/from-count");

    for backend in BACKENDS {
        assert_eq!(
            stdout_of(f.scap(backend).arg("root")),
            format!("{}\n", expected[0]),
            "under {}",
            backend_name(backend)
        );
        assert_eq!(
            stdout_of(f.scap(backend).arg("root").arg("--all")),
            format!("{}\n", expected.join("\n")),
            "under {}",
            backend_name(backend)
        );
    }
}

// -- oracle (ii): urlmatch delegation ------------------------------------

/// Six url-scoped sections, one per rule `urlmatch.c` implements: an exact
/// host, a host with a path prefix, a longer path prefix that must beat the
/// shorter one, a `*.` wildcard host, a scheme-specific section, and a
/// `user@host` one. `{plain}` is the last plain `scap.root`.
fn six_sections(plain: &str) -> String {
    format!(
        "[scap]\n\troot = /plain-first\n\troot = {plain}\n\
         [scap \"https://exact.example.com/\"]\n\troot = /r-exact\n\
         [scap \"https://pathy.example.com/team\"]\n\troot = /r-path-short\n\
         [scap \"https://pathy.example.com/team/deep\"]\n\troot = /r-path-long\n\
         [scap \"https://*.wild.example.com/\"]\n\troot = /r-wild\n\
         [scap \"ssh://git@sshy.example.com/\"]\n\troot = /r-scheme-user\n\
         [scap \"https://user@auth.example.com/\"]\n\troot = /r-user\n"
    )
}

#[test]
fn oracle_ii_urlmatch_delegation_matches_git_over_six_sections_and_twelve_urls() {
    // The last plain root is spelled through a symlinked component, which
    // is what separates rule (d)/(c) -- git's raw output -- from rule (b)'s
    // canonicalising fallback, exercised by the codecommit target below.
    let f = Fixture::new("");
    std::fs::create_dir_all(f.path().join("real/last")).expect("mkdir real/last");
    std::os::unix::fs::symlink(f.path().join("real"), f.path().join("lnk")).expect("symlink");
    let via_link = f.path().join("lnk").join("last");
    std::fs::write(f.path().join("gitconfig"), six_sections(&via_link.display().to_string()))
        .expect("rewrite gitconfig");
    let plain = via_link.display().to_string();

    // Eleven URLs through the delegation: every section matched at least
    // once, every near miss git resolves to the plain key instead, and the
    // path-prefix pair proving the longer pattern wins. `rm` normalises its
    // target to `https://<host>/<owner>/<name>`, so the `ssh://` and
    // `user@` sections are reachable here only as near misses; they are
    // matched positively in `root_for_url_matches_git_for_every_url_section_kind`
    // (src/config_tests.rs), which drives the same six sections directly.
    let cases: [(&str, &str); 11] = [
        ("https://exact.example.com/o/n", "/r-exact"),
        ("https://exact.example.com/other/repo", "/r-exact"),
        ("https://pathy.example.com/team/repo", "/r-path-short"),
        ("https://pathy.example.com/team/deep", "/r-path-long"),
        ("https://foo.wild.example.com/o/n", "/r-wild"),
        ("https://wild.example.com/o/n", &plain),
        ("https://sub.foo.wild.example.com/o/n", &plain),
        ("https://sshy.example.com/o/n", &plain),
        ("https://auth.example.com/o/n", &plain),
        ("https://pathy.example.com/other/repo", &plain),
        ("https://nomatch.example.com/o/n", &plain),
    ];

    for (target, expected_root) in cases {
        // git is the oracle; the label only guards against a fixture where
        // every URL collapses to the plain key and nothing is exercised.
        let oracle = f.git_lines(&["config", "--path", "--get-urlmatch", "scap.root", target]);
        assert_eq!(
            oracle,
            [expected_root.to_owned()],
            "the fixture must exercise the section under test for {target}"
        );

        let tail = f.dest_tail(target);
        for backend in BACKENDS {
            assert_eq!(
                f.resolved_dest(backend, target),
                format!("{expected_root}{tail}"),
                "{target} must resolve to git's own urlmatch answer under {}",
                backend_name(backend)
            );
        }
    }

    // The twelfth URL: a codecommit target skips urlmatch entirely (rule b)
    // and takes ghq's canonicalised primary root, so here it must *not*
    // follow git's urlmatch answer -- which is the raw symlinked spelling.
    let codecommit = "codecommit::us-east-1://my-repo";
    let last_plain = f
        .git_lines(&["config", "--path", "--get-all", "scap.root"])
        .pop()
        .expect("the fixture sets scap.root");
    let canonical = std::fs::canonicalize(&last_plain).expect("the symlink target exists");
    assert_ne!(canonical.display().to_string(), last_plain, "the fixture must cross a symlink");

    let tail = f.dest_tail(codecommit);
    for backend in BACKENDS {
        assert_eq!(
            f.resolved_dest(backend, codecommit),
            format!("{}{tail}", canonical.display()),
            "a codecommit target takes rule (b), not the urlmatch answer, under {}",
            backend_name(backend)
        );
    }
}

// -- oracle (iii): GIT_CONFIG_SYSTEM -------------------------------------

#[test]
fn oracle_iii_scap_root_from_the_system_level_only() {
    let f = Fixture::new("[scap]\n\troot = /from-system\n").at_the_system_level();

    let oracle = f.git_lines(&["config", "--path", "--get-all", "scap.root"]);
    assert_eq!(oracle, ["/from-system".to_owned()], "the fixture must reach git's system level");

    for backend in BACKENDS {
        assert_eq!(
            stdout_of(f.scap(backend).arg("root")),
            "/from-system\n",
            "under {}",
            backend_name(backend)
        );
    }
}

// -- oracle (v): the probe against git's own system file ------------------

#[test]
fn oracle_v_probe_selects_the_same_system_files_as_git() {
    // Reads the machine's real system configuration, so it runs only when
    // asked for: `SCAP_ORACLE_SYSTEM=1`.
    if std::env::var_os("SCAP_ORACLE_SYSTEM").is_none() {
        eprintln!("skipped: set SCAP_ORACLE_SYSTEM=1 to compare against the real system config");
        return;
    }

    let probed = scap::config::sources::probe_system_config(
        &scap::config::sources::default_system_candidates(),
    );

    let output = StdCommand::new("git")
        .args(["config", "--list", "--show-origin", "--system"])
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .env_remove("GIT_CONFIG_SYSTEM")
        .output()
        .expect("run git config --system");
    let mut origins: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split('\t').next())
        .filter_map(|origin| origin.strip_prefix("file:"))
        .map(PathBuf::from)
        .collect();
    origins.sort();
    origins.dedup();

    let mut probed_sorted = probed;
    probed_sorted.sort();

    assert_eq!(
        probed_sorted, origins,
        "the probe must select exactly the files git reads at the system level"
    );
}

// -- oracle (vi): the plain-key fallback ----------------------------------

#[test]
fn oracle_vi_plain_key_fallback_is_byte_equal_to_git_urlmatch() {
    // A url-section-free fixture whose last `scap.root` traverses a
    // symlinked component. `git config --path --get-urlmatch` prints that
    // value raw; ghq uses it raw; so scap must not resolve `lnk` away.
    let f = Fixture::new("");
    std::fs::create_dir_all(f.path().join("real/last")).expect("mkdir real/last");
    std::os::unix::fs::symlink(f.path().join("real"), f.path().join("lnk")).expect("symlink");
    let via_link = f.path().join("lnk").join("last");
    std::fs::write(
        f.path().join("gitconfig"),
        format!("[scap]\n\troot = /first\n\troot = {}\n", via_link.display()),
    )
    .expect("rewrite gitconfig");

    let url = "https://github.com/x/y";
    let oracle = f.git_lines(&["config", "--path", "--get-urlmatch", "scap.root", url]);
    assert_eq!(
        oracle,
        [via_link.display().to_string()],
        "git prints the last plain value raw when no url section matches"
    );

    let resolved = std::fs::canonicalize(&via_link).expect("the symlink target exists");
    assert_ne!(resolved, via_link, "the fixture must actually traverse a symlink");

    for backend in BACKENDS {
        let dest = f.resolved_dest(backend, url);
        assert_eq!(
            dest,
            format!("{}/github.com/x/y", via_link.display()),
            "rule (c) must use git's raw urlmatch output under {}",
            backend_name(backend)
        );
    }
}

#[test]
fn oracle_vi_url_scoped_sections_still_delegate_to_git() {
    let f = Fixture::new(
        "[scap]\n\troot = /default\n\
         [scap \"https://special.example.com/\"]\n\troot = /special\n",
    );

    for (url, expected_root) in [
        ("https://special.example.com/foo/bar", "/special"),
        ("https://other.example.com/foo/bar", "/default"),
    ] {
        let oracle = f.git_lines(&["config", "--path", "--get-urlmatch", "scap.root", url]);
        assert_eq!(oracle, [expected_root.to_owned()], "oracle for {url}");

        for backend in BACKENDS {
            let dest = f.resolved_dest(backend, url);
            assert!(
                dest.starts_with(&format!("{expected_root}/")),
                "{url} resolved to {dest} under {}",
                backend_name(backend)
            );
        }
    }
}

// -- repository-level configuration: common dir and `config.worktree` -----

/// A real repository (and, where needed, a real linked worktree) plus the
/// isolated environment both scap and the oracle see. Everything here is
/// created by the real `git`: the layout under test -- `commondir`, the
/// worktree-private `config`, `extensions.worktreeConfig` -- is exactly the
/// one git writes.
struct RepoFixture {
    _tmp: TempDir,
    root: PathBuf,
    global: PathBuf,
}

impl RepoFixture {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
        std::fs::create_dir_all(root.join("home")).expect("mkdir home");
        let global = root.join("global-gitconfig");
        // `git worktree add` needs a commit, and a commit needs an identity.
        std::fs::write(&global, "[user]\n\tname = t\n\temail = t@example.invalid\n")
            .expect("write the global gitconfig");
        Self { _tmp: tmp, root, global }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn env(&self) -> Vec<(String, String)> {
        vec![
            ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
            ("GIT_CONFIG_GLOBAL".to_owned(), self.global.display().to_string()),
            ("HOME".to_owned(), self.home().display().to_string()),
        ]
    }

    fn git(&self, dir: &Path, args: &[&str]) -> std::process::Output {
        let mut cmd = StdCommand::new("git");
        cmd.args(args)
            .current_dir(dir)
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("GIT_DIR");
        for (key, value) in self.env() {
            cmd.env(key, value);
        }
        cmd.output().expect("run git")
    }

    fn git_ok(&self, dir: &Path, args: &[&str]) {
        let out = self.git(dir, args);
        assert!(out.status.success(), "git {args:?} failed: {out:?}");
    }

    /// The oracle's answer for `scap.root`, in git's own order.
    fn oracle_roots(&self, dir: &Path) -> Vec<String> {
        let out = self.git(dir, &["config", "--path", "--get-all", "scap.root"]);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn scap(&self, dir: &Path, backend: Option<&str>) -> Command {
        let mut cmd = Command::cargo_bin("scap").expect("the scap binary");
        cmd.current_dir(dir)
            .env_remove("SCAP_ROOT")
            .env_remove("SCAP_CONFIG_BACKEND")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("GIT_DIR");
        for (key, value) in self.env() {
            cmd.env(key, value);
        }
        if let Some(backend) = backend {
            cmd.env("SCAP_CONFIG_BACKEND", backend);
        }
        cmd
    }

    /// `scap root --all` stdout as lines.
    fn scap_roots(&self, dir: &Path, backend: Option<&str>) -> Vec<String> {
        stdout_of(self.scap(dir, backend).arg("root").arg("--all"))
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn repo_config_comes_from_the_common_dir_not_the_linked_worktree_gitdir() {
    let f = RepoFixture::new();
    let main = f.root.join("main");
    std::fs::create_dir_all(&main).expect("mkdir main");
    f.git_ok(&main, &["init", "-q"]);
    f.git_ok(&main, &["commit", "-q", "--allow-empty", "-m", "seed"]);
    let wt = f.root.join("wt");
    f.git_ok(&main, &["worktree", "add", "-q", wt.to_str().expect("utf-8 path")]);

    let linked_git_dir = main.join(".git/worktrees/wt");
    assert!(linked_git_dir.join("commondir").is_file(), "git must have written commondir");

    let mut common = std::fs::read_to_string(main.join(".git/config")).expect("read common config");
    common.push_str("[scap]\n\troot = /from-common-config\n");
    std::fs::write(main.join(".git/config"), common).expect("write common config");
    std::fs::write(linked_git_dir.join("config"), "[scap]\n\troot = /from-linked-gitdir-config\n")
        .expect("write the worktree-private config");

    let oracle = f.oracle_roots(&wt);
    assert_eq!(
        oracle,
        ["/from-common-config".to_owned()],
        "git reads the common dir's config from inside a linked worktree"
    );

    for backend in BACKENDS {
        assert_eq!(
            f.scap_roots(&wt, backend),
            ["/from-common-config".to_owned()],
            "under {}",
            backend_name(backend)
        );
        // And the fall-through must not happen: `$HOME/scap` here would mean
        // the repository level was missed entirely.
        assert_ne!(
            f.scap_roots(&wt, backend),
            [f.home().join("scap").display().to_string()],
            "under {}",
            backend_name(backend)
        );
    }
}

#[test]
fn config_worktree_is_read_only_when_the_extension_is_enabled() {
    let f = RepoFixture::new();
    let repo = f.root.join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    f.git_ok(&repo, &["init", "-q"]);
    std::fs::write(repo.join(".git/config.worktree"), "[scap]\n\troot = /from-worktree-config\n")
        .expect("write config.worktree");

    // Without the extension git ignores the file entirely, so `--get-all`
    // finds nothing and exits 1.
    let without = f.git(&repo, &["config", "--path", "--get-all", "scap.root"]);
    assert_eq!(without.status.code(), Some(1), "the oracle must not see the file: {without:?}");
    for backend in BACKENDS {
        assert_eq!(
            f.scap_roots(&repo, backend),
            [f.home().join("scap").display().to_string()],
            "config.worktree must be ignored under {}",
            backend_name(backend)
        );
    }

    f.git_ok(&repo, &["config", "extensions.worktreeConfig", "true"]);

    assert_eq!(
        f.oracle_roots(&repo),
        ["/from-worktree-config".to_owned()],
        "with the extension enabled git reads it"
    );
    for backend in BACKENDS {
        assert_eq!(
            f.scap_roots(&repo, backend),
            ["/from-worktree-config".to_owned()],
            "under {}",
            backend_name(backend)
        );
    }
}

#[test]
fn an_empty_git_config_global_suppresses_the_global_level() {
    // m1: an empty value is a real, unopenable path to git -- the level is
    // suppressed, not defaulted back to `~/.gitconfig`.
    let f = Fixture::new("[scap]\n\troot = /from-global\n").with_env("GIT_CONFIG_GLOBAL", "");

    let oracle = f.git(&["config", "--path", "--get-all", "scap.root"]);
    assert_eq!(oracle.status.code(), Some(1), "the oracle must see nothing: {oracle:?}");

    for backend in BACKENDS {
        assert_eq!(
            stdout_of(f.scap(backend).arg("root")),
            format!("{}/scap\n", f.home().display()),
            "under {}",
            backend_name(backend)
        );
    }
}

// -- rule (b): codecommit targets take the canonicalised primary root -----

/// True when `GHQ_BINARY` names a file the current user can execute.
/// Mirrors tests/parity_ghq.rs, so `SCAP_REQUIRE_GHQ=1` makes a missing
/// oracle a hard failure rather than a silent pass.
fn ghq_oracle() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let executable = std::env::var_os("GHQ_BINARY").is_some_and(|path| {
        std::fs::metadata(PathBuf::from(path))
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    });
    if std::env::var("SCAP_REQUIRE_GHQ").as_deref() == Ok("1") && !executable {
        panic!(
            "SCAP_REQUIRE_GHQ=1 demands a ghq oracle, but GHQ_BINARY is unset or does not name \
             an executable file (GHQ_BINARY={:?})",
            std::env::var_os("GHQ_BINARY")
        );
    }
    executable.then(|| PathBuf::from(std::env::var_os("GHQ_BINARY").expect("checked above")))
}

/// A fixture whose single `scap.root`/`ghq.root` reaches its directory
/// through a symlinked component, which is what separates ADR-8 rule (c)
/// (raw) from rule (e) (canonicalised).
fn symlinked_root_fixture() -> (Fixture, PathBuf, PathBuf) {
    let f = Fixture::new("");
    std::fs::create_dir_all(f.path().join("real/last")).expect("mkdir real/last");
    std::os::unix::fs::symlink(f.path().join("real"), f.path().join("lnk")).expect("symlink");
    let via_link = f.path().join("lnk").join("last");
    let resolved = std::fs::canonicalize(&via_link).expect("the symlink target exists");
    assert_ne!(resolved, via_link, "the fixture must actually traverse a symlink");
    std::fs::write(
        f.path().join("gitconfig"),
        format!(
            "[scap]\n\troot = {}\n[ghq]\n\troot = {}\n",
            via_link.display(),
            via_link.display()
        ),
    )
    .expect("rewrite gitconfig");
    (f, via_link, resolved)
}

#[test]
fn codecommit_targets_resolve_against_the_canonicalised_primary_root() {
    // ADR-8 rule (b) at the binary layer, which is the only layer that
    // exercises the spelling production code actually produces:
    // `url::finalize_codecommit` normalises to `codecommit://<region>/<name>`
    // (ghq's own `<root>/<region>/<repo>` layout since 7425b1e), and a rule
    // keyed on the raw `codecommit::<region>://<repo>` form never fires for
    // it.
    let (f, via_link, resolved) = symlinked_root_fixture();

    for backend in BACKENDS {
        let dest = f.resolved_dest(backend, "codecommit::us-east-1://my-repo");
        assert!(
            dest.starts_with(&format!("{}/", resolved.display())),
            "codecommit must resolve under the canonicalised root, got {dest} under {}",
            backend_name(backend)
        );
        assert!(
            !dest.starts_with(&format!("{}/", via_link.display())),
            "the raw rule-(c) root is what ghq does not use, got {dest} under {}",
            backend_name(backend)
        );

        // An https target still takes rule (c), raw, so the two rules stay
        // distinguishable rather than both collapsing to one behaviour.
        let https = f.resolved_dest(backend, "https://github.com/x/y");
        assert_eq!(
            https,
            format!("{}/github.com/x/y", via_link.display()),
            "under {}",
            backend_name(backend)
        );
    }
}

#[test]
fn codecommit_root_component_matches_ghq() {
    let Some(ghq) = ghq_oracle() else {
        eprintln!("skipped: set GHQ_BINARY to compare against the real ghq");
        return;
    };
    let (f, via_link, resolved) = symlinked_root_fixture();

    // `ghq create` prints the destination it computed, which is the only
    // ghq surface that reveals `getRoot`'s answer for a codecommit target.
    let mut cmd = StdCommand::new(&ghq);
    cmd.args(["create", "codecommit::us-east-1://my-repo"])
        .current_dir(f.path())
        .env_remove("GHQ_ROOT")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("XDG_CONFIG_HOME");
    f.apply(&mut |key, value| {
        cmd.env(key, value);
    });
    let out = cmd.output().expect("run ghq create");
    assert!(out.status.success(), "ghq create failed: {out:?}");
    let ghq_dest = String::from_utf8_lossy(&out.stdout).trim().to_owned();

    assert!(
        ghq_dest.starts_with(&format!("{}/", resolved.display())),
        "the oracle itself must use the canonicalised root: {ghq_dest}"
    );
    assert!(!ghq_dest.starts_with(&format!("{}/", via_link.display())), "{ghq_dest}");

    for backend in BACKENDS {
        let dest = f.resolved_dest(backend, "codecommit::us-east-1://my-repo");
        // The root component is what rule (b) decides, and since 7425b1e
        // the path below it agrees with ghq's too: both spell a codecommit
        // destination `<root>/<region>/<repo>`, so the same suffix is
        // stripped from either side.
        let scap_root = dest
            .strip_suffix("/us-east-1/my-repo")
            .unwrap_or_else(|| panic!("unexpected scap destination shape: {dest}"));
        let ghq_root = ghq_dest
            .strip_suffix("/us-east-1/my-repo")
            .unwrap_or_else(|| panic!("unexpected ghq destination shape: {ghq_dest}"));
        assert_eq!(
            scap_root,
            ghq_root,
            "root component must match ghq under {}",
            backend_name(backend)
        );
    }
}
