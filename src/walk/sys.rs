//! The directory reader: ADR-9's entry semantics over safe `rustix::fs`.
//!
//! This is Decision B's **B2** reader, the one the W3.0 gate adopted. It
//! issues the same syscalls the `libc` B1 variant did — `openat`,
//! `fdopendir`, `readdir`, `fstatat`, `closedir` — through `rustix`'s safe
//! wrappers, so the crate keeps `#![deny(unsafe_code)]` and ADR-11's
//! `allow(unsafe_code)` site is never created. `libc` stays out of the
//! dependency graph entirely.
//!
//! Two constructors of `rustix::fs::Dir` exist and only one is correct here.
//! [`Dir::new`] takes the descriptor over, which is what lets the walk use
//! `Dir::fd()` as the `openat`/`fstatat` base for the directory's children
//! and so open no directory twice. `Dir::read_from`, the borrowing
//! constructor, re-opens the directory through `openat(fd, ".")` after an
//! `fcntl(F_GETFL)` so it can own the stream position (rustix 1.1.4
//! `backend/libc/fs/dir.rs:108-127`): two extra syscalls per directory, which
//! the W0.2 spike measured as a rise in sys time from 0.81 s to 1.08 s on
//! corpus a. `Dir::new` it is.
//!
//! Every entry rule lives in [`Walker::read_dir`] alone. Nothing else in the
//! walk decides what a repository is, so the pool can only change *when*
//! directories are read, never *what* the walk concludes about them.
//!
//! [`Dir::new`]: rustix::fs::Dir::new

use std::ffi::CStr;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicUsize, Ordering};

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;

use super::arena::Arena;
use super::{DetectStrategy, Pattern, Record};

/// Flags for every directory the walk opens *below* the root.
///
/// `NOFOLLOW` is what makes ADR-9 rule (iii) hold under a race: the walk only
/// ever opens an entry `readdir` typed as a directory, but between that read
/// and the `openat` the name can become a symlink, and following it would
/// descend a link ghq never descends. The root itself is opened without it —
/// a root reached through a symlink is a root the user named on purpose, and
/// ghq resolves roots before walking them (`local_repository.go:394-397`).
const CHILD_OFLAGS: OFlags =
    OFlags::RDONLY.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

/// What one `readdir` entry, or one `statat`, turned out to be.
///
/// `Other` covers regular files, gitfiles, FIFOs, sockets and devices
/// together: past "not a directory and not a symlink" the walk never needs to
/// tell them apart, and ghq does not either — it only asks whether a `stat`
/// of `<dir>/.git` succeeded (`local_repository.go:251`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Dir,
    Symlink,
    Other,
    /// The filesystem did not report a type, so classifying this entry costs
    /// one `statat`. APFS and ext4 always report one; overlayfs and some
    /// network filesystems do not.
    Unknown,
}

impl Kind {
    /// Maps a `rustix` file type onto the three cases the walk distinguishes.
    ///
    /// Only a genuinely untyped entry may become [`Kind::Unknown`]. Mapping
    /// known non-directory types there instead would send every regular file
    /// down the `statat` classification path: the W0.2 spike did exactly that
    /// by accident and its syscall counter read 116,182 instead of 55 on
    /// corpus a.
    fn from_file_type(file_type: FileType) -> Self {
        match file_type {
            FileType::Directory => Self::Dir,
            FileType::Symlink => Self::Symlink,
            FileType::Unknown => Self::Unknown,
            _ => Self::Other,
        }
    }
}

/// One directory's entries, in a buffer reused for every directory a worker
/// reads.
///
/// Names are copied out of the `readdir` stream as bytes and concatenated,
/// so reading a directory of 8 entries costs no allocation once the buffer
/// has warmed up. `.` and `..` are dropped on the way in, where no caller can
/// forget them and walk into a cycle.
#[derive(Default, Debug)]
struct EntryBuf {
    names: Vec<u8>,
    ents: Vec<Ent>,
}

#[derive(Clone, Copy, Debug)]
struct Ent {
    off: u32,
    len: u32,
    kind: Kind,
}

impl EntryBuf {
    /// Drops every entry, keeping the buffers for the next directory.
    fn clear(&mut self) {
        self.names.clear();
        self.ents.clear();
    }

    /// Appends one entry, skipping `.` and `..`.
    ///
    /// # Panics
    ///
    /// Panics if one directory's names exceed 4 GiB in total.
    fn push(&mut self, name: &[u8], kind: Kind) {
        if name == b"." || name == b".." {
            return;
        }
        let off = u32::try_from(self.names.len()).expect("directory name buffer grew past 4 GiB");
        let len = u32::try_from(name.len()).expect("directory entry name longer than 4 GiB");
        self.names.extend_from_slice(name);
        self.ents.push(Ent { off, len, kind });
    }

    /// Number of entries held, `.` and `..` excluded.
    fn len(&self) -> usize {
        self.ents.len()
    }

    /// The `i`th entry's name.
    ///
    /// # Panics
    ///
    /// Panics if `i` is out of bounds.
    fn name(&self, i: usize) -> &[u8] {
        let ent = self.ents[i];
        &self.names[ent.off as usize..(ent.off + ent.len) as usize]
    }

    /// The `i`th entry's type as `readdir` reported it.
    ///
    /// # Panics
    ///
    /// Panics if `i` is out of bounds.
    fn kind(&self, i: usize) -> Kind {
        self.ents[i].kind
    }
}

/// A directory the walk has found but not yet read.
///
/// It normally carries the descriptor its parent already opened, because the
/// parent held the only cheap base for a single-component `openat`. Past the
/// descriptor budget — or when the process has run out of descriptors
/// outright — it carries only its root-relative path and is re-opened from
/// the root descriptor instead, which costs one multi-component `openat` and
/// keeps the walk inside any `RLIMIT_NOFILE`.
#[derive(Debug)]
pub(crate) struct Job {
    pub(crate) fd: Option<OwnedFd>,
    pub(crate) rel: Vec<u8>,
}

/// Everything one worker produces: the repositories it found and the two
/// counters ADR-9 puts on the `scap::walk::root` span.
///
/// The counters are plain integers rather than shared atomics because each
/// worker owns its `Out` outright and the merge that collects them is the
/// natural place to add them up. That keeps the per-entry hot loop free of
/// atomic traffic without losing anything: `dirs_read` and `excluded` are
/// only ever read after the walk.
#[derive(Default, Debug)]
pub(crate) struct Out {
    pub(crate) arena: Arena,
    pub(crate) dirs_read: usize,
    pub(crate) excluded: usize,
    /// ADR-10's index entries, empty unless the walk was asked to record
    /// them. Exactly one per path the walk decided about — a directory it
    /// read, or a repository it emitted, never both — which is the set the
    /// index has to `statat` on the next run.
    pub(crate) records: Vec<Record>,
    /// Whether any directory or subtree was dropped rather than walked.
    ///
    /// A permission error, an exhausted descriptor table or an I/O failure
    /// leaves the listing short, which the walk has always warned about. What
    /// ADR-10 adds is that a short walk must not be *persisted* as a complete
    /// one: an index built from it would validate its surviving entries
    /// happily and keep reproducing the shortened listing until something
    /// moved a directory's mtime. Set at every site that drops a subtree,
    /// read by `Cache::store`, which then writes nothing.
    pub(crate) incomplete: bool,
    /// Whether [`Out::emit`] and [`Walker::read_dir`] append to `records`.
    record: bool,
}

impl Out {
    /// An output that records ADR-10 index entries as it goes, or one that
    /// does not.
    pub(crate) fn new(record: bool) -> Self {
        Self { record, ..Self::default() }
    }

    /// Records one repository at root-relative path `rel`.
    ///
    /// A root that is itself a repository has an empty relative path and is
    /// printed as `.`, which is what ghq prints for it
    /// (`local_repository.go:54`).
    ///
    /// The index entry keeps the *unsubstituted* `rel`, because that is what
    /// the validation pass hands to `statat` relative to the root
    /// descriptor; only the printed listing gets the `.`.
    pub(crate) fn emit(&mut self, rel: &[u8]) {
        self.arena.push(if rel.is_empty() { b"." } else { rel });
        if self.record {
            self.records.push(Record::repo(rel));
        }
    }

    /// Records that a directory or subtree was dropped instead of walked.
    fn note_incomplete(&mut self) {
        self.incomplete = true;
    }

    /// Records one directory the walk read and rule (i) did not turn into a
    /// repository, with the mtime the index validates it by.
    fn note_dir(&mut self, rel: &[u8], mtime_ns: i64) {
        if self.record {
            self.records.push(Record::dir(rel, mtime_ns));
        }
    }

    /// Folds another worker's output into this one.
    ///
    /// This is where the counters are summed, which is why they are plain
    /// integers: each worker owns its own, so the per-entry path never
    /// touches an atomic.
    pub(crate) fn merge(&mut self, other: &mut Out) {
        self.arena.merge(&other.arena);
        self.dirs_read += other.dirs_read;
        self.excluded += other.excluded;
        self.incomplete |= other.incomplete;
        self.records.append(&mut other.records);
    }
}

/// Immutable state every worker shares for the length of one root's walk.
pub(crate) struct Ctx<'a> {
    /// The root exactly as the caller gave it, used only to render absolute
    /// paths in warnings so they read the way ghq's do.
    pub(crate) root: &'a [u8],
    /// ADR-9 rule (viii) patterns, already folded by the config snapshot.
    pub(crate) exclude: &'a [Pattern],
    /// Which of the two `.git` detection strategies to use (deviation D-6).
    pub(crate) detect: DetectStrategy,
    /// Whether this walk is building an ADR-10 index as it goes. Recording
    /// costs one `fstat` per directory read and nothing per repository, and
    /// is off for every walk whose result is only printed.
    pub(crate) record: bool,
    /// Descriptors currently parked in the work queue, across all workers.
    pub(crate) live_fds: &'a AtomicUsize,
    /// The value of `live_fds` past which queued directories carry a path
    /// instead of a descriptor.
    ///
    /// A soft bound, not a hard one: the check and the `openat` that follows
    /// it are not atomic, so with `N` workers the parked total can reach
    /// about `fd_cap + N - 1`. On top of that each worker holds one
    /// descriptor for the directory it is reading, so the walk's own peak is
    /// roughly `fd_cap + 2N`. Both are far below any `RLIMIT_NOFILE` worth
    /// having at the default cap, and the EMFILE arms make the real limit
    /// arriving early a re-queue rather than a lost subtree.
    pub(crate) fd_cap: usize,
}

/// One worker's reusable buffers plus the output it is filling.
pub(crate) struct Walker<'a> {
    ctx: &'a Ctx<'a>,
    buf: EntryBuf,
    /// NUL-terminated path fragment handed to `openat`/`statat`.
    name_z: Vec<u8>,
    /// Root-relative path of the child currently being classified.
    rel: Vec<u8>,
    out: Out,
}

impl<'a> Walker<'a> {
    pub(crate) fn new(ctx: &'a Ctx<'a>) -> Self {
        Self {
            ctx,
            buf: EntryBuf::default(),
            name_z: Vec::new(),
            rel: Vec::new(),
            out: Out::new(ctx.record),
        }
    }

    /// Takes the output this worker accumulated, ending its walk.
    pub(crate) fn into_out(self) -> Out {
        self.out
    }

    /// Reads the directory `job` names and appends its unread children to
    /// `children`.
    ///
    /// `root_fd` is the walk root's descriptor, used only when the job
    /// carries a path rather than a descriptor.
    pub(crate) fn run(&mut self, job: Job, root_fd: BorrowedFd<'_>, children: &mut Vec<Job>) {
        let fd = match job.fd {
            Some(fd) => {
                self.ctx.live_fds.fetch_sub(1, Ordering::Relaxed);
                fd
            }
            None => {
                // A multi-component `openat` from the root. `NOFOLLOW` binds
                // only the last component, so an intermediate directory that
                // became a symlink after it was walked would be followed
                // here where the descriptor path would not have been. The
                // queued child was `readdir`-typed as a directory, so only a
                // queue-to-reopen race can differ, and the descriptor path —
                // which is every directory below the cap — is immune.
                let path = cstr(&mut self.name_z, &[&job.rel]);
                match rustix::fs::openat(root_fd, path, CHILD_OFLAGS, Mode::empty()) {
                    Ok(fd) => fd,
                    // Nothing left to fall back to: the job already gave its
                    // descriptor up, so this subtree is dropped and the
                    // listing is short. A standing limitation, and the reason
                    // it is loud even though ghq has no equivalent condition
                    // — a silently short listing is the one failure a
                    // repository lister must not have. Retrying the job once
                    // the queue has drained would remove it; that is a
                    // follow-up, and the warning is today's whole mitigation.
                    Err(err @ (Errno::MFILE | Errno::NFILE)) => {
                        tracing::warn!(
                            "{}: {}; subtree skipped",
                            abs(self.ctx.root, &job.rel),
                            std::io::Error::from(err)
                        );
                        self.out.note_incomplete();
                        return;
                    }
                    Err(err) => {
                        report_io_error(self.ctx.root, &job.rel, err);
                        if drops_a_subtree(err) {
                            self.out.note_incomplete();
                        }
                        return;
                    }
                }
            }
        };
        self.read_dir(fd, &job.rel, children);
    }

    /// Reads one directory and applies ADR-9 rules (i)–(iv) and (viii) to
    /// every entry in it.
    ///
    /// `fd` is consumed: the directory stream owns it for the length of this
    /// call and closes it on the way out, and while the stream is alive its
    /// descriptor is the `openat`/`statat` base for every child below.
    pub(crate) fn read_dir(&mut self, fd: OwnedFd, rel: &[u8], children: &mut Vec<Job>) {
        self.buf.clear();
        let mut dir = match Dir::new(fd) {
            Ok(dir) => dir,
            Err(err) => {
                report_io_error(self.ctx.root, rel, err);
                if drops_a_subtree(err) {
                    self.out.note_incomplete();
                }
                return;
            }
        };
        // ADR-10's mtime is read *before* the entry list, off a descriptor the
        // walk already holds. The order is the correctness argument: a
        // directory changed between the two reads carries a timestamp newer
        // than the one recorded here, so the next run sees a mismatch and
        // re-walks it. Reading it afterwards would pair the new timestamp with
        // the old entry list, which is the one way this index could produce a
        // false hit. A failed `fstat` records [`NEVER_VALID`], which no
        // observed mtime can equal.
        let recorded_mtime = if self.ctx.record {
            match dir.fd().and_then(rustix::fs::fstat) {
                Ok(stat) => mtime_ns(&stat),
                Err(_) => NEVER_VALID,
            }
        } else {
            NEVER_VALID
        };
        while let Some(entry) = dir.read() {
            match entry {
                Ok(entry) => self
                    .buf
                    .push(entry.file_name().to_bytes(), Kind::from_file_type(entry.file_type())),
                // A mid-stream failure leaves a truncated entry list, and
                // rule (i) run over one would be a guess: a `.git` the walk
                // never saw would turn a repository into a directory and
                // release everything under it. The directory counts as
                // unreadable instead — reported, skipped, not counted in
                // `dirs_read`, exactly as a failed open is.
                Err(err) => {
                    report_io_error(self.ctx.root, rel, err);
                    self.out.note_incomplete();
                    return;
                }
            }
        }
        let Ok(dirfd) = dir.fd() else {
            // `dirfd(3)` on a stream this function just built. Unreachable in
            // practice; skipping the directory is still better than a panic
            // in a command whose whole job is to print what it can.
            tracing::debug!("{}: directory stream lost its descriptor", abs(self.ctx.root, rel));
            self.out.note_incomplete();
            return;
        };
        self.out.dirs_read += 1;

        // Rule (i): the whole entry list decides whether this directory is a
        // repository *before* any child is emitted or queued. Doing it per
        // entry instead lets a child that sorts ahead of `.git` escape from
        // inside a repository, which is the §0 prototype's defect.
        if is_repo_dir(&self.buf, dirfd) {
            self.out.emit(rel);
            return;
        }
        // Only here, and not before rule (i): a directory that turned out to
        // be a repository is a repository entry and nothing else. Recording
        // both would put two entries under one path in the index, and which
        // of them survived the build's de-duplication would decide whether
        // the repository is listed at all on the next run.
        self.out.note_dir(rel, recorded_mtime);

        // Split borrows: the entry name borrows `buf` while the two scratch
        // buffers are written, and the three are separate fields.
        let Self { ctx, buf, name_z, rel: relbuf, out } = self;
        for i in 0..buf.len() {
            let name = buf.name(i);
            let mut kind = buf.kind(i);

            if kind == Kind::Unknown {
                // ghq `Lstat`s every entry unconditionally (saracen
                // `walker_unix.go:36`); a typed `readdir` is what lets the
                // walk skip that, so only the untyped remainder pays.
                kind = match rustix::fs::statat(
                    dirfd,
                    cstr(name_z, &[name]),
                    AtFlags::SYMLINK_NOFOLLOW,
                ) {
                    Ok(stat) => Kind::from_file_type(FileType::from_raw_mode(stat.st_mode)),
                    Err(_) => continue,
                };
            }

            match kind {
                Kind::Dir => {
                    join_rel(relbuf, rel, name);
                    // Rule (viii): the exclusion test runs before anything
                    // else touches the child, so an excluded subtree costs
                    // neither a `stat` nor an `open`.
                    if is_excluded(ctx.exclude, relbuf) {
                        out.excluded += 1;
                        continue;
                    }
                    // Rule (ii): a directory whose own name ends in `.git` is
                    // a repository and is never opened.
                    if name.ends_with(b".git") {
                        out.emit(relbuf);
                        continue;
                    }
                    // Deviation D-6: stat-first trades one `statat` per child
                    // directory for never opening a repository. It wins on
                    // repository-dense trees and loses on directory-dense
                    // ones, so which one ships is a measured decision
                    // (W3.0b) rather than a coded-in one. Rule (iv) is
                    // unaffected: this `statat` follows symlinks, so a
                    // dangling `.git` link fails it here exactly as it fails
                    // the entry-list test below.
                    if ctx.detect == DetectStrategy::StatFirst
                        && rustix::fs::statat(
                            dirfd,
                            cstr(name_z, &[name, b"/.git"]),
                            AtFlags::empty(),
                        )
                        .is_ok()
                    {
                        out.emit(relbuf);
                        continue;
                    }
                    match queue_child(ctx, dirfd, name, relbuf, name_z, children) {
                        Child::Repository => out.emit(relbuf),
                        Child::Settled => {}
                        Child::Dropped => out.note_incomplete(),
                    }
                }
                Kind::Symlink => {
                    // Rule (iii): a symlink is never descended, but it is a
                    // repository candidate. ghq resolves it for `IsDir`, then
                    // stats `<link>/.git` and reports the *link's* own path
                    // (`local_repository.go:268-299`, `walker.go:85-90`).
                    join_rel(relbuf, rel, name);
                    if is_excluded(ctx.exclude, relbuf) {
                        out.excluded += 1;
                        continue;
                    }
                    match rustix::fs::statat(dirfd, cstr(name_z, &[name]), AtFlags::empty()) {
                        Ok(stat) => {
                            if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
                                continue;
                            }
                        }
                        // A dangling link, a loop, or a link into a directory
                        // this process cannot search. ghq prints nothing for
                        // any of them, and the maintainer's own corpus holds
                        // a stale documentation link, so a warning here would
                        // put a line on every `scap list` and teach users to
                        // stop reading its stderr.
                        Err(err) => {
                            report_symlink_error(ctx.root, relbuf, err);
                            continue;
                        }
                    }
                    // The `.git` suffix is tested against the link's own
                    // name, never the target's: `link.git -> plaindir` is a
                    // repository and `link -> upstream.git` is not (W0.4
                    // case 7).
                    if name.ends_with(b".git") {
                        out.emit(relbuf);
                        continue;
                    }
                    if rustix::fs::statat(dirfd, cstr(name_z, &[name, b"/.git"]), AtFlags::empty())
                        .is_ok()
                    {
                        out.emit(relbuf);
                    }
                }
                Kind::Other | Kind::Unknown => {}
            }
        }
    }
}

/// The mtime a directory entry carries when the walk could not read one.
///
/// No filesystem timestamp reaches it, so an entry holding it can never
/// validate and its subtree is read afresh every run — the safe direction
/// for a cache whose only failure mode worth fearing is a false hit.
pub(crate) const NEVER_VALID: i64 = i64::MIN;

/// One `stat`'s modification time as nanoseconds since the Unix epoch.
///
/// `i64` nanoseconds spans 1678–2262, which covers every timestamp a
/// filesystem can hold without the 16 bytes a 128-bit count would add to
/// every index entry. The two saturating steps are what keep a filesystem
/// reporting a nonsense timestamp from wrapping into a *plausible* one: a
/// saturated value simply never matches, so the entry is re-walked.
pub(crate) fn mtime_ns(stat: &rustix::fs::Stat) -> i64 {
    let secs = widen(stat.st_mtime).unwrap_or(NEVER_VALID);
    let nsecs = widen(stat.st_mtime_nsec).unwrap_or(0);
    secs.saturating_mul(1_000_000_000).saturating_add(nsecs)
}

/// A `stat` timestamp field widened to `i64`.
///
/// `time_t` and the nanosecond field differ in width and signedness across
/// the unixes scap builds for — already `i64` on the two this ships on, `i32`
/// on a 32-bit target — so the conversion has to be written generically or it
/// is either a lint on one platform or a truncation on another.
fn widen<T: TryInto<i64>>(value: T) -> Option<i64> {
    value.try_into().ok()
}

/// Rule (i)/(iv): does this directory's entry list make it a repository?
///
/// `.git` as a directory, a gitfile, or anything else the filesystem could
/// type is decided without a syscall, because ghq's `os.Stat` would have
/// succeeded for all of them. Only a `.git` symlink or an untyped `.git`
/// costs one following `statat`, which is what makes a dangling `.git`
/// symlink not a repository (`local_repository.go:251`).
fn is_repo_dir(buf: &EntryBuf, dirfd: BorrowedFd<'_>) -> bool {
    for i in 0..buf.len() {
        if buf.name(i) != b".git" {
            continue;
        }
        return match buf.kind(i) {
            Kind::Symlink | Kind::Unknown => {
                rustix::fs::statat(dirfd, c".git", AtFlags::empty()).is_ok()
            }
            _ => true,
        };
    }
    false
}

/// Opens a child directory and queues it, or settles it without opening.
///
/// See [`Child`] for the three outcomes. The one worth naming here is
/// [`Child::Repository`]: a child the walk could not open but whose `.git`
/// stats is a repository it must emit without reading, which is the one case
/// where opening it was neither possible nor necessary.
fn queue_child(
    ctx: &Ctx<'_>,
    dirfd: BorrowedFd<'_>,
    name: &[u8],
    rel: &[u8],
    name_z: &mut Vec<u8>,
    children: &mut Vec<Job>,
) -> Child {
    if ctx.live_fds.load(Ordering::Relaxed) >= ctx.fd_cap {
        children.push(Job { fd: None, rel: rel.to_vec() });
        return Child::Settled;
    }
    match rustix::fs::openat(dirfd, cstr(name_z, &[name]), CHILD_OFLAGS, Mode::empty()) {
        Ok(fd) => {
            ctx.live_fds.fetch_add(1, Ordering::Relaxed);
            children.push(Job { fd: Some(fd), rel: rel.to_vec() });
            Child::Settled
        }
        // The descriptor budget is a self-imposed bound; these two are the
        // real one arriving early, from a low `RLIMIT_NOFILE` or from the
        // rest of the process. Queueing by path costs one `openat` from the
        // root later, by which time the queue has given descriptors back —
        // dropping the subtree here would silently shorten the listing.
        Err(Errno::MFILE | Errno::NFILE) => {
            children.push(Job { fd: None, rel: rel.to_vec() });
            Child::Settled
        }
        // Reading a directory needs its read bit; `stat`ping through it needs
        // only its search bit. ghq never opens a candidate — it stats
        // `<dir>/.git` (local_repository.go:251) — so a repository whose
        // directory is mode 0111 or 0311 is one ghq lists, silently, and one
        // this walk would drop if the failed `openat` were the last word.
        // The stat-first strategy already decides it before reaching here, so
        // without this probe the two strategies would disagree on a real
        // corpus and the W3.0b default would carry a semantic difference.
        // Verified against ghq 1.8.0: it prints the mode-0111 repository and
        // warns only for the mode-0111 plain directory, which is what the
        // fall-through below does.
        Err(err @ (Errno::ACCESS | Errno::PERM)) => {
            if rustix::fs::statat(dirfd, cstr(name_z, &[name, b"/.git"]), AtFlags::empty()).is_ok()
            {
                return Child::Repository;
            }
            report_io_error(ctx.root, rel, err);
            Child::Dropped
        }
        Err(err) => {
            report_io_error(ctx.root, rel, err);
            if drops_a_subtree(err) { Child::Dropped } else { Child::Settled }
        }
    }
}

/// What [`queue_child`] did with one child directory.
///
/// Three outcomes rather than a boolean, because ADR-10 needs to tell the two
/// falsehoods apart: a child handed to the queue leaves a complete walk, and
/// a child dropped after an error does not. The distinction used to be
/// invisible — both returned `false` — and an index written from the second
/// would have recorded a short listing as the whole truth.
enum Child {
    /// Rule (i)/(iv) decided it from outside: the directory could not be
    /// opened but `<dir>/.git` stats, which is the mode-0111 repository ghq
    /// lists too.
    Repository,
    /// Handed to the queue, or gone from the tree between the `readdir` and
    /// the `openat`. Either way the walk lost nothing: a name that is no
    /// longer there contributes nothing to a listing of what *is* there, and
    /// its removal already moved this directory's mtime.
    Settled,
    /// Neither read nor queued, and it exists. The listing is short by this
    /// subtree, the walk has said so on stderr, and no index may be written
    /// from this walk.
    Dropped,
}

/// Whether an error that stopped the walk reading a path left the listing
/// *short*, as opposed to correctly empty.
///
/// Three errors mean "there is nothing here to read", and nothing was
/// dropped when the walk declines to read it. `ENOENT` is a path deleted
/// since it was seen. `ENOTDIR` is one replaced by a non-directory. `ELOOP`,
/// against these flags, is not a symlink loop at all but the kernel saying
/// "the last component is a symlink" — and the walk never descends a
/// symlink, so declining it is rule (iii) working, not a loss. Each of the
/// three is also self-announcing: the change that caused it happened inside
/// the parent directory, which moved the parent's mtime, so the next
/// validation pass re-reads it through the ordinary miss path.
///
/// Which of the last two a symlink produces is the platform's choice, and
/// both have to be here for the same reason. `CHILD_OFLAGS` carries
/// `O_DIRECTORY` as well as `O_NOFOLLOW`, and Linux reports `ELOOP` for a
/// symlink under `O_NOFOLLOW` while Darwin reports `ENOTDIR` for one under
/// `O_DIRECTORY` — measured, not assumed: on Darwin every symlink kind
/// (to a directory, to a file, dangling) comes back `ENOTDIR` with these
/// flags and `ELOOP` only without `O_DIRECTORY`. Excluding just one of the
/// two would leave the other platform's index frozen for anyone holding a
/// symlinked repository whose target has lost its `.git`, because that
/// `openat` fails identically on every future run and no mtime ever moves
/// to break the cycle.
///
/// Every other error means a subtree that does exist went unread — a
/// permission denied, an I/O failure, an exhausted descriptor table — and
/// only those may stop ADR-10's index being written.
///
/// Getting this backwards is expensive in a quiet way, which is why the
/// exclusions are not simply "errors we can ignore": treating `ENOENT` as a
/// loss would suppress the index rewrite on every run that merely found a
/// repository deleted. The listing stays correct throughout, so the damage
/// would be invisible.
///
/// The accepted cost is on the other side: a genuine too-many-symbolic-links
/// failure on an *intermediate* component is read as benign here and does not
/// mark the walk short. That is rare, and it is the deliberate trade.
fn drops_a_subtree(err: Errno) -> bool {
    !matches!(err, Errno::NOENT | Errno::NOTDIR | Errno::LOOP)
}

/// ADR-9 rule (viii): whether `rel` matches any configured exclusion.
///
/// The matcher is W2b.1's, unchanged: git's own wildmatch under
/// `NO_MATCH_SLASH_LITERAL` (`WM_PATHNAME`, so `*` and `?` stop at a `/`
/// while `**` crosses it), matched against the whole root-relative path and
/// therefore anchored at the root, and case-sensitive because git's is. The
/// single trailing `/` a pattern may carry was already folded away when the
/// config snapshot was built, so patterns arrive here in matcher form.
fn is_excluded(patterns: &[Pattern], rel: &[u8]) -> bool {
    patterns.iter().any(|pattern| pattern.matches(rel))
}

/// Joins `rel` and `name` into `sink`, which is cleared first.
fn join_rel(sink: &mut Vec<u8>, rel: &[u8], name: &[u8]) {
    sink.clear();
    if !rel.is_empty() {
        sink.extend_from_slice(rel);
        sink.push(b'/');
    }
    sink.extend_from_slice(name);
}

/// Builds `<parts…>\0` in `scratch` and hands it back as a `CStr`.
///
/// `rustix`'s path arguments take a `&CStr` without copying it, so building
/// the NUL-terminated form in a buffer the worker reuses is what keeps the
/// per-entry path free of allocation.
///
/// # Panics
///
/// Panics if any part contains an interior NUL, which no filesystem name
/// can.
fn cstr<'a>(scratch: &'a mut Vec<u8>, parts: &[&[u8]]) -> &'a CStr {
    scratch.clear();
    for part in parts {
        scratch.extend_from_slice(part);
    }
    scratch.push(0);
    CStr::from_bytes_with_nul(scratch).expect("filesystem name contained an interior NUL")
}

/// Renders `<root>/<rel>` for a diagnostic.
///
/// Warnings name the absolute path because ghq's do, and because a
/// root-relative path in a message about a multi-root listing says nothing
/// about which root it came from. Undecodable bytes become U+FFFD, matching
/// what `Path::display` would have printed.
fn abs(root: &[u8], rel: &[u8]) -> String {
    let mut path = String::from_utf8_lossy(root).into_owned();
    if !rel.is_empty() {
        path.push('/');
        path.push_str(&String::from_utf8_lossy(rel));
    }
    path
}

/// ADR-9 rule (v): report a directory the walk could not read, then carry on
/// with exit status 0.
///
/// Only a permission error reaches stderr by default, carrying ghq's own
/// wording rather than the platform's `strerror` so the text does not vary by
/// OS (`local_repository.go:301-306`). Everything else is a debug line: ghq
/// prints nothing for those, and a listing that warns about entries ghq
/// ignores teaches users to stop reading its stderr.
fn report_io_error(root: &[u8], rel: &[u8], err: Errno) {
    let err = std::io::Error::from(err);
    let path = abs(root, rel);
    // EACCES and EPERM both map to `PermissionDenied`.
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        tracing::warn!("{path}: Permission denied");
    } else {
        tracing::debug!("{path}: {err}");
    }
}

/// Rule (iii): report a symlink the walk could not resolve.
///
/// Always at debug level, including for a permission error, because ghq
/// emits nothing for an unresolvable link of any kind — it is not a
/// directory it failed to read, it is a candidate that turned out not to be
/// one. This is W2b.1's ruling, kept so `list`'s stderr does not change under
/// the new walker.
fn report_symlink_error(root: &[u8], rel: &[u8], err: Errno) {
    tracing::debug!("{}: {}", abs(root, rel), std::io::Error::from(err));
}

#[cfg(test)]
#[path = "sys_tests.rs"]
mod tests;
