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
use self::sys::{Ctx, Out, Walker};

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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DetectStrategy {
    /// Open every candidate directory and look for `.git` among the entries
    /// already read. Cheaper on directory-dense trees, where most directories
    /// have to be opened anyway: corpus a+b holds 21,360 directories against
    /// 1,822 repositories.
    #[default]
    OpenScan,
    /// `statat` the candidate's `.git` before opening it, and never open a
    /// repository. Cheaper on repository-dense trees, where the saved opens
    /// outnumber the added stats: corpus a′ holds 2,036 directories against
    /// 841 repositories.
    StatFirst,
}

/// The detection strategy `SCAP_LIST_DETECT` selects, or the default.
///
/// The variable exists to measure the two strategies against each other on
/// the real corpora (W3.0b) and is deliberately undocumented until that
/// measurement freezes a default; it is not part of scap's supported surface
/// and carries no ADR-13 row yet.
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
    /// Worker threads. Clamped to `1..=64`; see [`DEFAULT_THREADS`].
    pub threads: usize,
    /// Subtrees to prune, matched against root-relative paths (rule viii).
    pub exclude: Vec<Pattern>,
    /// Which `.git` detection strategy to use (deviation D-6).
    pub detect: DetectStrategy,
}

impl WalkOptions {
    /// Options for a walk on `threads` workers excluding `exclude`, with the
    /// detection strategy the environment selects.
    pub fn new(threads: usize, exclude: Vec<Pattern>) -> Self {
        Self { threads, exclude, detect: detect_strategy_from_env() }
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
    /// counts repository directories under `OpenScan`, which the previous
    /// jwalk walker did not.
    pub fn dirs_read(&self) -> usize {
        self.dirs_read
    }

    /// Directories a rule (viii) pattern pruned, neither read nor emitted.
    pub fn excluded(&self) -> usize {
        self.excluded
    }

    fn from_out(out: Out) -> Self {
        Self { arena: out.arena, dirs_read: out.dirs_read, excluded: out.excluded }
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
    let mut out = Out::default();
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
    let root_oflags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let base_fd = match rustix::fs::openat(CWD, root_c.as_c_str(), root_oflags, Mode::empty()) {
        Ok(fd) => fd,
        Err(err) => {
            warn_unwalkable_root(root_bytes, err);
            return Ok(RootListing::default());
        }
    };
    let read_fd = match rustix::fs::openat(CWD, root_c.as_c_str(), root_oflags, Mode::empty()) {
        Ok(fd) => fd,
        Err(err) => {
            warn_unwalkable_root(root_bytes, err);
            return Ok(RootListing::default());
        }
    };

    let live_fds = AtomicUsize::new(0);
    let ctx = Ctx {
        root: root_bytes,
        exclude: &opts.exclude,
        detect: opts.detect,
        live_fds: &live_fds,
        fd_cap,
    };

    // The root is read on the calling thread, so every queued job carries a
    // non-empty relative path and the workers need no special case for the
    // root. It costs one directory read.
    let mut walker = Walker::new(&ctx);
    let mut queue = Vec::new();
    walker.read_dir(read_fd, b"", &mut queue);

    // W3.1 drains the queue on the calling thread, newest directory first,
    // which is the order the per-thread deques pop in as well. W3.2 replaces
    // this loop with the work-stealing pool and `opts.threads` starts being
    // read; the entry semantics above are what both share, and neither can
    // change the repository set the other would have produced.
    let base_fd = base_fd.as_fd();
    while let Some(job) = queue.pop() {
        walker.run(job, base_fd, &mut queue);
    }

    Ok(RootListing::from_out(walker.into_out()))
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
