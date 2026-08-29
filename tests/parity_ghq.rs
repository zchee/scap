use std::fs;
use std::process::Command;

use assert_cmd::Command as ScapCmd;
use tempfile::TempDir;

mod support;

use support::empty_path_dir;

/// The ghq oracle, or an early `return` that says in the log why the test
/// did not run.
///
/// A parity test that skips in silence is indistinguishable in a CI log from
/// one that ran and passed, which is a large part of how ledger #16's eleven
/// broken comparisons survived as long as they did. `SCAP_REQUIRE_GHQ=1`
/// turns the missing oracle into a panic; without it, this at least leaves a
/// line naming the test.
macro_rules! oracle {
    () => {{
        fn probe() {}
        match ghq_binary() {
            Some(ghq) => ghq,
            None => {
                let path = std::any::type_name_of_val(&probe);
                eprintln!(
                    "{}: skipped -- no ghq oracle (set GHQ_BINARY)",
                    path.strip_suffix("::probe").unwrap_or(path)
                );
                return;
            }
        }
    }};
}

/// True when `GHQ_BINARY` names a file the current user can execute.
fn ghq_binary_is_executable() -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Some(p) = std::env::var_os("GHQ_BINARY") else {
        return false;
    };
    std::fs::metadata(std::path::PathBuf::from(p))
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn ghq_binary() -> Option<std::path::PathBuf> {
    // Plan §5 "Oracle presence": V-4 exports both variables, so under
    // SCAP_REQUIRE_GHQ=1 a missing oracle is a hard failure rather than a
    // silent skip -- otherwise the whole parity suite passes vacuously during
    // verification. A developer who exports neither still gets the skip.
    if std::env::var("SCAP_REQUIRE_GHQ").as_deref() == Ok("1") && !ghq_binary_is_executable() {
        panic!(
            "SCAP_REQUIRE_GHQ=1 demands a ghq oracle, but GHQ_BINARY is unset or does not name \
             an executable file (GHQ_BINARY={:?}). Export GHQ_BINARY=$(command -v ghq), or unset \
             SCAP_REQUIRE_GHQ to let these tests skip.",
            std::env::var_os("GHQ_BINARY")
        );
    }

    if let Ok(p) = std::env::var("GHQ_BINARY") {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    for candidate in
        [format!("{home}/go/bin/ghq"), "/usr/local/bin/ghq".into(), "/opt/homebrew/bin/ghq".into()]
    {
        let pb = std::path::PathBuf::from(candidate);
        if pb.exists() {
            return Some(pb);
        }
    }
    None
}

/// Variables that leak in from whatever shell ran the test and would send
/// one of the two tools somewhere the other does not go. Neither `envs()`
/// nor a fixture gitconfig can express "unset", so [`ghq_cmd`] and
/// [`scap_cmd`] remove them from every child explicitly.
const CLEARED: [&str; 10] = [
    "SCAP_CONFIG_BACKEND",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_SYSTEM",
    "XDG_CONFIG_HOME",
    "GIT_DIR",
    "GIT_CEILING_DIRECTORIES",
    "SCAP_LIST_EXCLUDE",
    "SCAP_LOG",
    "RUST_LOG",
];

/// The environment both tools are compared under.
///
/// ghq reads `GHQ_ROOT` and scap reads `SCAP_ROOT`; setting only the latter
/// -- which this helper did until W2b.2 -- left ghq resolving its default
/// `$HOME/ghq` under the fixture `HOME`, a directory no test ever creates.
/// Every comparison then held an empty ghq listing against a populated scap
/// one and failed for a reason that had nothing to do with parity
/// (ledger #16 NOTE: 11 of the file's tests). Both variables now name the
/// same fixture root, which is also why the codecommit tests no longer push
/// a `GHQ_ROOT` of their own.
fn isolated(home: &std::path::Path, root: &std::path::Path) -> Vec<(String, String)> {
    isolated_roots(home, &[root])
}

/// [`isolated`] with several roots, in the `PATH`-shaped list both tools
/// read (`filepath.SplitList` in ghq, `std::env::split_paths` in scap).
fn isolated_roots(home: &std::path::Path, roots: &[&std::path::Path]) -> Vec<(String, String)> {
    let joined = std::env::join_paths(roots).unwrap().to_string_lossy().into_owned();
    let mut env = isolated_base(home);
    env.push(("GHQ_ROOT".to_string(), joined.clone()));
    env.push(("SCAP_ROOT".to_string(), joined));
    env
}

/// [`isolated`] with no root variable at all and `body` as the fixture's
/// global gitconfig, so each tool's own configuration keys decide where its
/// roots are: `ghq.root` for one, `scap.root` for the other.
fn isolated_from_gitconfig(home: &std::path::Path, body: &str) -> Vec<(String, String)> {
    let env = isolated_base(home);
    std::fs::write(home.join(".gitconfig"), body).unwrap();
    env
}

/// Everything both children share except the roots: a `HOME` of their own,
/// an empty global gitconfig, and no system gitconfig.
fn isolated_base(home: &std::path::Path) -> Vec<(String, String)> {
    let cfg = home.join(".gitconfig");
    if !cfg.exists() {
        std::fs::File::create(&cfg).unwrap();
    }
    vec![
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ("GIT_CONFIG_GLOBAL".to_string(), cfg.to_string_lossy().into_owned()),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
    ]
}

/// The `HOME` [`isolated`] put in `env`, used as both children's working
/// directory: repository discovery reads the configuration of whatever
/// repository contains the current directory, and under `cargo test` that is
/// the scap checkout itself.
fn fixture_home(env: &[(String, String)]) -> std::path::PathBuf {
    env.iter()
        .find(|(k, _)| k == "HOME")
        .map(|(_, v)| std::path::PathBuf::from(v))
        .expect("isolated() always sets HOME")
}

fn ghq_cmd(ghq: &std::path::Path, env: &[(String, String)]) -> Command {
    let mut cmd = Command::new(ghq);
    cmd.envs(env.iter().cloned()).current_dir(fixture_home(env));
    for key in CLEARED {
        cmd.env_remove(key);
    }
    cmd
}

fn scap_cmd(env: &[(String, String)]) -> ScapCmd {
    let mut cmd = ScapCmd::cargo_bin("scap").unwrap();
    cmd.envs(env.iter().cloned()).current_dir(fixture_home(env));
    for key in CLEARED {
        cmd.env_remove(key);
    }
    cmd
}

/// Restores a directory's mode when dropped, so a panic cannot leave a
/// mode-000 directory in `$TMPDIR` for `TempDir` to choke on.
#[cfg(unix)]
struct RestoreMode<'a>(&'a std::path::Path, fs::Permissions);

#[cfg(unix)]
impl Drop for RestoreMode<'_> {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.0, self.1.clone());
    }
}

/// Put `dir` at mode 000 for as long as the returned guard lives.
///
/// `None` when mode 000 does not actually deny this user, which is the case
/// for root: the caller has nothing to compare and should skip rather than
/// fail on a true negative.
#[cfg(unix)]
fn deny_reads<'a>(dir: &'a std::path::Path, test: &str) -> Option<RestoreMode<'a>> {
    use std::os::unix::fs::PermissionsExt;

    let original = fs::metadata(dir).unwrap().permissions();
    let guard = RestoreMode(dir, original);
    fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(dir).is_ok() {
        eprintln!("{test}: skipped -- mode 000 does not deny this user (running as root?)");
        return None;
    }
    Some(guard)
}

fn init_repo(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    Command::new("git").arg("init").arg("-q").current_dir(path).output().unwrap();
}

#[test]
fn root_matches_ghq() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    compare_stdout(&ghq, &env, &["root"], "");
}

#[test]
fn list_matches_ghq() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    for rel in ["github.com/a/x", "github.com/b/y", "gitlab.com/group/sub/proj"] {
        let p = root.path().join(rel);
        std::fs::create_dir_all(&p).unwrap();
        Command::new("git").arg("init").arg("-q").current_dir(&p).output().unwrap();
    }

    for flags in
        [vec!["list"], vec!["list", "-p"], vec!["list", "--unique"], vec!["list", "-e", "x"]]
    {
        compare_stdout(&ghq, &env, &flags, "");
    }
}

#[test]
fn list_matches_ghq_with_hidden_and_gitdir_repos() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    init_repo(&root.path().join("github.com/a/.hidden"));
    let gitdir_repo = root.path().join("github.com/b/gitdir");
    std::fs::create_dir_all(&gitdir_repo).unwrap();
    let marker_dir = gitdir_repo.join(".git-real");
    std::fs::create_dir_all(&marker_dir).unwrap();
    std::fs::write(gitdir_repo.join(".git"), format!("gitdir: {}\n", marker_dir.display()))
        .unwrap();

    compare_stdout(&ghq, &env, &["list"], "hidden/gitdir: ");
}

#[test]
fn get_local_file_url_matches_ghq() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let origin = TempDir::new().unwrap();
    Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("--bare")
        .current_dir(origin.path())
        .output()
        .unwrap();

    let url = format!("file://{}", origin.path().display());

    let r_ghq = TempDir::new().unwrap();
    let env_ghq = isolated(home.path(), r_ghq.path());
    ghq_cmd(&ghq, &env_ghq).args(["get", &url]).output().unwrap();

    let r_scap = TempDir::new().unwrap();
    let env_scap = isolated(home.path(), r_scap.path());
    scap_cmd(&env_scap).args(["get", &url]).output().unwrap();

    let ghq_paths = walkdir_to_relative(r_ghq.path());
    let scap_paths = walkdir_to_relative(r_scap.path());
    assert!(!ghq_paths.is_empty(), "ghq cloned nothing, so comparing the two trees proves nothing");
    assert_eq!(ghq_paths, scap_paths, "clone tree shape diverges");
}

fn walkdir_to_relative(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            out.push(rel.clone());
            if p.is_dir() && !p.ends_with(".git") {
                walk(&p, root, out);
            }
        }
    }
    walk(root, root, &mut out);
    out.sort();
    out
}

fn compare_list(
    ghq: &std::path::Path,
    home: &std::path::Path,
    root: &std::path::Path,
    flags: &[&str],
) {
    compare_stdout(ghq, &isolated(home, root), flags, "");
}

/// Run one command under both tools in `env` and require byte-identical
/// stdout, the same exit status, and *some* output. `label` names the
/// fixture in the panic.
///
/// The non-empty check is the point. Two empty listings compare equal, so a
/// fixture that silently stopped producing anything -- which is exactly what
/// ledger #16's broken `isolated()` did to eleven tests -- passes an
/// equality assertion while proving nothing. Every caller here expects
/// output; the two tests that deliberately expect none assert `eq("")` on
/// each side by hand instead of coming through this helper.
fn compare_stdout(ghq: &std::path::Path, env: &[(String, String)], flags: &[&str], label: &str) {
    let ghq_out = ghq_cmd(ghq, env).args(flags).output().unwrap();
    let scap_out = scap_cmd(env).args(flags).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "{label}ghq {flags:?} != scap {flags:?}"
    );
    assert_eq!(ghq_out.status.code(), scap_out.status.code(), "{label}exit status differs");
    assert!(
        !ghq_out.stdout.is_empty(),
        "{label}ghq {flags:?} printed nothing: the fixture proves nothing, since an empty \
         listing matches an empty listing"
    );
}

#[test]
fn list_matches_ghq_for_direct_root_repo() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    Command::new("git").arg("init").arg("-q").current_dir(root.path()).output().unwrap();
    compare_list(&ghq, home.path(), root.path(), &["list"]);
}

#[test]
fn list_matches_ghq_for_symlinked_repo_entries() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let repo = root.path().join("github.com/a/x");
    fs::create_dir_all(&repo).unwrap();
    Command::new("git").arg("init").arg("-q").current_dir(&repo).output().unwrap();
    let link = root.path().join("link-x");
    std::os::unix::fs::symlink(&repo, &link).unwrap();
    compare_list(&ghq, home.path(), root.path(), &["list"]);
}

#[test]
fn list_matches_ghq_for_gitfile_markers() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let repo = root.path().join("github.com/a/x");
    fs::create_dir_all(&repo).unwrap();
    Command::new("git").arg("init").arg("-q").current_dir(&repo).output().unwrap();
    let git_dir = repo.join(".git");
    fs::remove_dir_all(&git_dir).unwrap();
    fs::write(&git_dir, "gitdir: /tmp/worktree\n").unwrap();
    compare_list(&ghq, home.path(), root.path(), &["list"]);
}

#[test]
fn list_detects_git_file_marker_like_ghq() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    let repo = root.path().join("github.com/a/x");
    std::fs::create_dir_all(&repo).unwrap();

    let gitdir = TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "-q", "--separate-git-dir"])
        .arg(gitdir.path())
        .current_dir(&repo)
        .output()
        .unwrap();

    compare_stdout(&ghq, &env, &["list"], ".git file marker: ");
}

fn init_repo_at(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    Command::new("git").arg("init").arg("-q").current_dir(path).output().unwrap();
}

fn init_repo_with_gitfile(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    let gitdir = path.join(".git-dir");
    std::fs::create_dir_all(&gitdir).unwrap();
    Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("--separate-git-dir")
        .arg(&gitdir)
        .current_dir(path)
        .output()
        .unwrap();
}

#[test]
fn list_matches_ghq_on_edge_fixtures() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();

    fn setup_root_repo(root: &std::path::Path) {
        init_repo_at(root);
    }
    fn setup_hidden_repo(root: &std::path::Path) {
        let p = root.join(".hidden/org/proj");
        init_repo_at(&p);
    }
    fn setup_gitfile_repo(root: &std::path::Path) {
        init_repo_with_gitfile(&root.join("github.com/a/worktree"));
    }

    type SetupCase = (&'static str, fn(&std::path::Path));
    let cases: [SetupCase; 3] = [
        ("root repo", setup_root_repo),
        ("hidden path", setup_hidden_repo),
        ("gitfile marker", setup_gitfile_repo),
    ];

    for (label, setup) in cases {
        let root_ghq = TempDir::new().unwrap();
        setup(root_ghq.path());
        let env = isolated(home.path(), root_ghq.path());
        let ghq_out = ghq_cmd(&ghq, &env).arg("list").output().unwrap();

        let root_scap = TempDir::new().unwrap();
        setup(root_scap.path());
        let env = isolated(home.path(), root_scap.path());
        let scap_out = scap_cmd(&env).arg("list").output().unwrap();

        assert!(!ghq_out.stdout.is_empty(), "{label}: ghq printed nothing to compare against");
        assert_eq!(
            String::from_utf8_lossy(&ghq_out.stdout),
            String::from_utf8_lossy(&scap_out.stdout),
            "{label} diverges",
        );
    }
}

#[cfg(unix)]
#[test]
fn list_matches_ghq_with_symlinked_repo() {
    use std::os::unix::fs::symlink;

    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let real = root.path().join("github.com/a/real");
    init_repo_at(&real);
    let link = root.path().join("github.com/a/link");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&real, &link).unwrap();
    let env = isolated(home.path(), root.path());
    compare_stdout(&ghq, &env, &["list"], "symlinked repo: ");
}

#[test]
fn codecommit_destination_matches_ghq() {
    // #12b: ghq's destination for a codecommit ref is always
    // `<root>/<region>/<repo>` (local_repository.go:76-86: `pathParts =
    // [Hostname()] + Path.split("/")`; url.go:100-106: `Path` is the bare
    // repo name with no leading slash) -- no owner/profile path segment,
    // with or without a `<profile>@` prefix in the ref. `ghq create <ref>`
    // performs no network I/O and prints exactly that destination to
    // stdout (verified against the real binary: git's own chatter is
    // routed to stderr, same as `scap create`, see src/cmd/create.rs), so
    // it is usable as an oracle without a real CodeCommit repository or
    // AWS credentials.
    //
    // Every spelling below carries an explicit `::<region>:`. A bare
    // `codecommit://<repo>` with no region resolves in ghq via
    // AWS_REGION/AWS_DEFAULT_REGION or `aws configure get region`
    // (url.go:63-97), which scap does not replicate (see the doc comment
    // on `url::finalize_codecommit`); comparing that spelling against the
    // real oracle would depend on the CI machine's AWS configuration
    // rather than on scap's own logic, so it is intentionally excluded
    // here.
    let ghq = oracle!();

    let refs = [
        "codecommit::us-east-1://my-repo",
        "codecommit::us-east-1://profile@my-repo",
        "codecommit::eu-west-2://repo_1.x-y",
        "codecommit::ap-southeast-1://user.name@my.repo-name_2",
        "codecommit::sa-east-1://a-b-c",
        "codecommit::us-gov-west-1://another_profile.name@some.repo_name-3",
    ];

    for r in refs {
        let ghq_home = TempDir::new().unwrap();
        let ghq_root = TempDir::new().unwrap();
        let ghq_env = isolated(ghq_home.path(), ghq_root.path());
        let ghq_out = ghq_cmd(&ghq, &ghq_env).args(["create", r]).output().unwrap();
        assert!(ghq_out.status.success(), "ghq create {r:?} failed: {ghq_out:?}");
        let ghq_dest = String::from_utf8_lossy(&ghq_out.stdout).trim().to_owned();
        let ghq_rel = std::path::Path::new(&ghq_dest)
            .strip_prefix(ghq_root.path())
            .unwrap_or_else(|_| panic!("ghq dest {ghq_dest:?} not under its root for {r:?}"));

        let scap_home = TempDir::new().unwrap();
        let scap_root = TempDir::new().unwrap();
        let scap_env = isolated(scap_home.path(), scap_root.path());
        let scap_out = scap_cmd(&scap_env).args(["create", r]).output().unwrap();
        assert!(scap_out.status.success(), "scap create {r:?} failed: {scap_out:?}");
        let scap_dest = String::from_utf8_lossy(&scap_out.stdout).trim().to_owned();
        let scap_rel = std::path::Path::new(&scap_dest)
            .strip_prefix(scap_root.path())
            .unwrap_or_else(|_| panic!("scap dest {scap_dest:?} not under its root for {r:?}"));

        assert_eq!(scap_rel, ghq_rel, "destination diverges for {r:?}");
    }
}

#[test]
fn codecommit_region_resolution_matches_ghq() {
    // #12c: a codecommit ref with no explicit `::<region>:` resolves its
    // region via AWS_REGION, then AWS_DEFAULT_REGION, then `aws configure
    // get region`, else fails with "You must specify a region. You can
    // also configure your region by running \"aws configure\"." on
    // stderr and exit 1 (url.go:63-97; see the doc comment on
    // `url::resolve_codecommit_region`). Both branches below are driven
    // entirely by the CHILD process's environment (never this test
    // process's), so the comparison holds regardless of what this
    // machine's own AWS_REGION/AWS_DEFAULT_REGION/`aws` CLI happen to be.
    let ghq = oracle!();
    const REF: &str = "codecommit://plain-repo";
    const GHQ_MESSAGE: &str = "You must specify a region. You can also configure your region \
                                by running \"aws configure\".";

    // Success: AWS_REGION resolves the region exactly like an explicit
    // `::<region>:` would, still with no owner/profile segment (#12b).
    {
        let ghq_home = TempDir::new().unwrap();
        let ghq_root = TempDir::new().unwrap();
        let ghq_env = isolated(ghq_home.path(), ghq_root.path());
        let ghq_out = ghq_cmd(&ghq, &ghq_env)
            .args(["create", REF])
            .env_remove("AWS_DEFAULT_REGION")
            .env("AWS_REGION", "us-west-2")
            .output()
            .unwrap();
        assert!(ghq_out.status.success(), "ghq create {REF:?} failed: {ghq_out:?}");
        let ghq_dest = String::from_utf8_lossy(&ghq_out.stdout).trim().to_owned();
        let ghq_rel = std::path::Path::new(&ghq_dest)
            .strip_prefix(ghq_root.path())
            .unwrap_or_else(|_| panic!("ghq dest {ghq_dest:?} not under its root"));

        let scap_home = TempDir::new().unwrap();
        let scap_root = TempDir::new().unwrap();
        let scap_env = isolated(scap_home.path(), scap_root.path());
        let scap_out = scap_cmd(&scap_env)
            .args(["create", REF])
            .env_remove("AWS_DEFAULT_REGION")
            .env("AWS_REGION", "us-west-2")
            .output()
            .unwrap();
        assert!(scap_out.status.success(), "scap create {REF:?} failed: {scap_out:?}");
        let scap_dest = String::from_utf8_lossy(&scap_out.stdout).trim().to_owned();
        let scap_rel = std::path::Path::new(&scap_dest)
            .strip_prefix(scap_root.path())
            .unwrap_or_else(|_| panic!("scap dest {scap_dest:?} not under its root"));

        assert_eq!(scap_rel, ghq_rel, "AWS_REGION-resolved destination diverges from ghq");
    }

    // Failure: no region resolvable anywhere -- both env vars cleared and
    // PATH pointed at an empty directory, so no `aws` (or anything else)
    // can be found. Both tools must exit non-zero with ghq's own message.
    {
        let empty_path = empty_path_dir();

        let ghq_home = TempDir::new().unwrap();
        let ghq_root = TempDir::new().unwrap();
        let ghq_env = isolated(ghq_home.path(), ghq_root.path());
        let ghq_out = ghq_cmd(&ghq, &ghq_env)
            .args(["create", REF])
            .env_remove("AWS_REGION")
            .env_remove("AWS_DEFAULT_REGION")
            .env("PATH", empty_path.path())
            .output()
            .unwrap();
        assert!(!ghq_out.status.success(), "ghq create {REF:?} should have failed: {ghq_out:?}");
        let ghq_err = String::from_utf8_lossy(&ghq_out.stderr).into_owned();
        assert!(
            ghq_err.contains(GHQ_MESSAGE),
            "ghq's own stderr does not contain its own message: {ghq_err:?}"
        );

        let scap_home = TempDir::new().unwrap();
        let scap_root = TempDir::new().unwrap();
        let scap_env = isolated(scap_home.path(), scap_root.path());
        let scap_out = scap_cmd(&scap_env)
            .args(["create", REF])
            .env_remove("AWS_REGION")
            .env_remove("AWS_DEFAULT_REGION")
            .env("PATH", empty_path.path())
            .output()
            .unwrap();
        assert!(!scap_out.status.success(), "scap create {REF:?} should have failed: {scap_out:?}");
        let scap_err = String::from_utf8_lossy(&scap_out.stderr).into_owned();
        assert!(
            scap_err.contains(GHQ_MESSAGE),
            "scap stderr does not contain ghq's message: {scap_err:?}"
        );
    }
}

#[test]
fn list_prunes_nested_repo_matches_ghq() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    for rel in ["github.com/a/x", "github.com/a/x/sub/inner"] {
        let p = root.path().join(rel);
        std::fs::create_dir_all(&p).unwrap();
        Command::new("git").arg("init").arg("-q").current_dir(&p).output().unwrap();
    }

    compare_stdout(&ghq, &env, &["list"], "nested repo pruning: ");
}

/// AC-8b: ADR-9 rule (v) against the real oracle.
///
/// W2b.1 landed this with an environment built by hand, because the shared
/// `isolated()` helper set no `GHQ_ROOT` and so could not point the two
/// tools at one root. W2b.2 repaired the helper, and this test now uses it
/// like every other.
#[test]
#[cfg(unix)]
fn unreadable_directory_is_skipped_like_ghq() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    for rel in ["github.com/a/x", "github.com/b/y"] {
        init_repo(&root.path().join(rel));
    }
    let locked = root.path().join("locked");
    init_repo(&locked.join("deep/hidden-repo"));

    let Some(_guard) = deny_reads(&locked, "unreadable_directory_is_skipped_like_ghq") else {
        return;
    };

    let ghq_out = ghq_cmd(&ghq, &env).arg("list").output().unwrap();
    let scap_out = scap_cmd(&env).arg("list").output().unwrap();

    assert!(
        !ghq_out.stdout.is_empty(),
        "ghq listed nothing: the readable half of the tree must still appear"
    );
    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "ghq list != scap list across an unreadable directory"
    );
    assert_eq!(ghq_out.status.code(), scap_out.status.code(), "exit status differs");
    assert_eq!(scap_out.status.code(), Some(0));

    // Both name the skipped path on stderr; only the decoration differs
    // (ghq prints a coloured `warning` prefix, scap a `tracing` WARN line).
    for (who, stderr) in [("ghq", &ghq_out.stderr), ("scap", &scap_out.stderr)] {
        let stderr = String::from_utf8_lossy(stderr);
        assert!(stderr.contains("Permission denied"), "{who} stderr: {stderr}");
        assert!(stderr.contains(&locked.display().to_string()), "{who} stderr: {stderr}");
    }
}

// -- ADR-9 rules (iii), (iv), (vi) and (vii) against the oracle -----------
//
// The W0.4 probe (docs/benchmarks/2026-08-28-oracle-probe.md) answered these
// once, by hand, into a table. These re-ask the live binary on every run, so
// a ghq upgrade that changes an answer fails the suite instead of silently
// invalidating the table.

/// ADR-9 rule (iii), W0.4 cases 2, 3, 4 and 14a: the ways a symlink is
/// *not* a repository. ghq never recurses through one (walker.go:85-90), so
/// a link to an ordinary directory hides everything under it; a link that
/// resolves to nothing at all is simply not an entry.
///
/// The `link-to-plain-dir` arm is the divergence W2b.2 closed: until then
/// scap followed the link and printed `link-to-plain-dir/nested/repo`.
#[cfg(unix)]
#[test]
fn list_matches_ghq_for_symlinks_that_are_not_repositories() {
    use std::os::unix::fs::symlink;

    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();

    // A control, so "both printed nothing" cannot pass for the wrong reason.
    init_repo(&root.path().join("github.com/a/x"));

    init_repo(&outside.path().join("nested/repo"));
    symlink(outside.path(), root.path().join("link-to-plain-dir")).unwrap();
    symlink(root.path().join("nowhere"), root.path().join("dangling")).unwrap();
    symlink("loop-b", root.path().join("loop-a")).unwrap();
    symlink("loop-a", root.path().join("loop-b")).unwrap();

    compare_stdout(&ghq, &isolated(home.path(), root.path()), &["list"], "non-repo symlinks: ");
}

/// ADR-9 rules (ii) and (iii), W0.4 case 7: the `.git` suffix test reads the
/// entry's own name. A link to a bare repository is listed only when the
/// *link* is the one named `*.git`, whatever its target is called.
#[cfg(unix)]
#[test]
fn list_matches_ghq_for_a_symlinked_bare_repository() {
    use std::os::unix::fs::symlink;

    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();

    let upstream = store.path().join("upstream.git");
    std::fs::create_dir_all(&upstream).unwrap();
    Command::new("git").args(["init", "-q", "--bare"]).current_dir(&upstream).output().unwrap();

    symlink(&upstream, root.path().join("link-to-bare")).unwrap();
    symlink(&upstream, root.path().join("named.git")).unwrap();

    compare_stdout(&ghq, &isolated(home.path(), root.path()), &["list"], "symlinked bare: ");
}

/// ADR-9 rule (iv), W0.4 cases 5 and 6: a `.git` that is a symlink has to
/// resolve. Dangling, and the directory holding it is ordinary; pointing at
/// a real git directory, and it is a repository.
#[cfg(unix)]
#[test]
fn list_matches_ghq_for_dot_git_symlinks() {
    use std::os::unix::fs::symlink;

    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();

    let dangling = root.path().join("dangling-git");
    std::fs::create_dir_all(&dangling).unwrap();
    symlink(root.path().join("nowhere"), dangling.join(".git")).unwrap();

    let donor = root.path().join("github.com/a/donor");
    init_repo(&donor);
    let borrowed = root.path().join("github.com/a/borrowed");
    std::fs::create_dir_all(&borrowed).unwrap();
    symlink(donor.join(".git"), borrowed.join(".git")).unwrap();

    compare_stdout(&ghq, &isolated(home.path(), root.path()), &["list"], ".git symlinks: ");
}

/// ADR-9 rule (vii): neither tool de-duplicates across roots. A relative
/// path present in three roots is printed three times, `-p` distinguishes
/// them, and `--unique` is the only flag that collapses them -- which is
/// exactly why the duplicates have to survive to reach it
/// (cmd_list.go:78-110).
#[test]
fn list_matches_ghq_across_duplicated_roots() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let roots: Vec<TempDir> = (0..3).map(|_| TempDir::new().unwrap()).collect();
    for r in &roots {
        init_repo(&r.path().join("github.com/a/dup"));
    }
    init_repo(&roots[1].path().join("github.com/b/only"));

    let paths: Vec<&std::path::Path> = roots.iter().map(TempDir::path).collect();
    let env = isolated_roots(home.path(), &paths);
    for flags in [vec!["list"], vec!["list", "-p"], vec!["list", "--unique"]] {
        compare_stdout(&ghq, &env, &flags, "duplicated roots: ");
    }
}

/// ADR-9 rule (vii), the `root --all` half: a url-scoped section adds a root
/// that the bare `root` never prints. scap keys on `scap.root` where ghq
/// keys on `ghq.root`, so the fixture carries both spellings of the same two
/// values and the outputs still have to match line for line.
#[test]
fn root_all_matches_ghq_with_url_scoped_sections() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let default = TempDir::new().unwrap();
    let scoped = TempDir::new().unwrap();

    let body = format!(
        "[ghq]\n\troot = {d}\n[ghq \"https://example.com/\"]\n\troot = {s}\n\
         [scap]\n\troot = {d}\n[scap \"https://example.com/\"]\n\troot = {s}\n",
        d = default.path().display(),
        s = scoped.path().display(),
    );
    let env = isolated_from_gitconfig(home.path(), &body);

    compare_stdout(&ghq, &env, &["root"], "url-scoped: ");
    compare_stdout(&ghq, &env, &["root", "--all"], "url-scoped: ");
}

/// ADR-9 rule (vi), W0.4 case 12: a root that does not exist is not an
/// error and not a warning. Both tools print nothing, on either stream, and
/// exit 0.
#[test]
fn list_matches_ghq_for_a_non_existent_root() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let parent = TempDir::new().unwrap();
    let missing = parent.path().join("never-created");
    let env = isolated(home.path(), &missing);

    let ghq_out = ghq_cmd(&ghq, &env).arg("list").output().unwrap();
    let scap_out = scap_cmd(&env).arg("list").output().unwrap();

    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "stdout differs for a non-existent root"
    );
    for (who, out) in [("ghq", &ghq_out), ("scap", &scap_out)] {
        assert_eq!(out.status.code(), Some(0), "{who} exited {:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{who} printed something");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{who} warned about a missing root");
    }
}

/// ADR-9 rule (vi) second case, W0.4 case 13a: a root that exists but cannot
/// be read is skipped *with* a warning -- the listing it hides is not one
/// entry but everything, so silence would make the empty output look
/// authoritative. Only the decoration differs (ghq colours a `warning`
/// prefix, scap emits a `tracing` WARN line), so stdout is compared byte for
/// byte and stderr by content.
#[cfg(unix)]
#[test]
fn list_matches_ghq_for_an_unreadable_root() {
    let ghq = oracle!();
    let home = TempDir::new().unwrap();
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("locked");
    init_repo(&root.join("github.com/a/hidden"));
    let env = isolated(home.path(), &root);

    let Some(_guard) = deny_reads(&root, "list_matches_ghq_for_an_unreadable_root") else {
        return;
    };

    let ghq_out = ghq_cmd(&ghq, &env).arg("list").output().unwrap();
    let scap_out = scap_cmd(&env).arg("list").output().unwrap();

    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "stdout differs for an unreadable root"
    );
    for (who, out) in [("ghq", &ghq_out), ("scap", &scap_out)] {
        assert_eq!(out.status.code(), Some(0), "{who} exited {:?}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{who} printed something");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("Permission denied"), "{who} stderr: {stderr}");
        assert!(stderr.contains(&root.display().to_string()), "{who} stderr: {stderr}");
    }
}
