//! The readdir-floor repository walker (ADR-9, Decision B path B2).
//!
//! One call to [`walk_root`] reads a root and returns every repository under
//! it as root-relative bytes. What makes it a *floor* is that it asks the
//! kernel for nothing it can avoid: directories are opened relative to their
//! parent's descriptor, entry types come out of `readdir` rather than out of
//! a `stat` per entry, and no path is ever built as a `PathBuf`. On corpus a
//! that reduces the per-entry `stat` traffic the previous walker paid —
//! 17,771 calls — to 55.
//!
//! The rules deciding what counts as a repository are ADR-9's (i)–(viii),
//! pinned to `ghq` 1.8.0 by the oracle fixtures in `tests/`. They live in
//! `sys::Walker::read_dir`; this module owns only the root: which roots are
//! walked at all (rule vi), the root that is itself a repository (rule ii),
//! and the descriptor the walk hangs off.
//!
//! The module is unix-only by construction. `openat`, `readdir` entry types
//! and `AT_FDCWD`-relative `statat` are the mechanism, not an implementation
//! detail that could be swapped for a Windows equivalent.

mod arena;
mod pool;
mod sys;

use std::ffi::{CString, OsStr};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::AtomicUsize;

use bstr::ByteSlice;
use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags};
use rustix::io::Errno;

use self::arena::Arena;
use self::pool::MAX_THREADS;
pub(crate) use self::sys::mtime_ns;
use self::sys::{Ctx, Job, Out, Walker};

/// Worker threads a walk uses unless the caller says otherwise.
///
/// Four is `N*`: the smallest thread count whose median wall time on corpus
/// a+b came within 1.10× of the best, subject to `sys(N) ≤ 1.5 × sys(1)`.
/// Every walker variant in the W0.2 matrix selected it (plan §2, "Outcome
/// (W0.2, quiet)").
pub const DEFAULT_THREADS: usize = 4;

/// Directory descriptors the work queue may hold before queued directories
/// start carrying a path instead.
///
/// The walk opens each child from its parent's descriptor and parks the
/// descriptor in the queue, which is what keeps a directory from being opened
/// twice. The cap bounds how many of those can be outstanding: past it a
/// queued directory is re-opened from the root descriptor by its
/// root-relative path, one multi-component `openat` instead of a
/// single-component one. The W0.2 spike never reached the cap on corpora a, b
/// or a+b at any thread count.
const FD_CAP: usize = 4096;

/// Flags for the root's own descriptors.
///
/// `NOFOLLOW` is deliberately absent, unlike the child flags in `sys.rs`: a
/// root reached through a symlink is a root the user named on purpose, and
/// ghq resolves roots before walking them.
const ROOT_OFLAGS: OFlags = OFlags::RDONLY.union(OFlags::DIRECTORY).union(OFlags::CLOEXEC);

/// Which of the two `.git` detection strategies the walk uses.
///
/// They emit identical repository sets and differ only in how much I/O they
/// spend finding out, which is why the choice is measured rather than argued
/// (deviation D-6, W3.0b). Rule (iv) decides a `.git` the same way under
/// both, and the one case where they could have diverged is handled: a
/// repository whose directory is searchable but not readable (mode 0111)
/// cannot be opened, so open-and-scan falls back to stat-first's probe
/// rather than dropping it — which is also what keeps both of them matching
/// ghq, since ghq never opens a candidate at all.
///
/// `dirs_read` is therefore strategy-dependent where the repository set is
/// not: on the frozen corpus a′, open-and-scan reads 2,036 directories where
/// stat-first reads 1,196, the difference being the repositories the former
/// opens and the latter does not.
///
/// W3.0b measured the two at `N*` = 4 on a′, a and a+b and froze
/// [`StatFirst`](DetectStrategy::StatFirst) as the default. The expectation
/// written into the variants below — that opening pays for itself on a
/// directory-dense tree — did not survive contact with the corpora: an
/// `openat` that returns a descriptor costs more than a `statat` that
/// returns `ENOENT` by enough that stat-first won every corpus, including
/// the directory-dense a+b.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DetectStrategy {
    /// Open every candidate directory and look for `.git` among the entries
    /// already read. Predicted to be cheaper on directory-dense trees, where
    /// most directories have to be opened anyway — corpus a+b holds 21,360
    /// directories against 1,822 repositories — but measured slower there
    /// too (W3.0b), so it survives only as an override.
    OpenScan,
    /// `statat` the candidate's `.git` before opening it, and never open a
    /// repository. Cheaper on repository-dense trees, where the saved opens
    /// outnumber the added stats: corpus a′ holds 2,036 directories against
    /// 841 repositories. **The default**, and measured faster on every
    /// corpus, not only the repository-dense one (W3.0b).
    #[default]
    StatFirst,
}

/// The worker-thread count `SCAP_LIST_THREADS` selects, or
/// [`DEFAULT_THREADS`].
///
/// An ADR-13 divergence: ghq's walker has a fixed pool and no equivalent
/// knob. It exists because `N*` was measured on one machine's filesystem and
/// core count, and neither is universal.
pub fn threads_from_env() -> usize {
    parse_threads(std::env::var_os("SCAP_LIST_THREADS").as_deref())
}

/// Parses one `SCAP_LIST_THREADS` value.
///
/// Unset or empty means the measured default. Anything else that is not a
/// plain decimal in `1..=64` warns and falls back to it rather than failing
/// the listing: the variable tunes how a listing is produced, never whether
/// one is produced, so refusing to run over it would trade a working command
/// for a typo.
fn parse_threads(value: Option<&OsStr>) -> usize {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return DEFAULT_THREADS;
    };
    match value.to_str().and_then(|v| v.parse::<usize>().ok()) {
        Some(threads) if (1..=MAX_THREADS).contains(&threads) => threads,
        _ => {
            tracing::warn!(
                "SCAP_LIST_THREADS={}: expected a number in 1..={MAX_THREADS}; using {DEFAULT_THREADS}",
                value.as_bytes().as_bstr()
            );
            DEFAULT_THREADS
        }
    }
}

/// The detection strategy `SCAP_LIST_DETECT` selects, or the default.
///
/// An ADR-13 divergence, kept for the same reason as [`threads_from_env`]:
/// W3.0b froze the default from measurements on one machine's filesystem,
/// and a tree shaped unlike the author's — far more directories per
/// repository than corpus a+b's 11.7 — is where the losing strategy could
/// still win. ghq has no equivalent knob; it only ever `stat`s a candidate's
/// `.git`, which is what [`DetectStrategy::StatFirst`] does.
pub(crate) fn detect_strategy_from_env() -> DetectStrategy {
    parse_detect_strategy(std::env::var_os("SCAP_LIST_DETECT").as_deref())
}

/// Parses one `SCAP_LIST_DETECT` value.
///
/// Unset, empty, or unrecognised all resolve to the default, an unrecognised
/// value with a warning: a measurement knob that silently ignored its own
/// argument would produce two rows that are secretly the same row.
fn parse_detect_strategy(value: Option<&OsStr>) -> DetectStrategy {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return DetectStrategy::default();
    };
    match value.as_bytes() {
        b"open" => DetectStrategy::OpenScan,
        b"stat" => DetectStrategy::StatFirst,
        other => {
            tracing::warn!(
                "SCAP_LIST_DETECT={}: expected `open` or `stat`; using the default",
                other.as_bstr()
            );
            DetectStrategy::default()
        }
    }
}

/// One `scap.listExclude` / `SCAP_LIST_EXCLUDE` pattern, ready to match.
///
/// Patterns arrive already folded by the config snapshot — a single trailing
/// `/` removed, empty patterns dropped — so this type performs no folding of
/// its own. Folding here as well would change the meaning of a pattern
/// written `foo//`, which W2b.1 leaves matching nothing.
#[derive(Clone, Debug)]
pub struct Pattern {
    text: Box<[u8]>,
}

impl Pattern {
    /// Builds a pattern from its configured spelling.
    pub fn new(pattern: &str) -> Self {
        Self { text: pattern.as_bytes().into() }
    }

    /// The pattern's own bytes, as ADR-10's index stores them to notice that
    /// the exclusion set has changed since it was written.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.text
    }

    /// Whether `rel`, a root-relative path, is excluded by this pattern.
    ///
    /// Git's own wildmatch under `WM_PATHNAME`: `*` and `?` stop at a `/`
    /// while `**` crosses it, the match is against the whole root-relative
    /// path and so anchored at the root, and it is case-sensitive even on a
    /// case-insensitive filesystem, exactly as git is.
    pub(crate) fn matches(&self, rel: &[u8]) -> bool {
        gix_glob::wildmatch(
            self.text.as_bstr(),
            rel.as_bstr(),
            gix_glob::wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
        )
    }
}

/// How one root is to be walked.
#[derive(Clone, Debug)]
pub struct WalkOptions {
    /// Worker threads, clamped to `1..=64` by the pool; see
    /// [`DEFAULT_THREADS`] and [`threads_from_env`].
    pub threads: usize,
    /// Subtrees to prune, matched against root-relative paths (rule viii).
    pub exclude: Vec<Pattern>,
    /// Which `.git` detection strategy to use (deviation D-6).
    pub detect: DetectStrategy,
    /// Whether the walk records ADR-10 index entries as it goes.
    ///
    /// Off for a listing that is only printed: recording costs one `fstat`
    /// per directory read plus one owned path per entry, which a walk whose
    /// answer nobody will reuse has no reason to pay.
    pub record: bool,
}

impl WalkOptions {
    /// Options for a walk on `threads` workers excluding `exclude`, with the
    /// detection strategy the environment selects and no index recording.
    pub fn new(threads: usize, exclude: Vec<Pattern>) -> Self {
        Self { threads, exclude, detect: detect_strategy_from_env(), record: false }
    }

    /// The same options with index recording turned on or off.
    pub(crate) fn with_record(mut self, record: bool) -> Self {
        self.record = record;
        self
    }
}

/// One entry ADR-10's index remembers about a walk.
///
/// A *directory* entry carries the mtime the next run validates it by:
/// unchanged mtime means the directory's entry list is unchanged, so its
/// `.git` presence and its child set are unchanged too. A *repository* entry
/// carries no timestamp at all, because a repository's own mtime moves every
/// time anything in its working tree does while the only question the index
/// asks about it — is `.git` still there — is answered exactly by one
/// `statat`. Validating repositories by mtime instead would mark most of a
/// working corpus stale on every run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    rel: Box<[u8]>,
    mtime_ns: Option<i64>,
}

impl Record {
    /// A directory entry, validated by `mtime_ns`.
    pub(crate) fn dir(rel: &[u8], mtime_ns: i64) -> Self {
        Self { rel: rel.into(), mtime_ns: Some(mtime_ns) }
    }

    /// A repository entry, validated by its `.git` rather than by a
    /// timestamp.
    pub(crate) fn repo(rel: &[u8]) -> Self {
        Self { rel: rel.into(), mtime_ns: None }
    }

    /// The entry's root-relative path; empty for the root itself.
    pub(crate) fn rel(&self) -> &[u8] {
        &self.rel
    }

    /// The directory mtime in nanoseconds since the Unix epoch, or `None`
    /// for a repository entry.
    pub(crate) fn mtime_ns(&self) -> Option<i64> {
        self.mtime_ns
    }

    /// Whether this entry is a repository the walk emitted.
    pub(crate) fn is_repo(&self) -> bool {
        self.mtime_ns.is_none()
    }
}

/// What one root's walk found.
///
/// Repository paths are root-relative bytes held in one arena behind
/// `Range<u32>` handles, so the whole listing is two allocations rather than
/// one per repository. A root that is itself a repository yields the single
/// path `.`, which is what ghq prints for it.
#[derive(Default, Debug)]
pub struct RootListing {
    arena: Arena,
    dirs_read: usize,
    excluded: usize,
    records: Vec<Record>,
    incomplete: bool,
}

impl RootListing {
    /// Every repository found, in walk order.
    ///
    /// Walk order depends on scheduling, and nothing here imposes one: `list`
    /// sorts the concatenation of every root once, after filtering, which is
    /// the only ordering the output has ever had (ADR-9 rule vii). A
    /// per-root sort helper would have no caller but its own tests.
    pub fn repos(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.arena.iter()
    }

    /// Number of repositories found.
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    /// Whether the root held no repository at all.
    pub fn is_empty(&self) -> bool {
        self.arena.is_empty()
    }

    /// Directories whose entries were read.
    ///
    /// The count is strategy-dependent — see [`DetectStrategy`] — and it
    /// counts repository directories under `OpenScan`, which neither the
    /// default `StatFirst` nor the previous jwalk walker does.
    pub fn dirs_read(&self) -> usize {
        self.dirs_read
    }

    /// Directories a rule (viii) pattern pruned, neither read nor emitted.
    pub fn excluded(&self) -> usize {
        self.excluded
    }

    /// Every ADR-10 index entry the walk recorded, empty unless
    /// [`WalkOptions::record`] was set.
    ///
    /// Order follows the scheduling of the walk, so callers that need a
    /// tree sort it; the index builder does exactly that.
    pub(crate) fn records(&self) -> &[Record] {
        &self.records
    }

    /// Takes the recorded index entries, consuming the listing.
    pub(crate) fn into_records(self) -> Vec<Record> {
        self.records
    }

    /// Whether any directory or subtree was dropped instead of walked.
    ///
    /// A listing can be short for reasons the walk warns about but cannot fix
    /// — an unreadable directory, an exhausted descriptor table, an I/O error
    /// — and the caller usually does not care, because the warning already
    /// went to stderr. ADR-10's index does care: a short walk persisted as a
    /// complete one would be validated happily on every later run and would
    /// keep reproducing the short listing, silently, until some directory's
    /// mtime moved. `Cache::store` therefore writes nothing when this is set.
    pub(crate) fn incomplete(&self) -> bool {
        self.incomplete
    }

    /// Rebuilds a listing from index entries rather than from a walk.
    ///
    /// ADR-10's validation pass produces repositories it never walked to,
    /// and they have to reach `list`'s post-processing through the same type
    /// the walk produces or the two paths could print differently. A
    /// repository entry *is* a repository, so the repository set is derived
    /// here rather than carried separately — one source of truth for both
    /// the listing and the index that gets written back.
    ///
    /// `dirs_read`, `excluded` and `incomplete` describe only the directories
    /// this run actually read, which on a fully validated index is none.
    pub(crate) fn from_records(
        records: Vec<Record>,
        dirs_read: usize,
        excluded: usize,
        incomplete: bool,
    ) -> Self {
        let mut arena = Arena::default();
        for record in records.iter().filter(|record| record.is_repo()) {
            arena.push(if record.rel().is_empty() { b"." } else { record.rel() });
        }
        Self { arena, dirs_read, excluded, records, incomplete }
    }

    /// An empty listing from a walk that could not start.
    ///
    /// Distinct from [`RootListing::default`], which is a root that really
    /// held nothing: this one is a root the walk could not open, and the
    /// difference is exactly what stops an index being written from it.
    fn unreadable() -> Self {
        Self { incomplete: true, ..Self::default() }
    }

    fn from_out(out: Out) -> Self {
        Self {
            arena: out.arena,
            dirs_read: out.dirs_read,
            excluded: out.excluded,
            records: out.records,
            incomplete: out.incomplete,
        }
    }
}

/// A failure that stops a root from being walked at all.
///
/// Almost nothing qualifies: ADR-9 rule (vi) makes a missing root silent and
/// an unreadable or unstattable one a warning, because a listing that aborts
/// on one bad root is less useful than one that reports what it can. What is
/// left is a root the operating system cannot even be asked about.
#[derive(Debug, thiserror::Error)]
pub enum WalkError {
    /// The root path holds an interior NUL, so it cannot become a C string.
    #[error("{path}: root path contains an interior NUL byte")]
    InvalidRoot {
        path: String,
        #[source]
        source: std::ffi::NulError,
    },
}

/// Walks `root` and returns every repository beneath it.
///
/// Errors below the root are never propagated: an unreadable directory is
/// warned about and skipped, an unresolvable symlink is logged at debug, and
/// the walk still succeeds — `list` exits 0 in all of those cases, as ghq
/// does.
pub fn walk_root(root: &Path, opts: &WalkOptions) -> Result<RootListing, WalkError> {
    walk_root_capped(root, opts, FD_CAP)
}

/// [`walk_root`] with the descriptor budget spelled out, so tests can drive
/// the walk down the re-open path on a tree that would never reach the real
/// cap.
pub(crate) fn walk_root_capped(
    root: &Path,
    opts: &WalkOptions,
    fd_cap: usize,
) -> Result<RootListing, WalkError> {
    let root_bytes = root.as_os_str().as_bytes();
    let root_c = CString::new(root_bytes).map_err(|source| WalkError::InvalidRoot {
        path: String::from_utf8_lossy(root_bytes).into_owned(),
        source,
    })?;

    if !root_is_walkable(&root_c, root_bytes) {
        return Ok(RootListing::default());
    }

    // Rule (ii) applied to the root itself: a root named `*.git` is a bare
    // repository, prints `.` and is never opened. The test is on the name's
    // bytes, so a root whose name is not valid UTF-8 is treated like any
    // other (the previous walker decoded it first and so missed this case).
    let mut out = Out::new(opts.record);
    if root.file_name().is_some_and(|name| name.as_bytes().ends_with(b".git")) {
        out.emit(b"");
        return Ok(RootListing::from_out(out));
    }

    // Two descriptors on the root: the walk reads it through one, which the
    // directory stream consumes, and keeps the other for the whole walk as
    // the `openat` base for any directory that had to be queued by path.
    // Neither is opened `NOFOLLOW`: a root reached through a symlink is one
    // the user named deliberately, and ghq resolves roots before walking
    // them.
    let base_fd = match rustix::fs::openat(CWD, root_c.as_c_str(), ROOT_OFLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(err) => {
            warn_unwalkable_root(root_bytes, err);
            return Ok(RootListing::unreadable());
        }
    };
    let read_fd = match rustix::fs::openat(CWD, root_c.as_c_str(), ROOT_OFLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(err) => {
            warn_unwalkable_root(root_bytes, err);
            return Ok(RootListing::unreadable());
        }
    };

    let live_fds = AtomicUsize::new(0);
    let ctx = Ctx {
        root: root_bytes,
        exclude: &opts.exclude,
        detect: opts.detect,
        record: opts.record,
        live_fds: &live_fds,
        fd_cap,
    };

    // The root is read on the calling thread, so every queued job carries a
    // non-empty relative path and the workers need no special case for the
    // root. It costs one directory read.
    let mut walker = Walker::new(&ctx);
    let mut queue = Vec::new();
    walker.read_dir(read_fd, b"", &mut queue);

    let mut out = walker.into_out();
    // A root with nothing to descend into skips the pool rather than paying
    // to start threads it would immediately join. Otherwise every worker
    // returns its own output and the merge sums them, which is also where
    // the counters are added up.
    if !queue.is_empty() {
        for mut part in pool::run(&ctx, base_fd.as_fd(), queue, opts.threads) {
            out.merge(&mut part);
        }
    }

    Ok(RootListing::from_out(out))
}

/// Re-walks a set of root-relative subtrees, as ADR-10's index does when
/// validation finds an entry stale.
///
/// Each `rel` is read exactly as the walk would have read it on the way down
/// from `root` — same entry rules, same exclusions, same detection strategy —
/// and everything beneath it is walked. The paths that come back are already
/// root-relative, because the reader is told the prefix it is standing on.
///
/// The whole batch is seeded into one pool run rather than walked one
/// subtree at a time: a tree with one busy directory and forty quiet ones
/// otherwise walks the busy one on a single thread while the rest of the
/// pool idles.
///
/// `rel` must name a path the index recorded, which the entry rules only ever
/// produce for a directory that was read or a repository that was emitted. A
/// `rel` that no longer resolves contributes nothing and is not warned about:
/// the seed is opened by `Walker::run`, whose `ENOENT` reaches
/// `report_io_error` and comes out as a debug line rather than a warning, and
/// which leaves the listing complete rather than short. That is right and not
/// merely convenient — a recorded path that has since been deleted is the
/// ordinary case the re-walk exists to notice, and treating it as a loss
/// would both shout about every removed repository and stop the index being
/// rewritten afterwards. A `rel` that does resolve but cannot be read is a
/// different thing: it is warned about exactly as it would be mid-walk, and
/// it marks the listing incomplete so no index is written from it.
pub(crate) fn walk_subtrees(
    root: &Path,
    rels: &[Vec<u8>],
    opts: &WalkOptions,
) -> Result<RootListing, WalkError> {
    if rels.is_empty() {
        return Ok(RootListing::default());
    }
    let root_bytes = root.as_os_str().as_bytes();
    let root_c = CString::new(root_bytes).map_err(|source| WalkError::InvalidRoot {
        path: String::from_utf8_lossy(root_bytes).into_owned(),
        source,
    })?;
    let base_fd = match rustix::fs::openat(CWD, root_c.as_c_str(), ROOT_OFLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(err) => {
            warn_unwalkable_root(root_bytes, err);
            return Ok(RootListing::unreadable());
        }
    };

    let live_fds = AtomicUsize::new(0);
    let ctx = Ctx {
        root: root_bytes,
        exclude: &opts.exclude,
        detect: opts.detect,
        record: opts.record,
        live_fds: &live_fds,
        fd_cap: FD_CAP,
    };
    // Every seed carries a path rather than a descriptor: the walk is
    // starting part-way down and there is no parent descriptor to inherit,
    // which is the case `Job::fd == None` already exists for.
    let queue: Vec<Job> = rels.iter().map(|rel| Job { fd: None, rel: rel.clone() }).collect();

    let mut out = Out::new(opts.record);
    for mut part in pool::run(&ctx, base_fd.as_fd(), queue, opts.threads) {
        out.merge(&mut part);
    }
    Ok(RootListing::from_out(out))
}

/// ADR-9 rule (vi): whether this root is worth handing to the walker.
///
/// A root that does not exist is skipped silently — a `scap.root` naming a
/// directory the user has not created yet is not an error, and ghq skips it
/// the same way. A root that exists but cannot be read, and a root whose
/// `stat` fails for any other reason, are skipped *with* a warning: they hide
/// repositories that would otherwise be listed, so silence would make the
/// shorter output look authoritative. (ghq dereferences a nil `FileInfo` in
/// the third case and panics, so it cannot be the oracle for it — registered
/// in ADR-13.)
///
/// The `stat` follows symlinks, as the previous walker's `fs::metadata` did,
/// so a root that is a symlink to a directory is walked.
fn root_is_walkable(root_c: &CString, root_bytes: &[u8]) -> bool {
    let stat = match rustix::fs::statat(CWD, root_c.as_c_str(), AtFlags::empty()) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return false,
        Err(err) => {
            warn_unwalkable_root(root_bytes, err);
            return false;
        }
    };
    // A root that is not a directory holds no repositories and is not an
    // error: ghq's walker refuses to descend one and prints nothing
    // (`walker.go:27-35`).
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return false;
    }
    // ghq's own readability test (`local_repository.go:310-318`): any of the
    // three read bits set.
    if stat.st_mode & 0o444 == 0 {
        tracing::warn!("{}: Permission denied", root_bytes.as_bstr());
        return false;
    }
    true
}

/// Rule (vi): report a root the walk cannot use.
///
/// Unlike rule (v)'s per-entry errors this always warns, because a root scap
/// cannot walk hides every repository beneath it rather than one entry, and
/// the listing that results looks complete.
fn warn_unwalkable_root(root_bytes: &[u8], err: Errno) {
    let err = std::io::Error::from(err);
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        tracing::warn!("{}: Permission denied", root_bytes.as_bstr());
    } else {
        tracing::warn!("{}: {err}", root_bytes.as_bstr());
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
