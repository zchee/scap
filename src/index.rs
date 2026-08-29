//! The opt-in mtime-validated repository index (ADR-10).
//!
//! `scap list` normally answers by reading every directory under every root.
//! With the index enabled it answers most runs by `stat`ing them instead:
//! one file per root remembers what the last walk found, and a validation
//! pass decides per entry whether that memory is still true. On the
//! maintainer's own corpus that trades 2,036 `openat`+`getdirentries` pairs
//! for 2,877 `fstatat` calls, which the W0.5 spike measured at 4.07 ms wall
//! against the walker's 12.30 ms.
//!
//! # What the index remembers, and why that is enough
//!
//! A *directory* entry carries the mtime it had when it was read. A unix
//! directory's mtime moves whenever a name is created, removed or renamed
//! inside it, so an unchanged mtime means an unchanged entry list — and the
//! entry list is the only thing the walker's rules (i)–(iv) look at.
//! Unchanged entry list therefore means unchanged `.git` presence and an
//! unchanged child set, which is exactly what lets the validation pass
//! reuse the recorded children instead of reading the directory.
//!
//! A *repository* entry carries no timestamp. A repository's own mtime moves
//! every time anything is written in its working tree, so validating one by
//! mtime would mark most of a working corpus stale on every run; and the
//! only question the index has to ask about a repository is whether its
//! `.git` is still there, which one `statat` answers exactly. A repository
//! whose directory name ends in `.git` — ADR-9 rule (ii) — is not probed at
//! all: nothing but a rename can change that verdict, and a rename moves the
//! parent's mtime.
//!
//! # What it cannot see
//!
//! Everything §4 S1 of the plan lists, and nothing more: a filesystem whose
//! mtime granularity is coarser than the interval between two changes, NFS
//! attribute caching, and a deliberate `touch -t` rewind that restores a
//! directory's recorded mtime after changing its contents. The 2-second racy
//! window (git's own `core.trustctime` rule, [`RACY_WINDOW_NS`]) covers the
//! granularity case for changes made around the index write; the rest is why
//! the feature is opt-in, why `--no-cache` exists and why `--cache-check`
//! exists.
//!
//! Two more residues are worth naming because they are not on that list. A
//! symlink whose *target* gains or loses a `.git` is invisible to the
//! parent's mtime, so a symlinked repository can go stale in place; creating
//! or removing the link itself is caught, because that is the parent's entry
//! list changing. And a permission change is invisible for the same reason,
//! only more sharply: `chmod` moves a directory's *ctime*, never its mtime,
//! so a recorded directory that loses its read bit still validates, and the
//! index keeps reporting the repositories underneath it that a walk can no
//! longer enumerate.
//!
//! That last one wants stating precisely, because the obvious version of it
//! is not true. Taking away *all* permissions is caught: without the execute
//! bit the validation `statat` cannot traverse the directory either, so its
//! children probe stale and the subtree is re-walked. The mode that actually
//! diverges is 0111 — searchable but not readable — where `statat` walks
//! through happily while `readdir` is refused, so the index sees a subtree
//! the walk cannot. It does not decay, either: no later event repairs it,
//! which is why it is documented rather than mitigated. Validating on ctime
//! instead would mark most of a working corpus stale on every run, for the
//! same reason repository entries carry no timestamp at all. The other half
//! of the problem *is* fixed: a walk that could not read a directory never
//! persists its listing as an index ([`Cache::store_if_complete`]), so
//! reaching this blind spot needs an index built while the directory was
//! still readable and a `chmod` afterwards.
//!
//! The index is never allowed to fail a listing. A missing file, a truncated
//! one, a file from another version, an unreadable cache directory and a
//! full disk all resolve to "walk the tree", and the only trace is a debug
//! line.

use std::ffi::{CStr, OsStr, OsString};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bstr::ByteSlice;
// `rustix::fs::Mode` is deliberately not imported: this module's own `Mode`
// is the one every caller means.
use rustix::fs::{AtFlags, CWD, FileType, OFlags};

use crate::walk::{self, Record, RootListing, WalkError, WalkOptions};

/// Magic prefix; changing the format's *shape* changes this, changing its
/// meaning changes [`VERSION`].
const MAGIC: [u8; 8] = *b"SCAPIDX\0";

/// Format version. A file carrying any other value is discarded unread, so
/// bumping this is the whole migration story: the next run rebuilds.
const VERSION: u32 = 1;

/// How close to the index's own write time an observed mtime may sit before
/// the entry is treated as changed regardless of what it says.
///
/// git's "racy" rule, for the same reason git has it: a filesystem that
/// stores whole seconds cannot distinguish a change made just before the
/// index was written from one made just after, so an entry that recent is
/// not evidence of anything. Two seconds rather than one covers filesystems
/// whose granularity is a second *and* whose clock rounds rather than
/// truncates.
const RACY_WINDOW_NS: i64 = 2_000_000_000;

/// Smallest number of bytes one encoded entry can occupy: a kind byte and an
/// empty path's length prefix.
///
/// Deliberately the true floor rather than a typical size. A real entry runs
/// to something like 56 bytes, and using that here would reject a legitimate
/// file whose entries are all short before it was ever parsed. The constant's
/// only job is to bound the `Vec::with_capacity` that follows the entry
/// count, so that a corrupt four-byte length cannot ask for gigabytes; the
/// parse that follows rejects everything this lets through.
const MIN_ENTRY_BYTES: usize = 1 + 4;

/// Whether, and how, `list` consults the repository index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Walk every root; never read or write an index.
    Off,
    /// Answer from the index where it validates, walk what it does not, and
    /// write the index back.
    On,
    /// Answer from the index *and* walk every root, compare the two, and
    /// report any difference. The printed listing is always the fresh
    /// walk's.
    Check,
}

/// Resolves the effective [`Mode`] from the `list` flags and the
/// configuration.
///
/// `--no-cache` beats `--cache`, which beats `scap.listCache` /
/// `SCAP_LIST_CACHE`: the flag nearest the command wins, and the safe answer
/// wins a tie. That precedence is over the three ways of *enabling* the
/// index, which is what "`--no-cache` always wins" means.
///
/// `--cache-check` is not one of those. It is not a stronger way of turning
/// the index on, it is a different operation — walk, compare, report — and
/// combining it with `--no-cache` asks for a comparison against an index the
/// same command line forbids reading. There is no honest answer to that, and
/// silently picking either behaviour would give a user who typed both
/// something neither flag's help text describes, so `list` rejects the pair
/// as a usage error (`conflicts_with` on the argument) and this function
/// never sees it. The branch below is the defensive answer, and prefers the
/// check for the reason the flag exists: it is the operation that tells you
/// the truth about the index rather than the one that trusts it.
pub(crate) fn mode(cache: bool, no_cache: bool, cache_check: bool, configured: bool) -> Mode {
    if cache_check {
        return Mode::Check;
    }
    if no_cache {
        return Mode::Off;
    }
    if cache || configured {
        return Mode::On;
    }
    Mode::Off
}

/// One remembered directory or repository.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    /// Root-relative path; empty for the root itself.
    rel: Vec<u8>,
    /// The directory mtime to validate against, or `None` for a repository.
    mtime_ns: Option<i64>,
    /// Indices of this entry's children, always greater than its own index
    /// so the tree cannot contain a cycle.
    children: Vec<u32>,
}

impl Entry {
    fn is_repo(&self) -> bool {
        self.mtime_ns.is_none()
    }
}

/// One root's index, as it sits in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Index {
    /// The root path this index describes, as `list` spelled it.
    root: Vec<u8>,
    /// The root's device and inode when it was written. A restore that
    /// recreates the root — a Time Machine restore, a fresh checkout over
    /// the same path — changes these, and that invalidates the whole file
    /// rather than any individual entry.
    dev: u64,
    ino: u64,
    /// When the file was written, in nanoseconds since the Unix epoch; the
    /// reference point for [`RACY_WINDOW_NS`].
    written_ns: i64,
    /// The `scap.listExclude` patterns in force when it was written.
    ///
    /// An excluded subtree is not in `entries` at all, so a pattern that is
    /// *removed* would leave a subtree the index cannot describe and whose
    /// parent's mtime never moved. Filtering excluded entries out at load
    /// time only handles the other direction; making the pattern set part of
    /// the index's identity handles both.
    exclude: Vec<Vec<u8>>,
    /// Entry 0 is always the root.
    entries: Vec<Entry>,
}

/// What one entry's `stat` found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Probe {
    /// Still a directory, with this mtime.
    Dir(i64),
    /// Still a repository.
    Repo,
    /// Gone, replaced, or no longer a repository — every case that sends the
    /// path back to the walker.
    Stale,
}

/// The directory index files live in, resolved once per process.
///
/// A cache with nowhere to live is still a `Cache`: every method then walks
/// and reports honestly, which keeps the "no `HOME`" case out of `list`'s
/// control flow. Taking the directory as state rather than reading the
/// environment inside each call is also what makes the whole read-validate-
/// write cycle testable against a temporary directory, without a test ever
/// touching the real `~/.cache` or mutating the process environment.
#[derive(Clone, Debug)]
pub(crate) struct Cache {
    dir: Option<PathBuf>,
}

impl Cache {
    /// The process's cache directory: `$XDG_CACHE_HOME/scap`, else
    /// `$HOME/.cache/scap`, else nowhere.
    pub(crate) fn from_env() -> Self {
        Self {
            dir: cache_dir_from(
                std::env::var_os("XDG_CACHE_HOME").as_deref(),
                std::env::home_dir().as_deref(),
            ),
        }
    }

    /// A cache living in `dir`.
    #[cfg(test)]
    pub(crate) fn in_dir(dir: PathBuf) -> Self {
        Self { dir: Some(dir) }
    }

    /// This root's index file, or `None` when there is nowhere to keep one.
    ///
    /// The name carries FNV-1a of the root path so two roots cannot collide
    /// on one file; the file itself repeats the root path, so a hash
    /// collision is caught rather than believed.
    fn path_for(&self, root: &Path) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        Some(dir.join(format!("index-{:016x}.bin", fnv1a64(root.as_os_str().as_bytes()))))
    }

    /// Lists one root through the index, rewriting the index afterwards.
    ///
    /// Falls back to a full walk whenever the index cannot be used, so the
    /// returned listing is always the one `list` would have printed anyway.
    pub(crate) fn list_root(
        &self,
        root: &Path,
        opts: &WalkOptions,
    ) -> Result<RootListing, WalkError> {
        let span = new_span(root);
        let _entered = span.enter();
        let opts = opts.clone().with_record(true);

        let listing = match self.cached_listing(root, &opts, &span)? {
            Some(listing) => listing,
            None => walk::walk_root(root, &opts)?,
        };
        self.store_if_complete(root, &opts, &listing);
        span.record("entries", listing.records().len());
        Ok(listing)
    }

    /// Walks `root` fresh, reproduces the same listing through the index,
    /// and reports whether the two agree.
    ///
    /// Returns the *fresh* listing in both cases: a check that printed the
    /// index's answer could not be trusted to reveal that the index was
    /// wrong. The index is rewritten from the fresh walk, so a run that
    /// found a disagreement leaves a correct file behind.
    pub(crate) fn check_root(
        &self,
        root: &Path,
        opts: &WalkOptions,
    ) -> Result<(RootListing, bool), WalkError> {
        let span = new_span(root);
        let _entered = span.enter();
        let opts = opts.clone().with_record(true);

        let cached = self.cached_listing(root, &opts, &span)?;
        let fresh = walk::walk_root(root, &opts)?;
        self.store_if_complete(root, &opts, &fresh);
        span.record("entries", fresh.records().len());

        let Some(cached) = cached else {
            tracing::debug!("{}: no usable index to check", root.display());
            return Ok((fresh, false));
        };
        let differs = report_diff(root, &cached, &fresh);
        Ok((fresh, differs))
    }

    /// Produces one root's listing from the index, or `None` when there is
    /// no index this run may believe.
    ///
    /// `None` means "walk the whole root": no cache file, a file that does
    /// not decode, a file describing a different root, a different exclusion
    /// set, or a root whose own directory changed.
    fn cached_listing(
        &self,
        root: &Path,
        opts: &WalkOptions,
        span: &tracing::Span,
    ) -> Result<Option<RootListing>, WalkError> {
        let Some(path) = self.path_for(root) else {
            return Ok(None);
        };
        let Some(index) = load(&path) else {
            span.record("invalidated", 1);
            return Ok(None);
        };

        let Some(root_fd) = open_root(root) else {
            // The walker reports an unreadable root itself, through rule
            // (vi).
            return Ok(None);
        };
        let Ok(stat) = rustix::fs::fstat(root_fd.as_fd()) else {
            return Ok(None);
        };
        if !describes(&index, root, &stat, &exclude_patterns(opts)) {
            tracing::debug!("{}: index describes a different root; rebuilding", root.display());
            span.record("invalidated", 1);
            return Ok(None);
        }

        let root_name = root.file_name().map(OsStr::as_bytes).unwrap_or_default();
        let probes = probe_all(root_fd.as_fd(), &index, root_name, opts.threads);
        let outcome = walk_index(&index, &probes);
        span.record("hit", outcome.hits);
        span.record("miss", outcome.stale.len());
        span.record("racy", outcome.racy);

        // A stale root entry is a stale everything: there is no parent left
        // to re-walk from, and `walk_subtrees` has no descriptor to hang an
        // empty relative path off.
        if outcome.stale.iter().any(Vec::is_empty) {
            span.record("invalidated", 1);
            return Ok(None);
        }

        let mut records = outcome.fresh;
        let rewalked = walk::walk_subtrees(root, &outcome.stale, opts)?;
        let (dirs_read, excluded) = (rewalked.dirs_read(), rewalked.excluded());
        // A re-walk that dropped a subtree makes the whole answer short, even
        // though most of it came from entries that validated: the index this
        // run would write back is missing the same subtree.
        let incomplete = rewalked.incomplete();
        records.extend(rewalked.into_records());
        Ok(Some(RootListing::from_records(records, dirs_read, excluded, incomplete)))
    }

    /// Writes the index this listing describes, unless the listing is short.
    ///
    /// A walk that dropped a subtree — an unreadable directory, an exhausted
    /// descriptor table, an I/O error — produces a listing the command still
    /// prints, with a warning, and that is right: a repository lister that
    /// refuses to list anything because one directory is unreadable is worse
    /// than one that lists what it can and says so. Persisting that listing as
    /// an index would be a different thing entirely. The surviving entries
    /// would all validate on the next run, nothing would say the missing
    /// subtree was ever there, and the short listing would reprint itself
    /// silently — the warning gone, because no directory was read to warn
    /// about — until something happened to move a directory's mtime. So the
    /// short walk is printed and forgotten, and the index on disk stays
    /// whatever it was.
    fn store_if_complete(&self, root: &Path, opts: &WalkOptions, listing: &RootListing) {
        if listing.incomplete() {
            tracing::debug!("{}: a subtree was dropped; index left as it was", root.display());
            return;
        }
        self.store(root, opts, listing.records());
    }

    /// Builds an index from a walk's records and writes it, replacing any
    /// previous one atomically.
    ///
    /// Every failure is a debug line and nothing more. The index is a cache:
    /// a full disk, a read-only cache directory or a lost race with a
    /// concurrent `scap list` all mean "no faster next time", never "no
    /// listing".
    fn store(&self, root: &Path, opts: &WalkOptions, records: &[Record]) {
        let Some(path) = self.path_for(root) else {
            return;
        };
        let Some(index) = build(root, opts, records) else {
            return;
        };
        if let Err(err) = write_atomically(&path, &encode(&index)) {
            tracing::debug!("{}: {err}; index not written", path.display());
        }
    }
}

/// Whether `index` was written for exactly this root, under exactly these
/// exclusions.
///
/// The path guards against an FNV collision picking the wrong file; the
/// device and inode guard against the path being a different directory than
/// it was — a Time Machine restore, a remount, a root deleted and recreated
/// — none of which any individual entry's mtime would reveal; the exclusion
/// set guards against a pattern being *removed*, which leaves the index with
/// no entries for a subtree whose parent's mtime never moved.
fn describes(index: &Index, root: &Path, stat: &rustix::fs::Stat, exclude: &[Vec<u8>]) -> bool {
    index.root == root.as_os_str().as_bytes()
        && index.dev == ident(stat.st_dev)
        && index.ino == ident(stat.st_ino)
        && index.exclude == exclude
}

/// The `scap::index` span one root's index work is recorded on.
///
/// Every field a `Span::record` will write has to be declared at creation or
/// the write silently no-ops — the same instrumentation rule ADR-9's spans
/// follow.
fn new_span(root: &Path) -> tracing::Span {
    tracing::debug_span!(
        "scap::index",
        path = %root.display(),
        hit = tracing::field::Empty,
        miss = tracing::field::Empty,
        racy = tracing::field::Empty,
        invalidated = tracing::field::Empty,
        entries = tracing::field::Empty,
    )
}

/// What the top-down validation pass concluded.
struct Outcome {
    /// Entries that validated, with the mtimes just observed rather than the
    /// recorded ones — which is what makes the rewritten index describe the
    /// tree as it is now.
    fresh: Vec<Record>,
    /// Root-relative paths handed back to the walker.
    stale: Vec<Vec<u8>>,
    hits: usize,
    racy: usize,
}

/// Walks the recorded tree top-down, deciding per entry between "reuse" and
/// "re-walk".
///
/// Top-down is not an optimisation, it is the correctness argument: a child
/// entry may only be believed because its parent's mtime says the child set
/// is unchanged. An entry under a parent that did not validate is never
/// consulted at all — the walker re-derives that whole subtree.
fn walk_index(index: &Index, probes: &[Probe]) -> Outcome {
    let mut out = Outcome {
        fresh: Vec::with_capacity(index.entries.len()),
        stale: Vec::new(),
        hits: 0,
        racy: 0,
    };
    let mut stack = vec![0u32];

    while let Some(i) = stack.pop() {
        let entry = &index.entries[i as usize];
        match (probes[i as usize], entry.mtime_ns) {
            (Probe::Repo, _) => {
                out.hits += 1;
                out.fresh.push(Record::repo(&entry.rel));
            }
            (Probe::Dir(observed), Some(recorded)) if observed == recorded => {
                if is_racy(observed, index.written_ns) {
                    out.racy += 1;
                    out.stale.push(entry.rel.clone());
                    continue;
                }
                out.hits += 1;
                out.fresh.push(Record::dir(&entry.rel, observed));
                stack.extend_from_slice(&entry.children);
            }
            _ => out.stale.push(entry.rel.clone()),
        }
    }
    out
}

/// Whether an observed mtime is too close to the index's write time to be
/// evidence.
///
/// A timestamp *after* the write is racy too, and by the same argument: the
/// index cannot have described a state that had not happened yet.
fn is_racy(observed_ns: i64, written_ns: i64) -> bool {
    observed_ns > written_ns.saturating_sub(RACY_WINDOW_NS)
}

/// `stat`s every recorded entry, in parallel over the walk's own thread
/// count.
///
/// Every entry is probed, including ones the top-down pass will discard
/// because an ancestor did not validate. That is deliberate: the pass is one
/// `statat` per entry with no dependencies between them, which is the shape
/// that parallelises, and the W0.5 spike measured the whole sweep — 2,877
/// calls on corpus a′ — at less than a third of the walk it replaces. Making
/// it dependency-ordered would serialise it to buy back work only a tree
/// that changed everywhere would waste.
fn probe_all(
    root_fd: BorrowedFd<'_>,
    index: &Index,
    root_name: &[u8],
    threads: usize,
) -> Vec<Probe> {
    let entries = &index.entries;
    let mut probes = vec![Probe::Stale; entries.len()];
    let chunk = entries.len().div_ceil(threads.max(1)).max(1);

    std::thread::scope(|scope| {
        for (ents, slots) in entries.chunks(chunk).zip(probes.chunks_mut(chunk)) {
            scope.spawn(move || {
                let mut scratch = Vec::new();
                for (entry, slot) in ents.iter().zip(slots) {
                    *slot = probe(root_fd, entry, root_name, &mut scratch);
                }
            });
        }
    });
    probes
}

/// One entry's validation `stat`.
fn probe(root_fd: BorrowedFd<'_>, entry: &Entry, root_name: &[u8], scratch: &mut Vec<u8>) -> Probe {
    if entry.is_repo() {
        // Rule (ii): a name ending in `.git` is a repository by its name
        // alone, and only a rename can change that — which its parent's
        // mtime records.
        let name = if entry.rel.is_empty() { root_name } else { basename(&entry.rel) };
        if name.ends_with(b".git") {
            return Probe::Repo;
        }
        // Rule (i)/(iv) in one call, following symlinks exactly as
        // `is_repo_dir` does, so a dangling `.git` link is not a repository
        // here either.
        let path = if entry.rel.is_empty() {
            cstr(scratch, &[b".git"])
        } else {
            cstr(scratch, &[&entry.rel, b"/.git"])
        };
        let Some(path) = path else {
            return Probe::Stale;
        };
        return match rustix::fs::statat(root_fd, path, AtFlags::empty()) {
            Ok(_) => Probe::Repo,
            Err(_) => Probe::Stale,
        };
    }

    // `NOFOLLOW`, so a directory replaced by a symlink to a directory reads
    // as stale rather than as itself: the walker would not have descended
    // the symlink.
    let path =
        if entry.rel.is_empty() { cstr(scratch, &[b"."]) } else { cstr(scratch, &[&entry.rel]) };
    let Some(path) = path else {
        return Probe::Stale;
    };
    match rustix::fs::statat(root_fd, path, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
            Probe::Dir(walk::mtime_ns(&stat))
        }
        _ => Probe::Stale,
    }
}

/// Prints every repository the two listings disagree about, and says whether
/// there was one.
fn report_diff(root: &Path, cached: &RootListing, fresh: &RootListing) -> bool {
    let sorted = |listing: &RootListing| {
        let mut paths: Vec<Vec<u8>> = listing.repos().map(<[u8]>::to_vec).collect();
        paths.sort_unstable();
        paths
    };
    let (cached, fresh) = (sorted(cached), sorted(fresh));
    if cached == fresh {
        return false;
    }

    eprintln!("scap: the repository index disagrees with a fresh walk of {}", root.display());
    for path in cached.iter().filter(|p| !fresh.contains(p)) {
        eprintln!("scap:   -{} (in the index, not on disk)", path.as_bstr());
    }
    for path in fresh.iter().filter(|p| !cached.contains(p)) {
        eprintln!("scap:   +{} (on disk, not in the index)", path.as_bstr());
    }
    true
}

/// Opens the root as the base descriptor every validation `statat` hangs
/// off.
///
/// Symlinks are followed, as they are for the walk's own root descriptor: a
/// root reached through a symlink is one the user named on purpose.
fn open_root(root: &Path) -> Option<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    rustix::fs::openat(CWD, root, flags, rustix::fs::Mode::empty()).ok()
}

/// The exclusion patterns in force, in configured order.
fn exclude_patterns(opts: &WalkOptions) -> Vec<Vec<u8>> {
    opts.exclude.iter().map(|pattern| pattern.as_bytes().to_vec()).collect()
}

/// Reads and decodes one index file, or `None` for every reason a run should
/// walk instead.
fn load(path: &Path) -> Option<Index> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::debug!("{}: {err}; walking instead", path.display());
            return None;
        }
    };
    match decode(&bytes) {
        Some(index) => Some(index),
        None => {
            tracing::debug!("{}: unreadable index; rebuilding", path.display());
            None
        }
    }
}

/// Assembles the entry tree one walk's records describe.
///
/// Returns `None` when the records cannot form a tree rooted at the root
/// entry — an unwalkable root produces no records at all, and a record whose
/// parent directory was never recorded would describe a subtree the
/// validation pass could not reach.
fn build(root: &Path, opts: &WalkOptions, records: &[Record]) -> Option<Index> {
    let stat = rustix::fs::statat(CWD, root, AtFlags::empty()).ok()?;

    let mut sorted: Vec<&Record> = records.iter().collect();
    sorted.sort_unstable_by(|a, b| a.rel().cmp(b.rel()));
    sorted.dedup_by(|a, b| a.rel() == b.rel());
    if sorted.first().is_none_or(|first| !first.rel().is_empty()) {
        return None;
    }

    let mut entries: Vec<Entry> = sorted
        .iter()
        .map(|record| Entry {
            rel: record.rel().to_vec(),
            mtime_ns: record.mtime_ns(),
            children: Vec::new(),
        })
        .collect();

    // Byte-sorted paths put every parent before its children — a parent is a
    // proper prefix of each child and shorter — so linking children to
    // parents by binary search cannot produce a forward edge, which is the
    // property `decode` re-checks on the way back in.
    for i in 1..entries.len() {
        let parent = parent_of(&entries[i].rel);
        let at = sorted.binary_search_by(|record| record.rel().cmp(parent)).ok()?;
        let child = u32::try_from(i).ok()?;
        entries[at].children.push(child);
    }

    Some(Index {
        root: root.as_os_str().as_bytes().to_vec(),
        dev: ident(stat.st_dev),
        ino: ident(stat.st_ino),
        written_ns: now_ns(),
        exclude: exclude_patterns(opts),
        entries,
    })
}

/// The root-relative path of `rel`'s parent directory.
fn parent_of(rel: &[u8]) -> &[u8] {
    match memchr::memrchr(b'/', rel) {
        Some(i) => &rel[..i],
        None => b"",
    }
}

/// The last component of a root-relative path.
fn basename(rel: &[u8]) -> &[u8] {
    match memchr::memrchr(b'/', rel) {
        Some(i) => &rel[i + 1..],
        None => rel,
    }
}

/// Serialises one index.
///
/// Little-endian throughout and length-prefixed for every byte string, so a
/// path is stored as the bytes the filesystem gave rather than as text that
/// would have to be valid UTF-8. Repository entries carry neither a
/// timestamp nor a child list, which is most of why the file stays small:
/// on a 25,000-directory tree they are the majority of entries.
fn encode(index: &Index) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + index.entries.len() * 48);
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    push_bytes(&mut buf, &index.root);
    buf.extend_from_slice(&index.dev.to_le_bytes());
    buf.extend_from_slice(&index.ino.to_le_bytes());
    buf.extend_from_slice(&index.written_ns.to_le_bytes());

    push_len(&mut buf, index.exclude.len());
    for pattern in &index.exclude {
        push_bytes(&mut buf, pattern);
    }

    push_len(&mut buf, index.entries.len());
    for entry in &index.entries {
        buf.push(u8::from(entry.mtime_ns.is_some()));
        push_bytes(&mut buf, &entry.rel);
        if let Some(mtime) = entry.mtime_ns {
            buf.extend_from_slice(&mtime.to_le_bytes());
            push_len(&mut buf, entry.children.len());
            for child in &entry.children {
                buf.extend_from_slice(&child.to_le_bytes());
            }
        }
    }
    buf
}

/// Parses one index file, or `None` for anything that is not exactly one.
///
/// Structural validation is part of decoding rather than a later check,
/// because the validation pass walks the child lists as a tree and a
/// hand-edited or truncated file must not be able to make it loop or read
/// out of bounds. Three invariants do that: entry 0 is the root, every child
/// index is greater than its parent's (so the edges cannot cycle), and every
/// entry but the root is exactly one entry's child (so the graph is a tree
/// and no subtree is visited twice).
fn decode(bytes: &[u8]) -> Option<Index> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.take(MAGIC.len())? != MAGIC || reader.u32()? != VERSION {
        return None;
    }
    let root = reader.bytes()?.to_vec();
    let dev = reader.u64()?;
    let ino = reader.u64()?;
    let written_ns = reader.i64()?;

    // The count the file declares, not the vector's capacity: `with_capacity`
    // is free to allocate more than it was asked for, and looping over what it
    // actually reserved would read entries the file never claimed to hold.
    let exclude_count = reader.count(4)?;
    let mut exclude = Vec::with_capacity(exclude_count);
    for _ in 0..exclude_count {
        exclude.push(reader.bytes()?.to_vec());
    }

    let count = reader.count(MIN_ENTRY_BYTES)?;
    let total = u32::try_from(count).ok()?;
    let mut entries = Vec::with_capacity(count);
    let mut parents = vec![false; count];
    for i in 0..count {
        let kind = reader.u8()?;
        let rel = reader.bytes()?.to_vec();
        if !is_safe_rel(&rel) {
            return None;
        }
        let (mtime_ns, children) = match kind {
            0 => (None, Vec::new()),
            1 => {
                let mtime = reader.i64()?;
                let n = reader.count(4)?;
                let mut children = Vec::with_capacity(n);
                for _ in 0..n {
                    let child = reader.u32()?;
                    // Forward edges only, in range, and claimed once: a tree.
                    if child as usize <= i || child >= total || parents[child as usize] {
                        return None;
                    }
                    parents[child as usize] = true;
                    children.push(child);
                }
                (Some(mtime), children)
            }
            _ => return None,
        };
        entries.push(Entry { rel, mtime_ns, children });
    }
    if reader.at != bytes.len() {
        return None;
    }
    // Entry 0 is the root; every other entry is reachable from it.
    if entries.first().is_none_or(|first| !first.rel.is_empty())
        || parents.iter().skip(1).any(|claimed| !claimed)
    {
        return None;
    }

    Some(Index { root, dev, ino, written_ns, exclude, entries })
}

/// Whether a decoded path is one the validation pass may hand to `statat`
/// against the root descriptor.
///
/// Every entry's `rel` is resolved relative to an open descriptor for the
/// root, which confines it to the root's subtree — but only for as long as it
/// really is relative and really stays inside. An absolute path makes
/// `statat` ignore the descriptor outright, and a `..` component walks back
/// out of it. Neither can come from the walk, which builds every `rel` by
/// joining `readdir` names, so a file containing one is not a scap index: it
/// is a file someone wrote. The cache lives in a directory the user can
/// write, so "someone" includes anything else running as this user, and the
/// consequence of believing it would be `scap list --cache -p` printing — and
/// a script acting on — a path outside every configured root.
///
/// Checked at decode rather than at use, so a rejected file is discarded
/// whole and rebuilt by the walk, which is what every other malformed index
/// already does. An interior NUL is refused for the same reason it is refused
/// on the way in: it cannot survive the round trip to a C string intact, and
/// a path that silently truncates at one is a different path.
fn is_safe_rel(rel: &[u8]) -> bool {
    if rel.is_empty() {
        // The root's own entry, and the only empty path the format allows.
        return true;
    }
    if rel[0] == b'/' || rel.contains(&0) {
        return false;
    }
    rel.split(|byte| *byte == b'/').all(|part| !matches!(part, b"" | b"." | b".."))
}

/// A bounds-checked cursor over the encoded form.
///
/// Every accessor returns `None` past the end rather than panicking, which
/// is what turns "the file is truncated" into "rebuild the index" instead of
/// into a crash in a command whose job is to print a list.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    /// A length prefix, rejected when the remaining bytes could not hold
    /// that many items of `each` bytes.
    ///
    /// The point is the allocation that follows: without this a four-byte
    /// count of `0xffffffff` would ask for gigabytes before the read that
    /// fails.
    fn count(&mut self, each: usize) -> Option<usize> {
        let count = self.u32()? as usize;
        (count.checked_mul(each)? <= self.bytes.len() - self.at).then_some(count)
    }
}

fn push_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    push_len(buf, bytes.len());
    buf.extend_from_slice(bytes);
}

/// Writes a `u32` length prefix, saturating rather than panicking.
///
/// A saturated length can only be produced by a path or a collection four
/// billion entries long, and it makes the file fail its own decode on the
/// next run — the same outcome as any other corruption, and one the listing
/// survives.
fn push_len(buf: &mut Vec<u8>, len: usize) {
    buf.extend_from_slice(&u32::try_from(len).unwrap_or(u32::MAX).to_le_bytes());
}

/// Replaces `path` with `bytes`, atomically for any concurrent reader.
///
/// A temporary file in the same directory plus a rename, so a reader sees
/// either the whole previous index or the whole new one. Two `scap list`
/// runs racing to write leave one of the two files intact rather than a
/// blend of both.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "cache path has no parent")
    })?;
    std::fs::create_dir_all(dir)?;

    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dir.join(name);
    // Best effort on the way out: a rename that failed leaves the temporary
    // file behind, and leaking one per failure would be worse than the
    // failure.
    if let Err(err) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

/// `$XDG_CACHE_HOME/scap`, or `$HOME/.cache/scap`.
///
/// The XDG base directory specification requires `XDG_CACHE_HOME` to hold an
/// absolute path and says a relative one "should be considered invalid",
/// so a relative value falls back to `$HOME` rather than creating a cache
/// directory wherever the command happened to be run. With no usable home
/// either there is no cache at all, and every run walks.
fn cache_dir_from(xdg: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(xdg) = xdg.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(OsString::from_vec(xdg.as_bytes().to_vec()));
        if path.is_absolute() {
            return Some(path.join("scap"));
        }
    }
    Some(home?.join(".cache").join("scap"))
}

/// FNV-1a, 64-bit, over the root path's bytes.
///
/// Hand-rolled rather than taken from a crate: it is six lines against a new
/// dependency in a graph the plan keeps countable, and nothing here needs
/// collision resistance — the hash only picks a file name, and the file
/// itself carries the root path that a collision would have to match.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// A `stat` identity field widened to the type the file format stores.
///
/// `dev_t` and `ino_t` differ in width and signedness across the unixes
/// scap builds for, and this is only ever compared against itself, so an
/// out-of-range value collapsing to `u64::MAX` costs nothing: it makes the
/// index look like a different root and the run walks.
fn ident<T: TryInto<u64>>(value: T) -> u64 {
    value.try_into().unwrap_or(u64::MAX)
}

/// The current time in nanoseconds since the Unix epoch.
fn now_ns() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_nanos()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_nanos()).map_or(i64::MIN, |ns| -ns),
    }
}

/// Builds `<parts…>\0` in `scratch` and hands it back as a `CStr`, or `None`
/// when the bytes cannot be one.
///
/// The same trick `src/walk/sys.rs` uses: `rustix`'s path arguments take a
/// `&CStr` without copying, so building the NUL-terminated form in a buffer
/// the thread reuses keeps the per-entry probe free of allocation.
///
/// The failure is `None` and not a fallback path. An interior NUL cannot come
/// from a filesystem and cannot survive [`is_safe_rel`], so reaching here
/// means the bytes are not what they claim to be — and the previous fallback,
/// probing `.` instead, was a probe of the *root* that a repository entry
/// would have answered `Probe::Repo` to. That is the one direction this code
/// must never fail in: an unreadable path has to become `Probe::Stale` and go
/// back to the walker, never a hit on a path nobody asked about.
fn cstr<'a>(scratch: &'a mut Vec<u8>, parts: &[&[u8]]) -> Option<&'a CStr> {
    scratch.clear();
    for part in parts {
        scratch.extend_from_slice(part);
    }
    scratch.push(0);
    CStr::from_bytes_with_nul(scratch).ok()
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
