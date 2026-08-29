use std::path::Path;
use std::sync::atomic::AtomicUsize;

use rustix::fs::{Mode, OFlags};

use super::*;
use crate::walk::{DetectStrategy, FD_CAP, Pattern, WalkOptions, walk_root};

fn repo(root: &Path, rel: &str) {
    std::fs::create_dir_all(root.join(rel).join(".git")).expect("create repository fixture");
}

fn open_dir(path: &Path) -> std::os::fd::OwnedFd {
    rustix::fs::openat(
        rustix::fs::CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .expect("openat")
}

fn sorted(listing: &crate::walk::RootListing) -> Vec<String> {
    let mut repos: Vec<String> =
        listing.repos().map(|r| String::from_utf8_lossy(r).into_owned()).collect();
    repos.sort_unstable();
    repos
}

fn options(threads: usize) -> WalkOptions {
    WalkOptions {
        threads,
        exclude: Vec::<Pattern>::new(),
        detect: DetectStrategy::OpenScan,
        record: false,
    }
}

/// The pool is the only thing standing between a caller's number and
/// `thread::scope`, so the clamp has to live here rather than in the caller:
/// a zero would spawn no workers at all and the walk would return the root's
/// children unread, silently.
#[test]
fn a_request_for_no_threads_still_walks() {
    let tmp = tempfile::tempdir().expect("tempdir");
    repo(tmp.path(), "host/owner/one");
    repo(tmp.path(), "host/owner/two");

    let listing = walk_root(tmp.path(), &options(0)).expect("walk_root");
    assert_eq!(sorted(&listing), vec!["host/owner/one", "host/owner/two"]);
}

#[test]
fn a_request_for_more_threads_than_the_maximum_is_capped() {
    let tmp = tempfile::tempdir().expect("tempdir");
    repo(tmp.path(), "host/owner/one");

    // Past the cap the walk only adds contention; what matters here is that
    // the request is honoured as a walk rather than as a thread count.
    let listing = walk_root(tmp.path(), &options(MAX_THREADS * 4)).expect("walk_root");
    assert_eq!(sorted(&listing), vec!["host/owner/one"]);
}

/// Drives the pool directly to pin the contract `walk_root` depends on: one
/// `Out` per worker, each holding only what that worker found, and the union
/// of them holding everything.
///
/// Run at every worker count against both descriptor regimes. A cap of 2
/// forces most jobs onto the re-open path while a few still carry a
/// descriptor, and that mixed case is the only one where the increment a
/// parent makes and the decrement its child makes could come apart — so
/// `live_fds` returning to zero is asserted after every combination.
#[test]
fn every_worker_returns_its_own_output_and_the_union_is_the_walk() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    for owner in 0..4 {
        for n in 0..8 {
            repo(root, &format!("host/owner{owner}/repo{n}"));
        }
    }
    let mut expected: Vec<String> =
        (0..4).flat_map(|o| (0..8).map(move |n| format!("host/owner{o}/repo{n}"))).collect();
    expected.sort_unstable();

    let root_bytes = std::os::unix::ffi::OsStrExt::as_bytes(root.as_os_str());
    for fd_cap in [FD_CAP, 2] {
        for threads in [1, 2, 4, 16] {
            let live_fds = AtomicUsize::new(0);
            let ctx = Ctx {
                root: root_bytes,
                exclude: &[],
                detect: DetectStrategy::OpenScan,
                record: false,
                live_fds: &live_fds,
                fd_cap,
            };
            let base_fd = open_dir(root);
            let mut seed = Walker::new(&ctx);
            let mut queue = Vec::new();
            seed.read_dir(open_dir(root), b"", &mut queue);

            let mut parts = run(&ctx, std::os::fd::AsFd::as_fd(&base_fd), queue, threads);
            assert_eq!(parts.len(), threads, "one output per worker, joined in order");

            let mut merged = seed.into_out();
            for part in &mut parts {
                merged.merge(part);
            }
            let mut found: Vec<String> =
                merged.arena.iter().map(|r| String::from_utf8_lossy(r).into_owned()).collect();
            found.sort_unstable();

            let at = format!("fd_cap {fd_cap} at {threads} workers");
            assert_eq!(found, expected, "{at}: 32 repositories, however they were divided up");
            assert_eq!(
                merged.dirs_read,
                1 + 1 + 4 + 32,
                "{at}: root, host, four owners and the repositories open-and-scan reads"
            );
            assert_eq!(
                live_fds.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "{at}: every parked descriptor was taken back"
            );
        }
    }
}

/// A worker that panics must not hang the pool.
///
/// Injected through a real code path rather than a test-only hook: a queued
/// job whose relative path holds an interior NUL trips `cstr`'s `expect` on
/// the re-open branch. Before the `Retire` guard, the in-flight count would
/// never reach zero, every other worker would wait for work that could no
/// longer arrive, and `thread::scope` would block on the join forever --
/// turning a panic, which a user sees and can report, into a hang, which
/// looks like `list` merely being slow. If this test ever regresses it will
/// time out rather than fail.
#[test]
fn a_panicking_worker_propagates_instead_of_hanging_the_pool() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    repo(root, "host/owner/one");

    let root_bytes = std::os::unix::ffi::OsStrExt::as_bytes(root.as_os_str());
    let live_fds = AtomicUsize::new(0);
    let ctx = Ctx {
        root: root_bytes,
        exclude: &[],
        detect: DetectStrategy::OpenScan,
        record: false,
        live_fds: &live_fds,
        fd_cap: FD_CAP,
    };
    let base_fd = open_dir(root);
    let queue = vec![Job { fd: None, rel: b"interior\0nul".to_vec() }];

    // The panic is the point of the test, so its default stderr report is
    // suppressed for the duration and the previous hook put back.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&ctx, std::os::fd::AsFd::as_fd(&base_fd), queue, 4)
    }));
    std::panic::set_hook(previous);

    assert!(outcome.is_err(), "the worker's panic has to reach the caller, not stall the join");
}
