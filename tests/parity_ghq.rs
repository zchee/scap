use std::fs;
use std::process::Command;

use assert_cmd::Command as ScapCmd;
use tempfile::TempDir;

mod support;

use support::empty_path_dir;

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

fn isolated(home: &std::path::Path, root: &std::path::Path) -> Vec<(String, String)> {
    let cfg = home.join(".gitconfig");
    if !cfg.exists() {
        std::fs::File::create(&cfg).unwrap();
    }
    vec![
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ("GIT_CONFIG_GLOBAL".to_string(), cfg.to_string_lossy().into_owned()),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        ("SCAP_ROOT".to_string(), root.to_string_lossy().into_owned()),
    ]
}

fn init_repo(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    Command::new("git").arg("init").arg("-q").current_dir(path).output().unwrap();
}

#[test]
#[ignore]
fn root_matches_ghq() {
    let Some(ghq) = ghq_binary() else { return };
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    let ghq_out = Command::new(&ghq).arg("root").envs(env.iter().cloned()).output().unwrap();
    let scap_out =
        ScapCmd::cargo_bin("scap").unwrap().arg("root").envs(env.iter().cloned()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "ghq root != scap root"
    );
}

#[test]
#[ignore]
fn list_matches_ghq() {
    let Some(ghq) = ghq_binary() else { return };
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
        let ghq_out = Command::new(&ghq).args(&flags).envs(env.iter().cloned()).output().unwrap();
        let scap_out = ScapCmd::cargo_bin("scap")
            .unwrap()
            .args(&flags)
            .envs(env.iter().cloned())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ghq_out.stdout),
            String::from_utf8_lossy(&scap_out.stdout),
            "ghq {flags:?} != scap {flags:?}"
        );
    }
}

#[test]
#[ignore]
fn list_matches_ghq_with_hidden_and_gitdir_repos() {
    let Some(ghq) = ghq_binary() else { return };
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

    let ghq_out = Command::new(&ghq).arg("list").envs(env.iter().cloned()).output().unwrap();
    let scap_out =
        ScapCmd::cargo_bin("scap").unwrap().arg("list").envs(env.iter().cloned()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "ghq list != scap list for hidden/gitdir coverage"
    );
}

#[test]
#[ignore]
fn get_local_file_url_matches_ghq() {
    let Some(ghq) = ghq_binary() else { return };
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
    Command::new(&ghq).args(["get", &url]).envs(env_ghq.iter().cloned()).output().unwrap();

    let r_scap = TempDir::new().unwrap();
    let env_scap = isolated(home.path(), r_scap.path());
    ScapCmd::cargo_bin("scap")
        .unwrap()
        .args(["get", &url])
        .envs(env_scap.iter().cloned())
        .output()
        .unwrap();

    let ghq_paths = walkdir_to_relative(r_ghq.path());
    let scap_paths = walkdir_to_relative(r_scap.path());
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
    let env = isolated(home, root);
    let ghq_out = Command::new(ghq).args(flags).envs(env.iter().cloned()).output().unwrap();
    let scap_out =
        ScapCmd::cargo_bin("scap").unwrap().args(flags).envs(env.iter().cloned()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "ghq {flags:?} != scap {flags:?}"
    );
}

#[test]
#[ignore]
fn list_matches_ghq_for_direct_root_repo() {
    let Some(ghq) = ghq_binary() else { return };
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    Command::new("git").arg("init").arg("-q").current_dir(root.path()).output().unwrap();
    compare_list(&ghq, home.path(), root.path(), &["list"]);
}

#[test]
#[ignore]
fn list_matches_ghq_for_symlinked_repo_entries() {
    let Some(ghq) = ghq_binary() else { return };
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
#[ignore]
fn list_matches_ghq_for_gitfile_markers() {
    let Some(ghq) = ghq_binary() else { return };
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
#[ignore]
fn list_detects_git_file_marker_like_ghq() {
    let Some(ghq) = ghq_binary() else { return };
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

    let ghq_out = Command::new(&ghq).arg("list").envs(env.iter().cloned()).output().unwrap();
    let scap_out =
        ScapCmd::cargo_bin("scap").unwrap().arg("list").envs(env.iter().cloned()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "ghq list != scap list for .git file marker repo"
    );
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
#[ignore]
fn list_matches_ghq_on_edge_fixtures() {
    let Some(ghq) = ghq_binary() else { return };
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
        let ghq_out = Command::new(&ghq).arg("list").envs(env.iter().cloned()).output().unwrap();

        let root_scap = TempDir::new().unwrap();
        setup(root_scap.path());
        let env = isolated(home.path(), root_scap.path());
        let scap_out = ScapCmd::cargo_bin("scap")
            .unwrap()
            .arg("list")
            .envs(env.iter().cloned())
            .output()
            .unwrap();

        assert_eq!(
            String::from_utf8_lossy(&ghq_out.stdout),
            String::from_utf8_lossy(&scap_out.stdout),
            "{label} diverges",
        );
    }
}

#[cfg(unix)]
#[test]
#[ignore]
fn list_matches_ghq_with_symlinked_repo() {
    use std::os::unix::fs::symlink;

    let Some(ghq) = ghq_binary() else { return };
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let real = root.path().join("github.com/a/real");
    init_repo_at(&real);
    let link = root.path().join("github.com/a/link");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&real, &link).unwrap();
    let env = isolated(home.path(), root.path());
    let ghq_out = Command::new(&ghq).arg("list").envs(env.iter().cloned()).output().unwrap();
    let scap_out =
        ScapCmd::cargo_bin("scap").unwrap().arg("list").envs(env.iter().cloned()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "symlinked repo diverges",
    );
}

#[test]
#[ignore]
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
    let Some(ghq) = ghq_binary() else { return };

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
        let mut ghq_env = isolated(ghq_home.path(), ghq_root.path());
        ghq_env.push(("GHQ_ROOT".to_string(), ghq_root.path().to_string_lossy().into_owned()));
        let ghq_out =
            Command::new(&ghq).args(["create", r]).envs(ghq_env.iter().cloned()).output().unwrap();
        assert!(ghq_out.status.success(), "ghq create {r:?} failed: {ghq_out:?}");
        let ghq_dest = String::from_utf8_lossy(&ghq_out.stdout).trim().to_owned();
        let ghq_rel = std::path::Path::new(&ghq_dest)
            .strip_prefix(ghq_root.path())
            .unwrap_or_else(|_| panic!("ghq dest {ghq_dest:?} not under its root for {r:?}"));

        let scap_home = TempDir::new().unwrap();
        let scap_root = TempDir::new().unwrap();
        let scap_env = isolated(scap_home.path(), scap_root.path());
        let scap_out = ScapCmd::cargo_bin("scap")
            .unwrap()
            .args(["create", r])
            .envs(scap_env.iter().cloned())
            .output()
            .unwrap();
        assert!(scap_out.status.success(), "scap create {r:?} failed: {scap_out:?}");
        let scap_dest = String::from_utf8_lossy(&scap_out.stdout).trim().to_owned();
        let scap_rel = std::path::Path::new(&scap_dest)
            .strip_prefix(scap_root.path())
            .unwrap_or_else(|_| panic!("scap dest {scap_dest:?} not under its root for {r:?}"));

        assert_eq!(scap_rel, ghq_rel, "destination diverges for {r:?}");
    }
}

#[test]
#[ignore]
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
    let Some(ghq) = ghq_binary() else { return };
    const REF: &str = "codecommit://plain-repo";
    const GHQ_MESSAGE: &str = "You must specify a region. You can also configure your region \
                                by running \"aws configure\".";

    // Success: AWS_REGION resolves the region exactly like an explicit
    // `::<region>:` would, still with no owner/profile segment (#12b).
    {
        let ghq_home = TempDir::new().unwrap();
        let ghq_root = TempDir::new().unwrap();
        let mut ghq_env = isolated(ghq_home.path(), ghq_root.path());
        ghq_env.push(("GHQ_ROOT".to_string(), ghq_root.path().to_string_lossy().into_owned()));
        let ghq_out = Command::new(&ghq)
            .args(["create", REF])
            .env_remove("AWS_DEFAULT_REGION")
            .env("AWS_REGION", "us-west-2")
            .envs(ghq_env.iter().cloned())
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
        let scap_out = ScapCmd::cargo_bin("scap")
            .unwrap()
            .args(["create", REF])
            .env_remove("AWS_DEFAULT_REGION")
            .env("AWS_REGION", "us-west-2")
            .envs(scap_env.iter().cloned())
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
        let mut ghq_env = isolated(ghq_home.path(), ghq_root.path());
        ghq_env.push(("GHQ_ROOT".to_string(), ghq_root.path().to_string_lossy().into_owned()));
        let ghq_out = Command::new(&ghq)
            .args(["create", REF])
            .env_remove("AWS_REGION")
            .env_remove("AWS_DEFAULT_REGION")
            .env("PATH", empty_path.path())
            .envs(ghq_env.iter().cloned())
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
        let scap_out = ScapCmd::cargo_bin("scap")
            .unwrap()
            .args(["create", REF])
            .env_remove("AWS_REGION")
            .env_remove("AWS_DEFAULT_REGION")
            .env("PATH", empty_path.path())
            .envs(scap_env.iter().cloned())
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
#[ignore]
fn list_prunes_nested_repo_matches_ghq() {
    let Some(ghq) = ghq_binary() else { return };
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    for rel in ["github.com/a/x", "github.com/a/x/sub/inner"] {
        let p = root.path().join(rel);
        std::fs::create_dir_all(&p).unwrap();
        Command::new("git").arg("init").arg("-q").current_dir(&p).output().unwrap();
    }

    let ghq_out = Command::new(&ghq).arg("list").envs(env.iter().cloned()).output().unwrap();
    let scap_out =
        ScapCmd::cargo_bin("scap").unwrap().arg("list").envs(env.iter().cloned()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ghq_out.stdout),
        String::from_utf8_lossy(&scap_out.stdout),
        "nested repo pruning diverges"
    );
}
