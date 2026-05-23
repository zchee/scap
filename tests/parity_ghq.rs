use assert_cmd::Command as ScapCmd;
use std::process::Command;
use tempfile::TempDir;

fn ghq_binary() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("GHQ_BINARY") {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    for candidate in [
        format!("{home}/go/bin/ghq"),
        "/usr/local/bin/ghq".into(),
        "/opt/homebrew/bin/ghq".into(),
    ] {
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
        (
            "GIT_CONFIG_GLOBAL".to_string(),
            cfg.to_string_lossy().into_owned(),
        ),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        ("GHQ_ROOT".to_string(), root.to_string_lossy().into_owned()),
    ]
}

#[test]
#[ignore]
fn root_matches_ghq() {
    let Some(ghq) = ghq_binary() else { return };
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let env = isolated(home.path(), root.path());

    let ghq_out = Command::new(&ghq)
        .arg("root")
        .envs(env.iter().cloned())
        .output()
        .unwrap();
    let scap_out = ScapCmd::cargo_bin("scap")
        .unwrap()
        .arg("root")
        .envs(env.iter().cloned())
        .output()
        .unwrap();
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

    for rel in [
        "github.com/a/x",
        "github.com/b/y",
        "gitlab.com/group/sub/proj",
    ] {
        let p = root.path().join(rel);
        std::fs::create_dir_all(&p).unwrap();
        Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&p)
            .output()
            .unwrap();
    }

    for flags in [
        vec!["list"],
        vec!["list", "-p"],
        vec!["list", "--unique"],
        vec!["list", "-e", "x"],
    ] {
        let ghq_out = Command::new(&ghq)
            .args(&flags)
            .envs(env.iter().cloned())
            .output()
            .unwrap();
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
    Command::new(&ghq)
        .args(["get", &url])
        .envs(env_ghq.iter().cloned())
        .output()
        .unwrap();

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
