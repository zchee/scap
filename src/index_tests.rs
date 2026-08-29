use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use bstr::ByteSlice;

use super::*;
use crate::walk::{DetectStrategy, Pattern};

/// Both `.git` detection strategies must produce the same index and the same
/// cached listing. Which one ships is a measured I/O decision (deviation
/// D-6); an index that only round-tripped under one of them would quietly
/// make that decision semantic.
const BOTH: [DetectStrategy; 2] = [DetectStrategy::OpenScan, DetectStrategy::StatFirst];

/// Far enough in the past that nothing under a rewound tree can fall inside
/// [`RACY_WINDOW_NS`]: 2021-09-13, old enough to read as a fixture rather
/// than as a plausible "just now".
const SETTLED: rustix::fs::Timespec = rustix::fs::Timespec { tv_sec: 1_631_500_000, tv_nsec: 0 };

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// Options with recording on, which is what every [`Cache`] method turns on
/// for itself; a walk a test drives directly has to match or the two paths
/// would not be comparable.
fn options(detect: DetectStrategy, exclude: &[&str]) -> WalkOptions {
    WalkOptions {
        threads: 2,
        exclude: exclude.iter().copied().map(Pattern::new).collect(),
        detect,
        record: true,
    }
}

/// Creates `<root>/<rel>` as a repository, the way `git init` leaves one.
fn repo(root: &Path, rel: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.join(".git")).expect("repository fixture");
    path
}

fn dir(root: &Path, rel: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(&path).expect("directory fixture");
    path
}

/// Rewinds every directory under `path`, and `path` itself, to [`SETTLED`].
///
/// Every fixture here is younger than the racy window when it is built, so
/// without this *every* entry is re-walked and a test cannot tell a working
/// index from one that never validates anything. This is the same operation
/// `touch -t` performs, used to reach the state a real corpus is in almost
/// always: settled, and older than the last index write.
fn backdate(path: &Path) {
    for entry in fs::read_dir(path).expect("read the fixture") {
        let entry = entry.expect("fixture entry");
        if entry.file_type().expect("fixture entry type").is_dir() {
            backdate(&entry.path());
        }
    }
    let times = rustix::fs::Timestamps { last_access: SETTLED, last_modification: SETTLED };
    rustix::fs::utimensat(CWD, path, &times, AtFlags::SYMLINK_NOFOLLOW).expect("backdate");
}

/// Every repository a listing holds, in byte order.
fn repos(listing: &RootListing) -> Vec<String> {
    let mut found: Vec<String> =
        listing.repos().map(|p| String::from_utf8_lossy(p).into_owned()).collect();
    found.sort_unstable();
    found
}

/// The repositories a plain walk finds, which every cached answer has to
/// reproduce exactly.
fn walked(root: &Path, opts: &WalkOptions) -> Vec<String> {
    repos(&walk::walk_root(root, opts).expect("walk_root"))
}

/// An index built from one real walk of `root`.
fn index_of(root: &Path, opts: &WalkOptions) -> Index {
    let listing = walk::walk_root(root, opts).expect("walk_root");
    build(root, opts, listing.records()).expect("build")
}

// ---------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------

#[test]
fn an_encoded_index_decodes_back_to_itself() {
    for detect in BOTH {
        let tmp = tempdir();
        let root = tmp.path();
        repo(root, "github.com/a/x");
        repo(root, "github.com/b/y");
        dir(root, "github.com/b/empty");
        repo(root, "store/upstream.git");
        symlink(root.join("github.com/a/x"), root.join("mirror")).expect("symlink fixture");

        let index = index_of(root, &options(detect, &["never/matches"]));
        assert!(index.entries.len() > 5, "{detect:?}: the fixture has entries to encode");
        assert_eq!(
            decode(&encode(&index)),
            Some(index.clone()),
            "{detect:?}: encode and decode are inverses"
        );
    }
}

#[test]
fn a_path_that_is_not_utf8_survives_the_round_trip() {
    // Length-prefixed bytes rather than text is the whole reason: a listing
    // must not lose a repository because its directory name is not UTF-8.
    let index = Index {
        root: b"/roots/\xff\xfe".to_vec(),
        dev: 7,
        ino: 11,
        written_ns: 1_700_000_000_000_000_000,
        exclude: vec![b"vendor/\x80".to_vec()],
        entries: vec![
            Entry { rel: Vec::new(), mtime_ns: Some(1), children: vec![1] },
            Entry { rel: b"\xff\xfe/x".to_vec(), mtime_ns: None, children: Vec::new() },
        ],
    };
    assert_eq!(decode(&encode(&index)), Some(index));
}

#[test]
fn every_corruption_reads_as_no_index_rather_than_as_a_failure() {
    let tmp = tempdir();
    let root = tmp.path();
    repo(root, "github.com/a/x");
    dir(root, "github.com/b");
    let good = encode(&index_of(root, &options(DetectStrategy::StatFirst, &[])));

    assert!(decode(&good).is_some(), "the fixture encodes to something decodable");
    assert_eq!(decode(&[]), None, "an empty file");
    assert_eq!(decode(&good[..good.len() - 1]), None, "a truncated file");
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(decode(&trailing), None, "trailing bytes past the last entry");

    let mut bad_magic = good.clone();
    bad_magic[0] ^= 0xff;
    assert_eq!(decode(&bad_magic), None, "a different magic");

    let mut bad_version = good.clone();
    bad_version[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&(VERSION + 1).to_le_bytes());
    assert_eq!(decode(&bad_version), None, "a file from another version");
}

#[test]
fn a_forged_entry_count_is_rejected_before_it_is_allocated() {
    // `Reader::count` exists so a four-byte length cannot ask for gigabytes.
    // The assertion is that the file is rejected; the test completing at all
    // is the assertion that it was rejected without trying to allocate.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    push_bytes(&mut bytes, b"/roots");
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&2u64.to_le_bytes());
    bytes.extend_from_slice(&3i64.to_le_bytes());
    push_len(&mut bytes, 0);
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(decode(&bytes), None);
}

#[test]
fn a_child_edge_that_could_cycle_or_escape_is_rejected() {
    let root_entry = || Entry { rel: Vec::new(), mtime_ns: Some(1), children: Vec::new() };
    let child = |rel: &[u8]| Entry { rel: rel.to_vec(), mtime_ns: Some(2), children: Vec::new() };
    let template = Index {
        root: b"/roots".to_vec(),
        dev: 1,
        ino: 2,
        written_ns: 3,
        exclude: Vec::new(),
        entries: Vec::new(),
    };

    let mut backward = template.clone();
    backward.entries = vec![root_entry(), child(b"a")];
    backward.entries[1].children = vec![0];
    assert_eq!(decode(&encode(&backward)), None, "an edge pointing back at the root");

    let mut self_edge = template.clone();
    self_edge.entries = vec![root_entry(), child(b"a")];
    self_edge.entries[1].children = vec![1];
    assert_eq!(decode(&encode(&self_edge)), None, "an edge pointing at itself");

    let mut out_of_range = template.clone();
    out_of_range.entries = vec![root_entry(), child(b"a")];
    out_of_range.entries[0].children = vec![1, 9];
    assert_eq!(decode(&encode(&out_of_range)), None, "an edge past the last entry");

    let mut twice_claimed = template.clone();
    twice_claimed.entries = vec![root_entry(), child(b"a"), child(b"b")];
    twice_claimed.entries[0].children = vec![1, 2];
    twice_claimed.entries[1].children = vec![2];
    assert_eq!(decode(&encode(&twice_claimed)), None, "an entry claimed by two parents");

    let mut unreachable = template.clone();
    unreachable.entries = vec![root_entry(), child(b"a")];
    assert_eq!(decode(&encode(&unreachable)), None, "an entry no parent claims");

    let mut rootless = template;
    rootless.entries = vec![child(b"a")];
    assert_eq!(decode(&encode(&rootless)), None, "a file whose first entry is not the root");
}

#[test]
fn a_path_that_could_escape_the_root_is_rejected() {
    // The index file lives in a directory anything running as this user can
    // write, and every entry's path is resolved against an open descriptor for
    // the root. That confines it only while the path is genuinely relative and
    // genuinely stays inside: `statat` ignores the descriptor for an absolute
    // path, and `..` climbs back out of it. Neither can come from the walk, so
    // a file containing one was written by someone, and believing it would put
    // a path outside every configured root on `scap list --cache -p`'s stdout.
    let with_rel = |rel: &[u8]| Index {
        root: b"/roots".to_vec(),
        dev: 1,
        ino: 2,
        written_ns: 3,
        exclude: Vec::new(),
        entries: vec![
            Entry { rel: Vec::new(), mtime_ns: Some(1), children: vec![1] },
            Entry { rel: rel.to_vec(), mtime_ns: None, children: Vec::new() },
        ],
    };

    for escaping in [
        b"/etc".as_slice(),
        b"/".as_slice(),
        b"..".as_slice(),
        b"../etc".as_slice(),
        b"a/../../etc".as_slice(),
        b"a/..".as_slice(),
        b".".as_slice(),
        b"a/.".as_slice(),
        b"./a".as_slice(),
        b"a//b".as_slice(),
        b"a/".as_slice(),
        b"\0".as_slice(),
        b"a\0b".as_slice(),
    ] {
        assert_eq!(
            decode(&encode(&with_rel(escaping))),
            None,
            "a path the walk could never have produced: {:?}",
            escaping.as_bstr()
        );
    }

    // The shapes the walk does produce still decode, including the names that
    // merely look suspicious: a leading dot is an ordinary directory name.
    for ordinary in [
        b"a".as_slice(),
        b"a/b".as_slice(),
        b"github.com/zchee/scap".as_slice(),
        b".config".as_slice(),
        b"a/..b".as_slice(),
        b"a/b..".as_slice(),
        b"...".as_slice(),
        b"\xff\xfe".as_slice(),
    ] {
        assert!(
            decode(&encode(&with_rel(ordinary))).is_some(),
            "a path the walk does produce: {:?}",
            ordinary.as_bstr()
        );
    }

    // And the root's own entry is the one empty path the format allows.
    assert!(is_safe_rel(b""), "the root itself");
}

#[test]
fn a_directory_that_is_a_repository_is_recorded_once() {
    // The root is always read, even when rule (i) turns it into a repository,
    // and under `OpenScan` so is every repository below it. Recording both a
    // directory entry and a repository entry for one path would leave the
    // build's de-duplication deciding whether the repository is listed.
    for detect in BOTH {
        let tmp = tempdir();
        let root = dir(tmp.path(), "roots");
        fs::create_dir_all(root.join(".git")).expect("root repository fixture");
        repo(&root, "inner");
        let index = index_of(&root, &options(detect, &[]));

        let mut rels: Vec<&[u8]> = index.entries.iter().map(|entry| entry.rel.as_slice()).collect();
        rels.sort_unstable();
        let deduped = {
            let mut seen = rels.clone();
            seen.dedup();
            seen
        };
        assert_eq!(rels, deduped, "{detect:?}: one entry per path");
        assert_eq!(index.entries.len(), 1, "{detect:?}: a repository root has no children");
        assert!(index.entries[0].is_repo(), "{detect:?}: and it is a repository, not a directory");
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn an_entry_inside_the_racy_window_is_treated_as_changed() {
    let written = 1_700_000_000_000_000_000i64;
    assert!(is_racy(written, written), "an mtime equal to the write time");
    assert!(is_racy(written + 1, written), "an mtime after the write time");
    assert!(is_racy(written - RACY_WINDOW_NS + 1, written), "just inside the window");
    assert!(!is_racy(written - RACY_WINDOW_NS, written), "exactly the window's edge");
    assert!(!is_racy(written - RACY_WINDOW_NS - 1, written), "outside the window");
}

#[test]
fn a_racy_directory_is_re_walked_even_though_its_mtime_matches() {
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    dir(&root, "github.com/b");
    backdate(&root);

    let opts = options(DetectStrategy::StatFirst, &[]);
    let mut index = index_of(&root, &opts);
    let root_fd = open_root(&root).expect("open root");
    let probes = probe_all(root_fd.as_fd(), &index, b"roots", 2);

    let settled = walk_index(&index, &probes);
    assert_eq!(settled.racy, 0, "an index written long after the tree settled");
    assert!(settled.stale.is_empty(), "so every entry validates");
    assert_eq!(settled.hits, index.entries.len(), "and every entry is a hit");

    // The same probes, against an index claiming to have been written while
    // the tree was still moving: the state a filesystem storing whole seconds
    // cannot resolve, and the one git's own racy rule exists for.
    index.written_ns = SETTLED.tv_sec * 1_000_000_000;
    let racy = walk_index(&index, &probes);
    assert!(racy.racy > 0, "an index written while the tree was still moving");
    assert!(!racy.stale.is_empty(), "every racy entry goes back to the walker");
}

#[test]
fn an_index_written_for_another_root_is_never_believed() {
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    let opts = options(DetectStrategy::StatFirst, &[]);
    let index = index_of(&root, &opts);
    let stat = rustix::fs::statat(CWD, &root, AtFlags::empty()).expect("stat root");
    assert!(describes(&index, &root, &stat, &[]), "the index it was just built from");

    let other = dir(tmp.path(), "elsewhere");
    assert!(!describes(&index, &other, &stat, &[]), "a different root path");

    let mut moved = index.clone();
    moved.ino ^= 1;
    assert!(!describes(&moved, &root, &stat, &[]), "a root with a different inode");
    let mut remounted = index.clone();
    remounted.dev ^= 1;
    assert!(!describes(&remounted, &root, &stat, &[]), "a root on a different device");

    assert!(
        !describes(&index, &root, &stat, &[b"vendor".to_vec()]),
        "a run whose exclusion set gained a pattern the index never applied"
    );
}

#[test]
fn a_root_recreated_in_place_invalidates_the_whole_index() {
    // A Time Machine restore, or a checkout removed and cloned again: every
    // directory below can carry its old mtime, and only the root's identity
    // says the tree is not the one the index described.
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    backdate(&root);
    let opts = options(DetectStrategy::StatFirst, &[]);
    let cache = Cache::in_dir(tmp.path().join("cache"));

    assert_eq!(repos(&cache.list_root(&root, &opts).expect("first list")), ["github.com/a/x"]);

    fs::rename(&root, tmp.path().join("moved")).expect("move the root aside");
    let restored = dir(tmp.path(), "roots");
    repo(&restored, "github.com/b/y");
    backdate(&restored);
    assert_eq!(
        repos(&cache.list_root(&root, &opts).expect("list after the restore")),
        ["github.com/b/y"],
        "the index cannot outlive the directory it described"
    );
}

// ---------------------------------------------------------------------------
// The cached listing
// ---------------------------------------------------------------------------

#[test]
fn a_warm_index_answers_without_reading_a_single_directory() {
    for detect in BOTH {
        let tmp = tempdir();
        let root = dir(tmp.path(), "roots");
        repo(&root, "github.com/a/x");
        repo(&root, "github.com/b/y");
        repo(&root, "store/upstream.git");
        dir(&root, "github.com/c/not-a-repo/deeper");
        symlink(root.join("github.com/a/x"), root.join("mirror")).expect("symlink fixture");
        backdate(&root);

        let opts = options(detect, &[]);
        let cache = Cache::in_dir(tmp.path().join(format!("cache-{detect:?}")));
        let expected = walked(&root, &opts);

        let cold = cache.list_root(&root, &opts).expect("cold");
        assert_eq!(repos(&cold), expected, "{detect:?}: the run that builds the index");
        assert!(cold.dirs_read() > 0, "{detect:?}: which is a full walk");

        let warm = cache.list_root(&root, &opts).expect("warm");
        assert_eq!(repos(&warm), expected, "{detect:?}: the run that uses it");
        assert_eq!(
            warm.dirs_read(),
            0,
            "{detect:?}: a settled tree validates entirely, so nothing is read"
        );

        // And again, because the warm run rewrites the index from what it
        // observed: a second warm run is the one that would break if the
        // rewritten file were not equivalent to the one it replaced.
        let again = cache.list_root(&root, &opts).expect("warm again");
        assert_eq!(repos(&again), expected, "{detect:?}: the rewritten index");
        assert_eq!(again.dirs_read(), 0, "{detect:?}: still a full validation");
    }
}

#[test]
fn a_root_that_is_itself_a_repository_survives_the_index() {
    // The root is the one directory the walk always reads even when it turns
    // out to be a repository, so it is where a listing built from the index
    // would first lose one.
    for detect in BOTH {
        let tmp = tempdir();
        let root = dir(tmp.path(), "roots");
        fs::create_dir_all(root.join(".git")).expect("repository fixture");
        backdate(&root);
        let opts = options(detect, &[]);
        let cache = Cache::in_dir(tmp.path().join(format!("cache-{detect:?}")));

        assert_eq!(repos(&cache.list_root(&root, &opts).expect("cold")), ["."], "{detect:?}");
        let warm = cache.list_root(&root, &opts).expect("warm");
        assert_eq!(repos(&warm), ["."], "{detect:?}");
        assert_eq!(warm.dirs_read(), 0, "{detect:?}: answered from the index");
    }
}

#[test]
fn a_bare_root_is_a_repository_without_ever_being_probed() {
    // Rule (ii): a name ending in `.git` is a repository by its name alone,
    // and the index never stats one — only a rename can change that verdict,
    // and a rename moves the parent's mtime.
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots/upstream.git");
    backdate(&root);
    let opts = options(DetectStrategy::StatFirst, &[]);
    let cache = Cache::in_dir(tmp.path().join("cache"));

    assert_eq!(repos(&cache.list_root(&root, &opts).expect("cold")), ["."]);
    assert_eq!(repos(&cache.list_root(&root, &opts).expect("warm")), ["."]);
}

#[test]
fn every_mutation_the_index_can_see_is_seen() {
    for detect in BOTH {
        let tmp = tempdir();
        let root = dir(tmp.path(), "roots");
        repo(&root, "github.com/a/x");
        repo(&root, "github.com/a/y");
        dir(&root, "github.com/plain");
        backdate(&root);
        let opts = options(detect, &[]);
        let cache = Cache::in_dir(tmp.path().join(format!("cache-{detect:?}")));

        let check = |label: &str| {
            let cached = repos(&cache.list_root(&root, &opts).expect(label));
            assert_eq!(cached, walked(&root, &opts), "{detect:?}: {label}");
        };
        check("cold");
        check("warm");

        fs::create_dir_all(root.join("github.com/plain/.git")).expect("git init in place");
        check("a repository appearing inside a directory the index knew");

        fs::remove_dir_all(root.join("github.com/a/x/.git")).expect("rm -rf .git");
        check("a repository losing its .git");

        fs::rename(root.join("github.com/a/y"), root.join("github.com/a/z")).expect("rename");
        check("a repository renamed");

        fs::remove_dir_all(root.join("github.com/a/z")).expect("remove");
        check("a repository removed");

        repo(&root, "deeply/nested/one/two/three/four");
        check("a repository six levels down");

        fs::create_dir_all(root.join("bare/mirror.git")).expect("bare repository");
        check("a bare repository added");

        fs::remove_dir_all(root.join("bare/mirror.git")).expect("remove the bare repository");
        check("a bare repository removed");
    }
}

#[test]
fn a_directory_replaced_by_a_symlink_is_not_descended_on_a_cached_run() {
    // `NOFOLLOW` on the validation `stat` is what makes this work: the walk
    // never descends a symlink, so an index that validated one by following
    // it would report repositories the walk does not.
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    backdate(&root);
    let opts = options(DetectStrategy::StatFirst, &[]);
    let cache = Cache::in_dir(tmp.path().join("cache"));
    assert_eq!(repos(&cache.list_root(&root, &opts).expect("cold")), ["github.com/a/x"]);

    let elsewhere = dir(tmp.path(), "elsewhere");
    repo(&elsewhere, "hidden");
    fs::remove_dir_all(root.join("github.com/a")).expect("remove the directory");
    symlink(&elsewhere, root.join("github.com/a")).expect("replace it with a symlink");

    assert_eq!(
        repos(&cache.list_root(&root, &opts).expect("warm")),
        walked(&root, &opts),
        "a symlinked directory is not walked through, cached or not"
    );
}

#[test]
fn an_excluded_subtree_is_neither_indexed_nor_validated() {
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    repo(&root, "vendor/bundled");
    backdate(&root);
    let opts = options(DetectStrategy::StatFirst, &["vendor"]);
    let cache = Cache::in_dir(tmp.path().join("cache"));

    assert_eq!(repos(&cache.list_root(&root, &opts).expect("cold")), ["github.com/a/x"]);
    let warm = cache.list_root(&root, &opts).expect("warm");
    assert_eq!(repos(&warm), ["github.com/a/x"]);
    assert_eq!(warm.dirs_read(), 0, "answered from the index");

    let index = load(&cache.path_for(&root).expect("a cache path")).expect("an index on disk");
    assert!(
        !index.entries.iter().any(|entry| entry.rel.starts_with(b"vendor")),
        "an excluded subtree never reaches the index, so it is never validated"
    );

    // Dropping the pattern has to bring the subtree back: its parent's mtime
    // never moved, so only the recorded exclusion set can reveal the change.
    let opened = options(DetectStrategy::StatFirst, &[]);
    assert_eq!(
        repos(&cache.list_root(&root, &opened).expect("without the pattern")),
        ["github.com/a/x", "vendor/bundled"]
    );
}

#[test]
fn a_check_run_reports_a_backdated_mutation_and_still_prints_the_truth() {
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    backdate(&root);
    let opts = options(DetectStrategy::StatFirst, &[]);
    let cache = Cache::in_dir(tmp.path().join("cache"));

    let (listing, differs) = cache.check_root(&root, &opts).expect("check with no index");
    assert!(!differs, "there was no index to disagree with");
    assert_eq!(repos(&listing), ["github.com/a/x"]);

    let (listing, differs) = cache.check_root(&root, &opts).expect("check against a fresh index");
    assert!(!differs, "an index just written describes the tree");
    assert_eq!(repos(&listing), ["github.com/a/x"]);

    // The one mutation the index cannot see: a repository added, and every
    // directory's mtime then rewound to what the index recorded. This is the
    // documented failure, and `--cache-check` is its whole answer.
    repo(&root, "github.com/a/y");
    backdate(&root);

    let (listing, differs) = cache.check_root(&root, &opts).expect("check against a stale index");
    assert!(differs, "a backdated mutation is invisible to validation and visible to the check");
    assert_eq!(repos(&listing), ["github.com/a/x", "github.com/a/y"], "stdout is the fresh walk");

    // The check rewrites the index from its own fresh walk, so the run after
    // one that found a difference agrees again.
    let (_, differs) = cache.check_root(&root, &opts).expect("check after the repair");
    assert!(!differs, "the check leaves a correct index behind");
}

#[test]
fn a_backdated_mutation_is_exactly_what_a_cached_run_misses() {
    // The counterpart to the check above, stated as the limitation it is:
    // this is the residue §4 S1 names, and the reason the index is opt-in.
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    backdate(&root);
    let opts = options(DetectStrategy::StatFirst, &[]);
    let cache = Cache::in_dir(tmp.path().join("cache"));
    assert_eq!(repos(&cache.list_root(&root, &opts).expect("cold")), ["github.com/a/x"]);

    repo(&root, "github.com/a/y");
    backdate(&root);
    assert_eq!(
        repos(&cache.list_root(&root, &opts).expect("warm")),
        ["github.com/a/x"],
        "a rewound mtime is a lie the index has no way to catch"
    );
    assert_eq!(
        walked(&root, &opts),
        ["github.com/a/x", "github.com/a/y"],
        "which the walk, and so `--no-cache`, still sees"
    );
}

#[test]
fn an_unusable_cache_directory_still_lists() {
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    backdate(&root);
    let opts = options(DetectStrategy::StatFirst, &[]);

    let nowhere = Cache { dir: None };
    assert!(nowhere.path_for(&root).is_none(), "nowhere to keep an index");
    assert_eq!(repos(&nowhere.list_root(&root, &opts).expect("list")), ["github.com/a/x"]);

    // A cache "directory" that is a file: every write fails, every read finds
    // nothing, and the listing is unaffected.
    let blocked = tmp.path().join("blocked");
    fs::write(&blocked, b"not a directory").expect("write the blocker");
    let cache = Cache::in_dir(blocked);
    assert_eq!(repos(&cache.list_root(&root, &opts).expect("first")), ["github.com/a/x"]);
    assert_eq!(repos(&cache.list_root(&root, &opts).expect("second")), ["github.com/a/x"]);
}

#[test]
fn a_corrupt_index_file_is_rebuilt_rather_than_fatal() {
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    backdate(&root);
    let opts = options(DetectStrategy::StatFirst, &[]);
    let cache = Cache::in_dir(tmp.path().join("cache"));
    assert_eq!(repos(&cache.list_root(&root, &opts).expect("cold")), ["github.com/a/x"]);

    let path = cache.path_for(&root).expect("a cache path");
    fs::write(&path, b"SCAPIDX\0garbage").expect("corrupt the index");
    let rebuilt = cache.list_root(&root, &opts).expect("warm");
    assert_eq!(repos(&rebuilt), ["github.com/a/x"]);
    assert!(rebuilt.dirs_read() > 0, "an unreadable index means a full walk");
    assert!(load(&path).is_some(), "and the run leaves a decodable index behind");
}

#[test]
fn a_walk_that_dropped_a_subtree_is_never_persisted_as_an_index() {
    // The listing is still printed -- a repository lister that refuses to list
    // anything because one directory is unreadable is worse than one that
    // lists what it can and warns. What must not happen is that short listing
    // being written down as the truth: every surviving entry would validate on
    // the next run, nothing would say the missing subtree was ever there, and
    // the short listing would reprint itself in silence.
    let tmp = tempdir();
    let root = dir(tmp.path(), "roots");
    repo(&root, "github.com/a/x");
    let closed = dir(&root, "github.com/closed");
    repo(&closed, "inside");
    backdate(&root);

    let opts = options(DetectStrategy::StatFirst, &[]);
    let cache = Cache::in_dir(tmp.path().join("cache"));
    let path = cache.path_for(&root).expect("a cache path");

    fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    let listing = cache.list_root(&root, &opts).expect("list");
    assert!(listing.incomplete(), "an unreadable directory makes the walk short");
    assert!(!path.exists(), "and a short walk writes no index at all");

    // Readable again: the walk is complete, so now it is written.
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o755)).expect("restore the mode");
    let listing = cache.list_root(&root, &opts).expect("list");
    assert!(!listing.incomplete());
    assert!(path.exists(), "a complete walk is persisted");
    let written = fs::read(&path).expect("read the index");

    // Unreadable again, with an index already on disk: the previous index is
    // left exactly as it was rather than being replaced by the short one.
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    cache.list_root(&root, &opts).expect("list");
    assert_eq!(fs::read(&path).expect("read the index"), written, "the index is left alone");

    // Restore, so the temporary directory can be cleaned up.
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o755)).expect("restore the mode");
}

#[test]
fn the_index_is_written_atomically_and_leaves_nothing_behind() {
    let tmp = tempdir();
    let dir_path = tmp.path().join("cache").join("nested");
    let path = dir_path.join("index-0.bin");
    write_atomically(&path, b"first").expect("write");
    assert_eq!(fs::read(&path).expect("read"), b"first");
    write_atomically(&path, b"second").expect("overwrite");
    assert_eq!(fs::read(&path).expect("read"), b"second");

    let leftovers: Vec<_> = fs::read_dir(&dir_path)
        .expect("read the cache directory")
        .map(|entry| entry.expect("entry").file_name())
        .filter(|name| name != "index-0.bin")
        .collect();
    assert!(leftovers.is_empty(), "no temporary file survives a write: {leftovers:?}");
}

// ---------------------------------------------------------------------------
// Enablement and placement
// ---------------------------------------------------------------------------

#[test]
fn no_cache_wins_over_every_way_of_turning_the_index_on() {
    assert_eq!(mode(false, false, false, false), Mode::Off, "nothing asked for it");
    assert_eq!(mode(true, false, false, false), Mode::On, "--cache");
    assert_eq!(mode(false, false, false, true), Mode::On, "scap.listCache");
    assert_eq!(mode(false, true, false, true), Mode::Off, "--no-cache over the configuration");
    assert_eq!(mode(true, true, false, true), Mode::Off, "--no-cache over --cache");
    assert_eq!(mode(false, false, true, false), Mode::Check, "--cache-check without the index");
    assert_eq!(mode(true, false, true, true), Mode::Check, "--cache-check beside --cache");

    // `--no-cache --cache-check` never reaches this function: `list` rejects
    // the pair as a usage error, which tests/e2e_help.rs pins. The answer here
    // is the defensive one, and it prefers the operation that tells the truth
    // about the index over the one that declines to look at it.
    assert_eq!(mode(false, true, true, true), Mode::Check, "the unreachable pair, defensively");
}

#[test]
fn the_cache_directory_follows_the_xdg_specification() {
    let xdg = |value: &str| OsString::from(value);
    let home = Path::new("/home/u");
    assert_eq!(
        cache_dir_from(Some(&xdg("/x/cache")), Some(home)),
        Some(PathBuf::from("/x/cache/scap"))
    );
    assert_eq!(
        cache_dir_from(Some(&xdg("")), Some(home)),
        Some(PathBuf::from("/home/u/.cache/scap")),
        "an empty variable is an unset variable"
    );
    assert_eq!(
        cache_dir_from(Some(&xdg("relative")), Some(home)),
        Some(PathBuf::from("/home/u/.cache/scap")),
        "the specification calls a relative value invalid"
    );
    assert_eq!(cache_dir_from(None, Some(home)), Some(PathBuf::from("/home/u/.cache/scap")));
    assert_eq!(cache_dir_from(None, None), None, "no home, no cache, still a listing");
}

#[test]
fn the_process_cache_reads_the_same_environment_the_specification_names() {
    // Read-only on purpose: `std::env::set_var` is `unsafe` in edition 2024
    // and a test that mutated the environment would race every other test in
    // the binary. `from_env` is a thin wrapper over `cache_dir_from`, so
    // agreeing with it on the process's own values is the whole contract.
    let expected = cache_dir_from(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        std::env::home_dir().as_deref(),
    );
    let cache = Cache::from_env();
    assert_eq!(cache.dir, expected);

    let root = Path::new("/roots/a");
    assert_eq!(
        cache.path_for(root).is_some(),
        expected.is_some(),
        "a cache with nowhere to live is still a cache; it just never has a path"
    );
    if let Some(dir) = expected {
        assert_eq!(cache.path_for(root).expect("a path").parent(), Some(dir.as_path()));
    }
}

#[test]
fn two_roots_never_share_one_index_file() {
    let cache = Cache::in_dir(PathBuf::from("/cache"));
    let a = cache.path_for(Path::new("/roots/a")).expect("a path");
    let b = cache.path_for(Path::new("/roots/b")).expect("a path");
    assert_ne!(a, b);
    assert_eq!(a.parent(), Some(Path::new("/cache")), "index files live in the cache directory");
    assert_eq!(a, cache.path_for(Path::new("/roots/a")).expect("a path"), "and are stable");
}

#[test]
fn fnv1a64_matches_the_published_vectors() {
    // The reference values from the FNV specification, which is the whole
    // reason something this small is worth hand-rolling.
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn a_relative_path_is_split_the_way_the_walk_built_it() {
    assert_eq!(parent_of(b""), b"");
    assert_eq!(parent_of(b"a"), b"");
    assert_eq!(parent_of(b"a/b"), b"a");
    assert_eq!(parent_of(b"a/b/c"), b"a/b");
    assert_eq!(basename(b""), b"");
    assert_eq!(basename(b"a"), b"a");
    assert_eq!(basename(b"a/b.git"), b"b.git");
}
