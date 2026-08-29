use super::*;

/// ADR-9 rule (iv)/(iii) hinge on this mapping, and the walk pays one
/// `statat` for every entry it sends to [`Kind::Unknown`]. The W0.2 spike
/// mapped every known non-directory there by accident and its syscall counter
/// went from 55 to 116,182 on corpus a with the output unchanged, so a
/// regression here is invisible in the listing and expensive in the kernel.
#[test]
fn only_a_genuinely_untyped_entry_classifies_as_unknown() {
    assert_eq!(Kind::from_file_type(FileType::Directory), Kind::Dir);
    assert_eq!(Kind::from_file_type(FileType::Symlink), Kind::Symlink);
    assert_eq!(Kind::from_file_type(FileType::Unknown), Kind::Unknown);

    for known in [
        FileType::RegularFile,
        FileType::Fifo,
        FileType::Socket,
        FileType::CharacterDevice,
        FileType::BlockDevice,
    ] {
        assert_eq!(
            Kind::from_file_type(known),
            Kind::Other,
            "{known:?} is typed, so it must not cost a classification syscall"
        );
    }
}

/// The expression the untyped-entry fallback runs: a non-following `statat`
/// whose `st_mode` is mapped by the same classifier. It is written out here
/// against real files because no fixture on this host can reach the fallback
/// itself — APFS types every entry — so this is the only place the mode path
/// is exercised before a filesystem that returns `DT_UNKNOWN` (overlayfs,
/// some network mounts) reaches it in production.
#[test]
fn a_stat_mode_classifies_what_readdir_could_not_type() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join("dir")).expect("mkdir");
    std::fs::write(tmp.path().join("file"), b"x").expect("write");
    std::os::unix::fs::symlink("dir", tmp.path().join("link")).expect("symlink");

    let classify = |name: &str| {
        let stat =
            rustix::fs::statat(rustix::fs::CWD, tmp.path().join(name), AtFlags::SYMLINK_NOFOLLOW)
                .expect("statat");
        Kind::from_file_type(FileType::from_raw_mode(stat.st_mode))
    };

    assert_eq!(classify("dir"), Kind::Dir);
    assert_eq!(classify("file"), Kind::Other);
    // Not followed, so the link classifies as a link and rule (iii) — never
    // descend, resolve once to decide — gets to run on it.
    assert_eq!(classify("link"), Kind::Symlink);
}

#[test]
fn entry_buf_drops_dot_and_dotdot_on_the_way_in() {
    // Dropped here rather than at the point of use so that no classification
    // path can forget them and walk the directory into itself.
    let mut buf = EntryBuf::default();
    buf.push(b".", Kind::Dir);
    buf.push(b"..", Kind::Dir);
    buf.push(b".git", Kind::Dir);
    buf.push(b".hidden", Kind::Other);

    assert_eq!(buf.len(), 2);
    assert_eq!(buf.name(0), b".git");
    assert_eq!(buf.name(1), b".hidden");
}

#[test]
fn entry_buf_addresses_each_name_and_kind_by_index() {
    let mut buf = EntryBuf::default();
    buf.push(b"a", Kind::Dir);
    buf.push(b"bb", Kind::Symlink);
    buf.push(b"\xff\xfe", Kind::Unknown);
    buf.push(b"cccc", Kind::Other);

    assert_eq!(buf.len(), 4);
    assert_eq!(
        (0..buf.len()).map(|i| buf.name(i)).collect::<Vec<_>>(),
        vec![&b"a"[..], b"bb", b"\xff\xfe", b"cccc"],
        "concatenated names must stay individually addressable"
    );
    assert_eq!(
        (0..buf.len()).map(|i| buf.kind(i)).collect::<Vec<_>>(),
        vec![Kind::Dir, Kind::Symlink, Kind::Unknown, Kind::Other,]
    );
}

#[test]
fn entry_buf_clear_keeps_no_entry_from_the_previous_directory() {
    let mut buf = EntryBuf::default();
    buf.push(b"stale", Kind::Dir);
    buf.clear();
    buf.push(b"fresh", Kind::Dir);

    assert_eq!(buf.len(), 1, "a reused buffer must not leak the last directory's entries");
    assert_eq!(buf.name(0), b"fresh");
}

#[test]
fn cstr_nul_terminates_the_joined_parts() {
    let mut scratch = Vec::new();
    assert_eq!(cstr(&mut scratch, &[b"repo"]).to_bytes(), b"repo");
    // The `<child>/.git` probe of rules (iii) and the stat-first strategy is
    // built this way rather than allocating a path per candidate.
    assert_eq!(cstr(&mut scratch, &[b"repo", b"/.git"]).to_bytes(), b"repo/.git");
    // Reusing the scratch buffer must not leave the previous name behind.
    assert_eq!(cstr(&mut scratch, &[b"x"]).to_bytes(), b"x");
    assert_eq!(cstr(&mut scratch, &[b"\xff\xfe"]).to_bytes(), b"\xff\xfe");
}

#[test]
fn join_rel_separates_with_exactly_one_slash() {
    let mut sink = Vec::new();

    // A child of the root has no separator in front of it: the walk's
    // relative paths never start with `/`, or every exclusion pattern and
    // every printed line would carry a leading one.
    join_rel(&mut sink, b"", b"github.com");
    assert_eq!(sink, b"github.com");

    join_rel(&mut sink, b"github.com/zchee", b"scap");
    assert_eq!(sink, b"github.com/zchee/scap");

    join_rel(&mut sink, b"a", b"b");
    assert_eq!(sink, b"a/b", "the sink is cleared before each join");
}

#[test]
fn out_emits_a_root_repository_as_dot() {
    // ghq prints `.` for a root that is itself a repository
    // (local_repository.go:54), and the walk represents that root with an
    // empty relative path.
    let mut out = Out::default();
    out.emit(b"");
    out.emit(b"github.com/a/b");

    assert_eq!(out.arena.iter().collect::<Vec<_>>(), vec![&b"."[..], b"github.com/a/b"]);
}

/// Drives the reader directly, below `walk_root`: open the root, read it,
/// drain the queue the read produced, and take the output. This is the
/// contract the work-stealing pool consumes — one `Walker` per worker, each
/// handing back its own `Out` — so it is worth pinning at this level rather
/// than only through the public entry point.
#[test]
fn a_walker_reads_a_tree_and_hands_back_what_it_found() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("host/owner/repo/.git")).expect("repo");
    std::fs::create_dir_all(root.join("host/owner/plain")).expect("plain");
    std::fs::write(root.join("host/stray"), b"x").expect("file");

    let root_bytes = std::os::unix::ffi::OsStrExt::as_bytes(root.as_os_str());
    let live_fds = AtomicUsize::new(0);
    let ctx = Ctx {
        root: root_bytes,
        exclude: &[],
        detect: DetectStrategy::OpenScan,
        record: false,
        live_fds: &live_fds,
        fd_cap: crate::walk::FD_CAP,
    };
    let open = |path: &std::path::Path| {
        rustix::fs::openat(
            rustix::fs::CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("openat")
    };

    // The base descriptor a job without one would be re-opened from; it has
    // to outlive the loop, which is why it is not opened inside it.
    let base_fd = open(root);
    let mut walker = Walker::new(&ctx);
    let mut queue = Vec::new();
    walker.read_dir(open(root), b"", &mut queue);
    while let Some(job) = queue.pop() {
        // Every job the reader queues carries its parent's `openat` result,
        // so the descriptor budget is what bounds the queue, not the tree.
        assert!(job.fd.is_some(), "{:?} should have been opened by its parent", job.rel);
        walker.run(job, std::os::fd::AsFd::as_fd(&base_fd), &mut queue);
    }

    let out = walker.into_out();
    assert_eq!(out.arena.iter().collect::<Vec<_>>(), vec![&b"host/owner/repo"[..]]);
    assert_eq!(
        out.dirs_read, 5,
        "root, host, host/owner, host/owner/plain and the repository, which \
         open-and-scan reads to find its `.git`"
    );
    assert_eq!(out.excluded, 0);
    assert_eq!(live_fds.load(Ordering::Relaxed), 0, "every queued descriptor is handed back");
}

/// The predicate that decides whether a failed read shortened the listing,
/// and so whether ADR-10's index may be written from it.
///
/// Pinned as a table because the risk here is a later, entirely reasonable
/// simplification. `LOOP` is demonstrably redundant on Darwin — with
/// `O_DIRECTORY` alongside `O_NOFOLLOW` a symlink comes back `ENOTDIR`, so
/// removing `LOOP` keeps every test on this platform green — while on Linux
/// `O_NOFOLLOW` answers `ELOOP` for the same case, and dropping it there
/// freezes the index permanently for anyone holding a symlinked repository
/// whose target has lost its `.git`: the `openat` fails identically on every
/// future run and no mtime ever moves to break the cycle. The listing stays
/// correct throughout, so nothing else would notice.
#[test]
fn only_a_subtree_that_exists_and_went_unread_shortens_the_listing() {
    for benign in [Errno::NOENT, Errno::NOTDIR, Errno::LOOP] {
        assert!(!drops_a_subtree(benign), "{benign:?}: the path is not there to read");
    }
    for lost in [Errno::ACCESS, Errno::PERM, Errno::IO, Errno::MFILE, Errno::NFILE] {
        assert!(drops_a_subtree(lost), "{lost:?}: a subtree that exists went unread");
    }
}
