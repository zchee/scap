use std::fs;
use std::process::Command;

use assert_cmd::Command as ScapCmd;
use tempfile::TempDir;

fn ghq_binary() -> Option<std::path::PathBuf> {
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
