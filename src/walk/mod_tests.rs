use std::ffi::OsStr;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{fs, io};

use tracing_subscriber::fmt::MakeWriter;

use super::*;

/// Every rule is asserted under both `.git` detection strategies. They are
/// required to produce identical repository sets — only the amount of I/O
/// differs (deviation D-6) — so a rule that held under one and not the other
/// would be a divergence the Phase-3 gate could freeze into the default.
const BOTH: [DetectStrategy; 2] = [DetectStrategy::OpenScan, DetectStrategy::StatFirst];

fn options(detect: DetectStrategy, exclude: &[&str]) -> WalkOptions {
    WalkOptions {
        threads: DEFAULT_THREADS,
        exclude: exclude.iter().copied().map(Pattern::new).collect(),
        detect,
    }
}

/// Creates `<root>/<rel>` as a repository: a directory holding a `.git`
/// directory, which is what `git init` leaves behind.
fn repo(root: &Path, rel: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.join(".git")).expect("create repository fixture");
    path
}

fn dir(root: &Path, rel: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(&path).expect("create directory fixture");
    path
}

/// Every repository the walk found, in byte order. The walk deliberately
/// emits in scheduling order, so a test that wants a stable sequence sorts
/// one here exactly as `list` will sort the concatenation of every root.
fn sorted(listing: &RootListing) -> Vec<&[u8]> {
    let mut repos: Vec<&[u8]> = listing.repos().collect();
    repos.sort_unstable();
    repos
}

fn walk(root: &Path, opts: &WalkOptions) -> Vec<String> {
    let listing = walk_root(root, opts).expect("walk_root");
    assert_eq!(
        listing.repos().len(),
        listing.len(),
        "the walk-order iterator must agree with the count"
    );
    sorted(&listing).iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect()
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// Restores a mode-000 fixture so the temporary directory can be removed.
struct ModeGuard(PathBuf);

impl Drop for ModeGuard {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

/// Captures what a subscriber wrote, so a test can assert on a warning's
/// exact text. Same shape as `src/lib_tests.rs`'s, which exists because
/// `tracing_subscriber`'s `TestWriter` writes to stdout and never hands the
/// bytes back to the test body.
#[derive(Clone)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

struct BufWriterGuard(Arc<Mutex<Vec<u8>>>);

impl io::Write for BufWriterGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'w> MakeWriter<'w> for BufWriter {
    type Writer = BufWriterGuard;

    fn make_writer(&'w self) -> Self::Writer {
        BufWriterGuard(self.0.clone())
    }
}

/// Runs `body` with a WARN-level subscriber installed and returns everything
/// it logged.
fn captured_warnings(body: impl FnOnce()) -> String {
    let writer = BufWriter(Arc::new(Mutex::new(Vec::new())));
    tracing::subscriber::with_default(crate::build_subscriber(None, writer.clone()), body);
    let bytes = writer.0.lock().unwrap().clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Repository detection (rules i, ii, iv)
// ---------------------------------------------------------------------------

#[test]
fn walk_root_finds_every_repository_under_the_root() {
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "github.com/zchee/scap");
    repo(root, "github.com/zchee/ghq");
    repo(root, "gitlab.com/a/b");
    dir(root, "github.com/empty-owner");
    fs::write(root.join("stray-file"), b"x").expect("write");

    for detect in BOTH {
        assert_eq!(
            walk(root, &options(detect, &[])),
            vec!["github.com/zchee/ghq", "github.com/zchee/scap", "gitlab.com/a/b"],
            "{detect:?}: a plain directory and a regular file are not repositories"
        );
    }
}

#[test]
fn a_repository_inside_a_repository_is_never_reached() {
    // ADR-9 rule (i): the entry list decides `.git` before any child is
    // emitted or queued. The §0 prototype decided per entry instead, so a
    // child that happened to be visited before `.git` escaped from inside the
    // repository. The two inner names bracket `.git` in byte order, so
    // whichever way the filesystem orders the directory one of them is read
    // before the marker.
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "outer");
    repo(root, "outer/!early");
    repo(root, "outer/zlate");
    dir(root, "outer/plain/deeper");

    for detect in BOTH {
        assert_eq!(walk(root, &options(detect, &[])), vec!["outer"], "{detect:?}");
    }
}

#[test]
fn a_directory_named_dot_git_suffixed_is_a_repository_and_is_not_opened() {
    // Rule (ii): ghq matches the `.git` suffix on the path itself
    // (local_repository.go:247-249), so a bare repository is recognised
    // without being read. The nested repository proves it was not read: it
    // would have been found if the walk had descended.
    let tmp = tempdir();
    let root = tmp.path();
    dir(root, "mirror/bare.git");
    repo(root, "mirror/bare.git/inner");

    for detect in BOTH {
        let listing = walk_root(root, &options(detect, &[])).expect("walk_root");
        assert_eq!(sorted(&listing), vec![&b"mirror/bare.git"[..]], "{detect:?}");
        assert_eq!(
            listing.dirs_read(),
            2,
            "{detect:?}: only the root and `mirror` are read; a bare repository is not opened"
        );
    }
}

#[test]
fn a_gitfile_marks_a_repository() {
    // Rule (iv): ghq's `os.Stat(<dir>/.git)` succeeds for a regular file just
    // as it does for a directory, which is what makes a worktree or submodule
    // checkout — where `.git` is a `gitdir:` pointer file — a repository.
    let tmp = tempdir();
    let root = tmp.path();
    dir(root, "worktree");
    fs::write(root.join("worktree/.git"), b"gitdir: /elsewhere/.git\n").expect("write gitfile");

    for detect in BOTH {
        assert_eq!(walk(root, &options(detect, &[])), vec!["worktree"], "{detect:?}");
    }
}

#[test]
fn a_dangling_git_symlink_is_not_a_repository() {
    // Rule (iv): a `.git` that is a symlink is the one case the entry type
    // cannot settle, so it costs one following `statat` — and a link with no
    // target fails it. ghq's `os.Stat` follows too, and prints nothing here.
    let tmp = tempdir();
    let root = tmp.path();
    dir(root, "dangling");
    symlink("nowhere", root.join("dangling/.git")).expect("symlink");
    repo(root, "real");

    for detect in BOTH {
        assert_eq!(walk(root, &options(detect, &[])), vec!["real"], "{detect:?}");
    }
}

#[test]
fn a_git_symlink_that_resolves_is_a_repository() {
    let tmp = tempdir();
    let root = tmp.path();
    dir(root, "store/realgit");
    dir(root, "linked");
    symlink("../store/realgit", root.join("linked/.git")).expect("symlink");

    for detect in BOTH {
        assert_eq!(walk(root, &options(detect, &[])), vec!["linked"], "{detect:?}");
    }
}

// ---------------------------------------------------------------------------
// Symlinks (rule iii)
// ---------------------------------------------------------------------------

#[test]
fn a_symlink_to_a_repository_is_emitted_at_the_links_own_path() {
    // Rule (iii): ghq resolves the link for `IsDir`, stats `<link>/.git`, and
    // calls back with the *link's* path (local_repository.go:268-299), so
    // both spellings of the same repository are printed. tests/list.rs:553.
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "github.com/a/x");
    symlink(root.join("github.com/a/x"), root.join("mirror")).expect("symlink");

    for detect in BOTH {
        assert_eq!(
            walk(root, &options(detect, &[])),
            vec!["github.com/a/x", "mirror"],
            "{detect:?}"
        );
    }
}

#[test]
fn a_symlink_to_a_plain_directory_is_not_descended() {
    // Rule (iii) again, and the reason tests/list.rs:167 was rewritten: the
    // repository below the link is reachable by its real path only. ghq's
    // walker refuses to descend a link at all (walker.go:85-90).
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "plain/nested/repo");
    symlink(root.join("plain"), root.join("link-to-plaindir")).expect("symlink");

    for detect in BOTH {
        assert_eq!(walk(root, &options(detect, &[])), vec!["plain/nested/repo"], "{detect:?}");
    }
}

#[test]
fn links_that_resolve_to_nothing_useful_are_ignored() {
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "real");
    fs::write(root.join("target-file"), b"x").expect("write");
    symlink(root.join("target-file"), root.join("link-to-file")).expect("symlink");
    symlink("no-such-entry", root.join("dangling")).expect("symlink");
    // A link back to an ancestor: a loop candidate the walk must not follow.
    symlink(root, root.join("loop")).expect("symlink");

    for detect in BOTH {
        assert_eq!(walk(root, &options(detect, &[])), vec!["real"], "{detect:?}");
    }
}

#[test]
fn the_dot_git_suffix_test_reads_the_links_own_name() {
    // W0.4 case 7 and its mirror: ghq's `HasSuffix` sees the path it was
    // given, which for a symlink is the link's own spelling. So a link named
    // `slink.git` is a repository whatever it points at, and a link named
    // `link-to-baregit` is not, even pointing at a bare repository.
    let tmp = tempdir();
    let root = tmp.path();
    dir(root, "plaindir");
    dir(root, "store/upstream.git");
    symlink(root.join("plaindir"), root.join("slink.git")).expect("symlink");
    symlink(root.join("store/upstream.git"), root.join("link-to-baregit")).expect("symlink");

    for detect in BOTH {
        assert_eq!(
            walk(root, &options(detect, &[])),
            vec!["slink.git", "store/upstream.git"],
            "{detect:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Errors below the root (rule v)
// ---------------------------------------------------------------------------

#[test]
fn an_unreadable_directory_is_warned_about_and_skipped() {
    if rustix::process::getuid().is_root() {
        // root opens a mode-000 directory regardless, so the fixture cannot
        // produce the condition under test.
        return;
    }
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "reachable");
    let locked = dir(root, "locked");
    repo(root, "locked/hidden");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    let _guard = ModeGuard(locked.clone());

    for detect in BOTH {
        let mut found = Vec::new();
        let logged = captured_warnings(|| found = walk(root, &options(detect, &[])));

        assert_eq!(found, vec!["reachable"], "{detect:?}: the walk continues past the failure");
        assert!(
            logged.contains(&format!("{}: Permission denied", locked.display())),
            "{detect:?}: rule (v) warns with ghq's own wording and the full path; got: {logged}"
        );
    }
}

#[test]
fn a_searchable_but_unreadable_repository_is_listed_the_way_ghq_lists_it() {
    // Reading a directory needs its read bit; `stat`ping through it needs
    // only its search bit. ghq never opens a candidate — it stats
    // `<dir>/.git` — so a repository at mode 0111 or 0311 is one ghq prints,
    // silently, while the plain directory beside it draws the warning
    // because ghq does try to read that one. Verified against ghq 1.8.0 on
    // this exact fixture shape.
    //
    // Open-and-scan is the strategy that has to work for this: it cannot open
    // the directory at all, so it falls back to the same probe stat-first
    // makes up front. Without that fallback the two strategies would return
    // different repository sets and W3.0b would be choosing between two
    // semantics rather than two costs.
    if rustix::process::getuid().is_root() {
        return;
    }
    let tmp = tempdir();
    let root = tmp.path();
    let search_only = repo(root, "search-only-repo");
    let write_search = repo(root, "write-search-repo");
    let plain = dir(root, "search-only-plain");
    dir(root, "search-only-plain/unreachable");
    repo(root, "normal");

    fs::set_permissions(&search_only, fs::Permissions::from_mode(0o111)).expect("chmod 0111");
    let _g1 = ModeGuard(search_only.clone());
    fs::set_permissions(&write_search, fs::Permissions::from_mode(0o311)).expect("chmod 0311");
    let _g2 = ModeGuard(write_search);
    fs::set_permissions(&plain, fs::Permissions::from_mode(0o111)).expect("chmod 0111");
    let _g3 = ModeGuard(plain.clone());

    for detect in BOTH {
        let mut found = Vec::new();
        let logged = captured_warnings(|| found = walk(root, &options(detect, &[])));

        assert_eq!(
            found,
            vec!["normal", "search-only-repo", "write-search-repo"],
            "{detect:?}: an unreadable repository is still a repository"
        );
        assert!(
            logged.contains(&format!("{}: Permission denied", plain.display())),
            "{detect:?}: the directory ghq fails to read is the one that warns; got: {logged}"
        );
        assert!(
            !logged.contains("search-only-repo") && !logged.contains("write-search-repo"),
            "{detect:?}: ghq never reads a repository, so there is nothing to warn about; \
             got: {logged}"
        );
    }
}

// ---------------------------------------------------------------------------
// The root itself (rules ii, vi)
// ---------------------------------------------------------------------------

#[test]
fn a_root_that_is_a_repository_prints_dot() {
    let tmp = tempdir();
    let root = tmp.path();
    fs::create_dir(root.join(".git")).expect("mkdir .git");
    repo(root, "inner");

    for detect in BOTH {
        assert_eq!(
            walk(root, &options(detect, &[])),
            vec!["."],
            "{detect:?}: a repository root emits `.` and nothing beneath it"
        );
    }
}

#[test]
fn a_root_whose_name_ends_in_dot_git_prints_dot_without_being_read() {
    let tmp = tempdir();
    let root = dir(tmp.path(), "rootbare.git");
    repo(&root, "inner");

    for detect in BOTH {
        let listing = walk_root(&root, &options(detect, &[])).expect("walk_root");
        assert_eq!(sorted(&listing), vec![&b"."[..]], "{detect:?}");
        assert_eq!(listing.dirs_read(), 0, "{detect:?}: rule (ii) settles it without a read");
    }
}

#[test]
fn a_missing_root_is_skipped_in_silence() {
    let tmp = tempdir();
    let missing = tmp.path().join("not-created");

    let logged = captured_warnings(|| {
        let listing = walk_root(&missing, &options(DetectStrategy::OpenScan, &[])).expect("ok");
        assert!(listing.is_empty());
        assert_eq!(listing.dirs_read(), 0);
    });
    assert!(
        logged.is_empty(),
        "a `scap.root` the user has not created yet is not an error; got: {logged}"
    );
}

#[test]
fn a_root_that_is_not_a_directory_yields_nothing() {
    let tmp = tempdir();
    let file = tmp.path().join("a-file");
    fs::write(&file, b"x").expect("write");

    let listing = walk_root(&file, &options(DetectStrategy::OpenScan, &[])).expect("ok");
    assert!(listing.is_empty());
}

#[test]
fn an_unreadable_root_is_skipped_with_a_warning() {
    if rustix::process::getuid().is_root() {
        return;
    }
    let tmp = tempdir();
    let root = dir(tmp.path(), "locked-root");
    repo(&root, "hidden");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    let _guard = ModeGuard(root.clone());

    let mut listing = None;
    let logged = captured_warnings(|| {
        listing = Some(walk_root(&root, &options(DetectStrategy::OpenScan, &[])).expect("ok"));
    });

    assert!(listing.expect("listing").is_empty());
    assert!(
        logged.contains(&format!("{}: Permission denied", root.display())),
        "rule (vi) always warns for a root, since it hides every repository below it; got: {logged}"
    );
}

#[test]
fn a_root_path_holding_an_interior_nul_is_the_one_error() {
    let root = PathBuf::from(OsStr::from_bytes(b"/tmp/interior\0nul"));
    let err = walk_root(&root, &options(DetectStrategy::OpenScan, &[]))
        .expect_err("a path the OS cannot be asked about is a root-level failure");
    assert!(matches!(err, WalkError::InvalidRoot { .. }));
    assert!(err.to_string().contains("interior NUL"), "{err}");
}

// ---------------------------------------------------------------------------
// Exclusions (rule viii)
// ---------------------------------------------------------------------------

/// The tree both exclusion tests use.
///
/// Directory reads with no exclusion: root, `keep`, `keep/repo`, `skip`,
/// `skip/repo` and `plain` under open-and-scan, which reads repositories too;
/// the same list without the two repositories under stat-first.
fn exclusion_fixture(root: &Path) {
    repo(root, "keep/repo");
    repo(root, "skip/repo");
    dir(root, "plain");
}

#[test]
fn an_excluded_directory_is_neither_read_nor_emitted() {
    let tmp = tempdir();
    let root = tmp.path();
    exclusion_fixture(root);

    for (detect, unpruned, pruned) in
        [(DetectStrategy::OpenScan, 6, 4), (DetectStrategy::StatFirst, 4, 3)]
    {
        let all = walk_root(root, &options(detect, &[])).expect("walk_root");
        assert_eq!(sorted(&all), vec![&b"keep/repo"[..], b"skip/repo"], "{detect:?}");
        assert_eq!(all.dirs_read(), unpruned, "{detect:?}: baseline directory reads");
        assert_eq!(all.excluded(), 0);

        let some = walk_root(root, &options(detect, &["skip"])).expect("walk_root");
        assert_eq!(
            sorted(&some),
            vec![&b"keep/repo"[..]],
            "{detect:?}: an excluded subtree emits nothing"
        );
        assert_eq!(
            some.dirs_read(),
            pruned,
            "{detect:?}: the exclusion is tested at queue time, so the subtree is never read"
        );
        assert_eq!(some.excluded(), 1, "{detect:?}");
    }
}

#[test]
fn an_excluded_repository_directory_is_not_emitted() {
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "github.com/zchee/scap");
    repo(root, "github.com/zchee/hidden");

    for detect in BOTH {
        let listing =
            walk_root(root, &options(detect, &["github.com/zchee/hidden"])).expect("walk_root");
        assert_eq!(sorted(&listing), vec![&b"github.com/zchee/scap"[..]], "{detect:?}");
        assert_eq!(listing.excluded(), 1, "{detect:?}");
    }
}

#[test]
fn patterns_are_anchored_at_the_root_and_case_sensitive() {
    // W2b.1's matcher, unchanged: git's wildmatch under `WM_PATHNAME`.
    let node_modules = Pattern::new("node_modules");
    assert!(node_modules.matches(b"node_modules"));
    assert!(!node_modules.matches(b"a/node_modules"), "a pattern is anchored at the root");
    assert!(!node_modules.matches(b"node_modules/x"), "it matches the directory, not its subtree");
    assert!(
        !Pattern::new("Node_Modules").matches(b"node_modules"),
        "git's matcher is case-sensitive"
    );

    // `*` stops at a separator, `**` crosses it.
    assert!(Pattern::new("*/vendor").matches(b"a/vendor"));
    assert!(!Pattern::new("*/vendor").matches(b"a/b/vendor"));
    assert!(Pattern::new("**/vendor").matches(b"a/b/vendor"));

    // The trailing slash the config snapshot folds away is not folded again
    // here: `foo//` still names an empty component and still matches nothing.
    assert!(!Pattern::new("foo//").matches(b"foo"));
}

// ---------------------------------------------------------------------------
// Scheduling (the repository set must not depend on it)
// ---------------------------------------------------------------------------

/// A tree wide and deep enough that four workers genuinely interleave on it:
/// 8 hosts x 8 owners x 32 children, every fourth child a repository.
///
/// Returns the directories each strategy reads and the repositories both must
/// find. The two read counts differ by exactly the repositories, which
/// open-and-scan opens and stat-first does not.
fn wide_tree(root: &Path) -> (usize, usize, usize) {
    for host in 0..8 {
        for owner in 0..8 {
            for n in 0..32 {
                let rel = format!("host{host}/owner{owner}/n{n:02}");
                if n % 4 == 0 {
                    repo(root, &rel);
                } else {
                    dir(root, &rel);
                }
            }
        }
    }
    let repos = 8 * 8 * 8;
    // 1 root + 8 hosts + 64 owners + 2,048 children, all of which
    // open-and-scan reads, repositories included; stat-first reads the same
    // set less the repositories.
    let open_reads = 1 + 8 + 64 + 8 * 8 * 32;
    (open_reads, open_reads - repos, repos)
}

#[test]
fn the_repository_set_does_not_depend_on_the_worker_count() {
    // The whole point of the pool: scheduling decides the order directories
    // are read in and nothing else. Walk order is not asserted — it is
    // genuinely non-deterministic, and `list` sorts once at the end — but the
    // set, the count and both counters have to come out the same every time.
    let tmp = tempdir();
    let root = tmp.path();
    let (open_reads, stat_reads, repos) = wide_tree(root);

    for detect in BOTH {
        let expected_reads =
            if detect == DetectStrategy::OpenScan { open_reads } else { stat_reads };
        let mut baseline: Option<Vec<String>> = None;
        for threads in [1, 2, 4, 16] {
            let opts = WalkOptions { threads, exclude: Vec::new(), detect };
            let listing = walk_root(root, &opts).expect("walk_root");
            let found: Vec<String> =
                sorted(&listing).iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect();

            assert_eq!(found.len(), repos, "{detect:?} at {threads} workers");
            assert_eq!(
                listing.dirs_read(),
                expected_reads,
                "{detect:?} at {threads} workers: every directory is read exactly once"
            );
            assert_eq!(listing.excluded(), 0);

            match &baseline {
                None => baseline = Some(found),
                Some(first) => assert_eq!(
                    &found, first,
                    "{detect:?}: {threads} workers found a different set than 1 worker did"
                ),
            }
        }
    }
}

#[test]
fn the_edge_fixtures_survive_every_worker_count() {
    // The rules that cost a syscall to decide — symlink resolution, the
    // `<link>/.git` probe, the bare-repository shortcut — are the ones a
    // racing worker could plausibly disturb, so they get the same treatment
    // as the wide tree.
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "github.com/a/x");
    repo(root, "plain/nested/repo");
    dir(root, "store/upstream.git");
    dir(root, "worktree");
    fs::write(root.join("worktree/.git"), b"gitdir: /elsewhere/.git\n").expect("write gitfile");
    symlink(root.join("github.com/a/x"), root.join("mirror")).expect("symlink");
    symlink(root.join("plain"), root.join("link-to-plaindir")).expect("symlink");
    symlink(root.join("store/upstream.git"), root.join("link-to-baregit")).expect("symlink");
    symlink("nowhere", root.join("dangling")).expect("symlink");
    symlink(root, root.join("loop")).expect("symlink");

    let expected =
        vec!["github.com/a/x", "mirror", "plain/nested/repo", "store/upstream.git", "worktree"];
    for detect in BOTH {
        for threads in [1, 2, 4, 16] {
            let opts = WalkOptions { threads, exclude: Vec::new(), detect };
            assert_eq!(walk(root, &opts), expected, "{detect:?} at {threads} workers");
        }
    }
}

#[test]
fn parse_threads_accepts_the_documented_range_and_falls_back_otherwise() {
    assert_eq!(parse_threads(Some(OsStr::new("1"))), 1);
    assert_eq!(parse_threads(Some(OsStr::new("16"))), 16);
    assert_eq!(parse_threads(Some(OsStr::new("64"))), MAX_THREADS);

    // Unset and empty mean "no opinion", and every malformed spelling falls
    // back rather than failing the listing.
    assert_eq!(parse_threads(None), DEFAULT_THREADS);
    assert_eq!(parse_threads(Some(OsStr::new(""))), DEFAULT_THREADS);
    assert_eq!(parse_threads(Some(OsStr::new("0"))), DEFAULT_THREADS);
    assert_eq!(parse_threads(Some(OsStr::new("65"))), DEFAULT_THREADS);
    assert_eq!(parse_threads(Some(OsStr::new("-1"))), DEFAULT_THREADS);
    assert_eq!(parse_threads(Some(OsStr::new("4 "))), DEFAULT_THREADS);
    assert_eq!(parse_threads(Some(OsStr::new("four"))), DEFAULT_THREADS);
}

#[test]
fn a_rejected_thread_count_says_so_rather_than_changing_nothing_silently() {
    // Falling back without a word would leave a user who typed `=0` believing
    // the walk honoured it, and a measurement row secretly identical to the
    // default row.
    for value in ["four", "0", "65"] {
        let logged = captured_warnings(|| {
            assert_eq!(parse_threads(Some(OsStr::new(value))), DEFAULT_THREADS);
        });
        assert!(
            logged.contains("SCAP_LIST_THREADS") && logged.contains("using 4"),
            "{value:?} should warn and name the fallback; got: {logged}"
        );
    }
}

#[test]
fn an_unset_variable_leaves_the_measured_default_in_place() {
    // The suite cannot set the variable to check the other direction --
    // Edition 2024 makes `set_var` unsafe and this crate denies unsafe, which
    // is why the parser above is a pure function over the value. What is
    // testable here is the case that actually reaches users: nothing set, so
    // the walk runs on the thread count W0.2 measured.
    assert_eq!(
        std::env::var_os("SCAP_LIST_THREADS"),
        None,
        "unset SCAP_LIST_THREADS to run the suite; it changes what the walk does"
    );
    assert_eq!(threads_from_env(), DEFAULT_THREADS);
}

// ---------------------------------------------------------------------------
// Descriptor budget, options and the detection switch
// ---------------------------------------------------------------------------

#[test]
fn the_reopen_path_produces_the_same_listing_as_the_descriptor_path() {
    // With the budget at zero every queued directory is re-opened from the
    // root by its relative path instead of from its parent's descriptor. That
    // branch is unreachable on any corpus this machine has — the W0.2 spike
    // never hit the 4,096 cap — so it is proved here rather than in
    // production.
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "a/b/c/deep");
    repo(root, "a/other");
    repo(root, "x/y");

    for detect in BOTH {
        let opts = options(detect, &[]);
        let budgeted = walk_root_capped(root, &opts, 0).expect("walk_root_capped");
        let normal = walk_root(root, &opts).expect("walk_root");
        assert_eq!(sorted(&budgeted), sorted(&normal), "{detect:?}");
        assert_eq!(budgeted.dirs_read(), normal.dirs_read(), "{detect:?}");
    }
}

#[test]
fn both_strategies_agree_on_the_repository_set_and_disagree_on_the_reads() {
    // Deviation D-6 in one assertion: the two strategies are interchangeable
    // for correctness and are not interchangeable for cost, which is why the
    // default is measured rather than chosen.
    let tmp = tempdir();
    let root = tmp.path();
    for i in 0..8 {
        repo(root, &format!("host/owner/repo{i}"));
    }

    let scan = walk_root(root, &options(DetectStrategy::OpenScan, &[])).expect("walk_root");
    let stat = walk_root(root, &options(DetectStrategy::StatFirst, &[])).expect("walk_root");

    assert_eq!(sorted(&scan), sorted(&stat));
    assert_eq!(scan.dirs_read(), 11, "root, host, owner and the 8 repositories");
    assert_eq!(stat.dirs_read(), 3, "root, host and owner; no repository is opened");
}

#[test]
fn parse_detect_strategy_reads_the_two_documented_values() {
    assert_eq!(parse_detect_strategy(Some(OsStr::new("open"))), DetectStrategy::OpenScan);
    assert_eq!(parse_detect_strategy(Some(OsStr::new("stat"))), DetectStrategy::StatFirst);

    // Unset and empty are the same as not asking, and an unrecognised value
    // falls back rather than failing the listing.
    assert_eq!(parse_detect_strategy(None), DetectStrategy::default());
    assert_eq!(parse_detect_strategy(Some(OsStr::new(""))), DetectStrategy::default());
    assert_eq!(parse_detect_strategy(Some(OsStr::new("OPEN"))), DetectStrategy::default());
    assert_eq!(parse_detect_strategy(Some(OsStr::new("both"))), DetectStrategy::default());
}

#[test]
fn walk_options_take_their_strategy_from_the_environment() {
    let opts = WalkOptions::new(2, vec![Pattern::new("x")]);
    assert_eq!(opts.threads, 2);
    assert_eq!(opts.exclude.len(), 1);
    assert_eq!(
        opts.detect,
        detect_strategy_from_env(),
        "the measurement override has to reach the walk without every caller knowing about it"
    );
}

#[test]
fn an_empty_root_is_read_but_yields_nothing() {
    let tmp = tempdir();
    let listing =
        walk_root(tmp.path(), &options(DetectStrategy::OpenScan, &[])).expect("walk_root");
    assert!(listing.is_empty());
    assert_eq!(listing.len(), 0);
    assert_eq!(listing.dirs_read(), 1, "the root itself is read");
    assert_eq!(listing.excluded(), 0);
    assert!(listing.repos().next().is_none());
}
