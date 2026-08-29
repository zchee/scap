//! Unit tests for the explicit source list (ADR-8, oracle iv).

use tempfile::TempDir;

use super::*;

/// The temp directory's physical spelling: gix-discover matches ceiling
/// directories against the symlink-resolved path it traverses, and on macOS
/// `TempDir` hands back the `/var` -> `/private/var` symlinked spelling.
fn scratch() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize the tempdir");
    (tmp, root)
}

fn touch(dir: &Path, rel: &str) -> PathBuf {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir -p");
    }
    std::fs::write(&path, "").expect("touch");
    path
}

#[test]
fn default_system_candidates_are_the_four_prefix_locations() {
    assert_eq!(
        default_system_candidates(),
        [
            PathBuf::from("/etc/gitconfig"),
            PathBuf::from("/usr/local/etc/gitconfig"),
            PathBuf::from("/opt/homebrew/etc/gitconfig"),
            PathBuf::from("/opt/local/etc/gitconfig"),
        ]
    );
}

#[test]
fn probe_system_config_reports_zero_one_and_two_matches() {
    let tmp = TempDir::new().expect("tempdir");
    let absent = tmp.path().join("nowhere/gitconfig");
    let first = touch(tmp.path(), "etc-a/gitconfig");
    let second = touch(tmp.path(), "etc-b/gitconfig");
    let a_directory = tmp.path().join("etc-a");

    assert!(probe_system_config(std::slice::from_ref(&absent)).is_empty(), "zero matches");
    assert_eq!(
        probe_system_config(&[absent.clone(), first.clone()]),
        std::slice::from_ref(&first),
        "one match"
    );
    assert_eq!(
        probe_system_config(&[first.clone(), absent, second.clone()]),
        [first, second],
        "two matches, in the order given"
    );
    assert!(probe_system_config(&[a_directory]).is_empty(), "a directory is not a config file");
}

#[test]
fn enumerate_skips_the_system_probe_under_git_config_nosystem() {
    let tmp = TempDir::new().expect("tempdir");
    let system = touch(tmp.path(), "etc/gitconfig");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).expect("mkdir home");

    let base = Env {
        home: Some(home.clone()),
        cwd: Some(tmp.path().to_path_buf()),
        system_probe_candidates: vec![system.clone()],
        ..Default::default()
    };

    let probing = enumerate(&base);
    assert_eq!(probing.system_probe_matches, 1);
    assert!(probing.files.iter().any(|(path, _)| *path == system));

    for truthy_value in ["1", "true", "yes", "on", "anything-else"] {
        let env = Env { git_config_nosystem: Some(truthy_value.into()), ..base.clone() };
        let list = enumerate(&env);
        assert_eq!(list.system_probe_matches, 0, "value: {truthy_value}");
        assert!(!list.files.iter().any(|(path, _)| *path == system), "value: {truthy_value}");
    }

    for falsey_value in ["", "0", "false", "no", "off"] {
        let env = Env { git_config_nosystem: Some(falsey_value.into()), ..base.clone() };
        assert_eq!(enumerate(&env).system_probe_matches, 1, "value: {falsey_value}");
    }
}

#[test]
fn enumerate_prefers_git_config_system_over_the_probe() {
    let tmp = TempDir::new().expect("tempdir");
    let probed = touch(tmp.path(), "etc/gitconfig");
    let explicit = touch(tmp.path(), "explicit/gitconfig");

    let env = Env {
        git_config_system: Some(explicit.clone()),
        system_probe_candidates: vec![probed.clone()],
        ..Default::default()
    };
    let list = enumerate(&env);

    assert_eq!(list.system_probe_matches, 0, "an explicit file means nothing is ambiguous");
    assert!(list.files.iter().any(|(path, _)| *path == explicit));
    assert!(!list.files.iter().any(|(path, _)| *path == probed));
}

#[test]
fn enumerate_appends_config_worktree_only_when_the_extension_is_enabled() {
    let tmp = TempDir::new().expect("tempdir");
    let git_dir = tmp.path().join("repo/.git");
    std::fs::create_dir_all(&git_dir).expect("mkdir .git");
    std::fs::write(git_dir.join("config"), "").expect("write local config");
    std::fs::write(git_dir.join("config.worktree"), "").expect("write config.worktree");

    let env = Env { git_dir: Some(git_dir.clone()), ..Default::default() };
    let without = enumerate(&env);
    assert_eq!(without.git_dir.as_deref(), Some(git_dir.as_path()));
    assert_eq!(
        without.files.iter().map(|(path, _)| path).collect::<Vec<_>>(),
        [&git_dir.join("config")],
        "git ignores config.worktree unless extensions.worktreeConfig is on"
    );

    std::fs::write(git_dir.join("config"), "[extensions]\n\tworktreeConfig = true\n")
        .expect("enable the extension");
    let with = enumerate(&env);
    assert_eq!(
        with.files.iter().map(|(path, _)| path).collect::<Vec<_>>(),
        [&git_dir.join("config"), &git_dir.join("config.worktree")]
    );
}

#[test]
fn enumerate_reads_the_common_dir_config_from_a_linked_worktree() {
    // A linked worktree's git dir carries a `commondir` file; git reads the
    // repository-level configuration from the directory it names, not from
    // the worktree-private `config` beside it.
    let (_tmp, tmp) = scratch();
    let main_git = tmp.join("main/.git");
    let linked = main_git.join("worktrees/wt");
    std::fs::create_dir_all(&linked).expect("mkdir worktrees/wt");
    std::fs::write(main_git.join("config"), "").expect("common config");
    std::fs::write(linked.join("config"), "").expect("worktree-private config");
    std::fs::write(linked.join("commondir"), "../..\n").expect("commondir");

    let list = enumerate(&Env { git_dir: Some(linked.clone()), ..Default::default() });

    // `commondir` names the directory relatively, so the source keeps the
    // `../..` spelling git itself joins; popping it lexically would be wrong
    // wherever a component is a symlink. Compare the file it names.
    let local: Vec<PathBuf> = list
        .files
        .iter()
        .map(|(path, _)| std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .collect();
    let expected = std::fs::canonicalize(main_git.join("config")).expect("common config exists");
    assert_eq!(local, [expected], "got {local:?}");
    let private = std::fs::canonicalize(linked.join("config")).expect("private config exists");
    assert!(
        !local.contains(&private),
        "the worktree-private config file is not a configuration source"
    );
}

#[test]
fn enumerate_treats_a_main_worktree_as_its_own_common_dir() {
    let (_tmp, tmp) = scratch();
    let git_dir = tmp.join("repo/.git");
    std::fs::create_dir_all(&git_dir).expect("mkdir .git");
    std::fs::write(git_dir.join("config"), "").expect("config");

    let list = enumerate(&Env { git_dir: Some(git_dir.clone()), ..Default::default() });

    assert_eq!(
        list.files.iter().map(|(path, _)| path).collect::<Vec<_>>(),
        [&git_dir.join("config")]
    );
}

#[test]
fn enumerate_discovers_the_repository_containing_the_cwd() {
    let (_tmp, tmp) = scratch();
    let work = tmp.join("work");
    let deep = work.join("a/b/c");
    std::fs::create_dir_all(&deep).expect("mkdir deep");
    std::fs::create_dir_all(work.join(".git/objects")).expect("mkdir objects");
    std::fs::create_dir_all(work.join(".git/refs")).expect("mkdir refs");
    std::fs::write(work.join(".git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");
    std::fs::write(work.join(".git/config"), "").expect("config");

    let found = enumerate(&Env { cwd: Some(deep.clone()), ..Default::default() });
    assert!(found.git_dir.is_some(), "discovery must find the repository above the cwd");

    // `GIT_CEILING_DIRECTORIES` stops the ascent before the repository.
    let ceiled = enumerate(&Env {
        cwd: Some(deep.clone()),
        git_ceiling_directories: Some(work.join("a").into_os_string()),
        ..Default::default()
    });
    assert!(ceiled.git_dir.is_none(), "a ceiling below the repository must hide it");

    let outside = enumerate(&Env { cwd: Some(tmp.clone()), ..Default::default() });
    assert!(outside.git_dir.is_none(), "no repository above a bare temp dir");
}

#[test]
fn a_ceiling_in_a_symlinked_spelling_is_not_honoured() {
    // Registered divergence: gix matches ceiling directories against the
    // symlink-resolved ancestor chain it traverses, so a ceiling written
    // through a symlink does not stop the ascent, where git's literal
    // prefix match would. Asserted rather than worked around, so a future
    // gix release that changes it fails here loudly.
    let (_tmp, tmp) = scratch();
    let work = tmp.join("work");
    let deep = work.join("a/b");
    std::fs::create_dir_all(&deep).expect("mkdir deep");
    std::fs::create_dir_all(work.join(".git/objects")).expect("mkdir objects");
    std::fs::create_dir_all(work.join(".git/refs")).expect("mkdir refs");
    std::fs::write(work.join(".git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");
    std::fs::write(work.join(".git/config"), "").expect("config");
    std::os::unix::fs::symlink(&work, tmp.join("link")).expect("symlink to the work tree");

    let physical = enumerate(&Env {
        cwd: Some(deep.clone()),
        git_ceiling_directories: Some(work.join("a").into_os_string()),
        ..Default::default()
    });
    assert!(physical.git_dir.is_none(), "the physical spelling stops the ascent");

    let symlinked = enumerate(&Env {
        cwd: Some(deep),
        git_ceiling_directories: Some(tmp.join("link/a").into_os_string()),
        ..Default::default()
    });
    assert!(
        symlinked.git_dir.is_some(),
        "the symlinked spelling is ignored -- the documented divergence"
    );
}

#[test]
fn resolve_git_program_walks_the_injected_path() {
    let tmp = TempDir::new().expect("tempdir");
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).expect("mkdir empty");

    let missing = Env { path: Some(empty.clone().into_os_string()), ..Default::default() };
    assert!(resolve_git_program(&missing).is_none(), "an empty PATH finds no git");

    let real = Env { path: std::env::var_os("PATH"), ..Default::default() };
    let found = resolve_git_program(&real).expect("a real git on the test PATH");
    assert!(found.is_absolute() && found.ends_with("git"));

    assert!(resolve_git_program(&Env::default()).is_none(), "no PATH at all finds no git");
}
