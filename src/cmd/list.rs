use std::collections::{HashMap, HashSet};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bstr::ByteSlice;
use clap::Args;
use gix_glob::wildmatch;
use jwalk::{Parallelism, ReadChildren, WalkDirGeneric};

use crate::config;

/// Size of the rayon pool each root is walked on.
///
/// Four is the thread count the W0.2 matrix selected for every walker
/// variant on corpus a+b, and it is what this walker already used. Phase 3
/// makes it configurable through `SCAP_LIST_THREADS`; until then it is a
/// constant so the `threads` span field never reports a number the walk did
/// not actually use.
const WALK_THREADS: usize = 4;

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Perform an exact match (against `project` or `owner/project` or `host/owner/project`).
    #[arg(long, short = 'e')]
    pub exact: bool,

    /// VCS backend to match. v1 accepts `git`/`github`/`codecommit` only;
    /// other values (svn, hg, darcs, fossil, bzr) are rejected
    /// (intentional divergence from ghq, see ADR-2).
    #[arg(long, value_name = "vcs")]
    pub vcs: Option<String>,

    /// Print full paths.
    #[arg(long, short = 'p')]
    pub full_path: bool,

    /// Print unique sub-paths only.
    #[arg(long)]
    pub unique: bool,

    /// Query as a bare repository URL (does NOT filter to bare-only;
    /// matches ghq's behavior where --bare only changes URL-query normalization).
    #[arg(long)]
    pub bare: bool,

    /// Optional query string.
    pub query: Option<String>,
}

struct DiscoveredRepo {
    full_path: PathBuf,
    rel_path: String,
    rel_parts: Vec<String>,
    is_under_primary: bool,
}

impl DiscoveredRepo {
    fn rel_path(&self) -> String {
        self.rel_path.clone()
    }

    // ghq local_repository.go:NonHostPath — path without the leading host segment.
    fn non_host_path(&self) -> String {
        if self.rel_parts.len() <= 1 { String::new() } else { self.rel_parts[1..].join("/") }
    }

    // ghq local_repository.go:Subpaths — tails of the relative path, shortest first.
    fn subpaths(&self) -> Vec<String> {
        let n = self.rel_parts.len();
        (0..n).map(|i| self.rel_parts[n - (i + 1)..].join("/")).collect()
    }

    fn matches_exact(&self, query: &str) -> bool {
        self.subpaths().iter().any(|p| p == query)
    }
}

// ghq cmd_list.go:doList.
pub fn run(args: &ListArgs) -> anyhow::Result<()> {
    if let Some(v) = &args.vcs
        && !matches!(v.as_str(), "git" | "github" | "codecommit")
    {
        anyhow::bail!(
            "unsupported VCS: {:?} (v1 supports git only; see issue tracker for non-git)",
            v
        );
    }

    let roots = config::resolve_roots(true)?;
    let primary = roots.first().cloned();

    let mut repos = Vec::new();
    for root in &roots {
        walk_for_repos(root, &mut repos, primary.as_deref())?;
    }

    let repos = filter_repos(repos, args);

    let lines = if args.unique {
        format_unique(&repos)
    } else if args.full_path {
        repos.iter().map(|r| r.full_path.display().to_string()).collect::<Vec<_>>()
    } else {
        repos.iter().map(|r| r.rel_path()).collect::<Vec<_>>()
    };

    let mut sorted = lines;
    sorted.sort_unstable();
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for line in sorted {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn walk_for_repos(
    root: &Path,
    out: &mut Vec<DiscoveredRepo>,
    primary: Option<&Path>,
) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    walk_for_repos_with_seen(root, out, primary, &mut seen)
}

fn walk_for_repos_with_seen(
    root: &Path,
    out: &mut Vec<DiscoveredRepo>,
    primary: Option<&Path>,
    seen: &mut HashSet<PathBuf>,
) -> anyhow::Result<()> {
    if !root_is_walkable(root) {
        return Ok(());
    }

    // ADR-9's instrumentation rule: a field `Span::record` will write later
    // has to be declared at creation, or the write silently no-ops and the
    // close line never carries it.
    let span = tracing::debug_span!(
        "scap::walk::root",
        path = %root.display(),
        dirs_read = tracing::field::Empty,
        excluded = tracing::field::Empty,
        repos = tracing::field::Empty,
        threads = WALK_THREADS,
    );
    let _entered = span.enter();
    let before = out.len();

    if is_repo_path(root) {
        maybe_push_repo(root.to_path_buf(), root, out, primary, seen);
        span.record("dirs_read", 0usize);
        span.record("excluded", 0usize);
        span.record("repos", out.len() - before);
        return Ok(());
    }

    // ADR-9's mechanics: `process_read_dir` is `Fn + Send + Sync + 'static`
    // and runs on the rayon workers, so the counters are shared atomics
    // rather than captured locals.
    let dirs_read = Arc::new(AtomicUsize::new(0));
    let excluded = Arc::new(AtomicUsize::new(0));
    let (walk_dirs_read, walk_excluded) = (Arc::clone(&dirs_read), Arc::clone(&excluded));
    // `&'static ConfigSnapshot`, so the patterns need no copy to outlive the
    // closure.
    let patterns: &'static [String] = config::snapshot().list_exclude();
    let walk_root: Arc<Path> = Arc::from(root);

    let walker = WalkDirGeneric::<((), bool)>::new(root)
        // ADR-9 rule (iii): jwalk must not resolve links on scap's behalf.
        // With `follow_links(true)` a symlinked directory is reported as a
        // directory and descended, so `list` printed repositories ghq never
        // reaches -- W0.4 case 2 and case 14a, where ghq printed nothing and
        // scap printed `link-to-plain-dir/nested/repo`. Off, every symlink
        // is reported as a symlink and left unread, and the one link ghq
        // does emit -- one whose target is itself a repository -- is
        // recognised by the explicit `metadata` call in `process_read_dir`.
        .follow_links(false)
        .parallelism(Parallelism::RayonNewPool(WALK_THREADS))
        .skip_hidden(false)
        .sort(false)
        .process_read_dir(move |depth, dir_path, _, children| {
            // jwalk calls this once before anything is read, with `depth`
            // `None`, the walk root's *parent* as the path and the root
            // itself as the only child. That call is not a directory read
            // and must not be counted as one, and the root is not a
            // candidate for exclusion (a pattern is matched against paths
            // relative to it).
            if depth.is_none() {
                return;
            }
            walk_dirs_read.fetch_add(1, Ordering::Relaxed);

            // The root-relative path of this directory, with the separator
            // already appended, so each child costs a truncate and an
            // extend rather than an allocation.
            let mut rel = Vec::new();
            let mut rel_is_root_relative = false;
            if !patterns.is_empty() {
                let stripped = dir_path.strip_prefix(&walk_root);
                debug_assert!(
                    stripped.is_ok(),
                    "walk path {} is not under the walk root {}",
                    dir_path.display(),
                    walk_root.display()
                );
                // jwalk only ever reports paths under the root it was
                // given. If that ever stopped holding, the exclusion test
                // is skipped for this directory rather than falling back to
                // the absolute path: patterns are anchored at the root, and
                // matching them against `/Users/...` could silently exclude
                // a subtree because of where the corpus happens to live.
                if let Ok(prefix) = stripped {
                    rel.extend_from_slice(prefix.as_os_str().as_encoded_bytes());
                    if !rel.is_empty() {
                        rel.push(b'/');
                    }
                    rel_is_root_relative = true;
                }
            }
            let dir_len = rel.len();

            for child in children.iter_mut() {
                let child = match child {
                    Ok(child) => child,
                    // ADR-9 rule (v): this entry is skipped, and reported
                    // from the walk iterator rather than here -- jwalk
                    // yields every erroring child from there as well, so
                    // reporting in both arms would print each problem
                    // twice.
                    Err(_) => continue,
                };

                // ADR-9 rule (iii): a symlink is a candidate as well as a
                // directory. ghq resolves one for `IsDir` and, when the
                // target is a repository, calls back with the *link's* path
                // (local_repository.go:268-299); it never walks through one
                // (walker.go:85-90). Anything else -- a regular file, a
                // socket -- is neither.
                let file_type = child.file_type();
                let is_symlink = file_type.is_symlink();
                if !file_type.is_dir() && !is_symlink {
                    continue;
                }

                // ADR-9 rule (viii): an excluded directory is neither read
                // nor emitted, and the test runs before the repository
                // probe so an excluded subtree costs no `stat` either.
                if rel_is_root_relative {
                    rel.truncate(dir_len);
                    rel.extend_from_slice(child.file_name.as_encoded_bytes());
                    if is_excluded(patterns, &rel) {
                        child.read_children = None;
                        walk_excluded.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                }

                let child_path = child.path();

                // ADR-9 rules (iii) and (iv): one following `stat` per
                // symlink, and only per symlink. A link that resolves to
                // anything but a directory -- a regular file, a dangling
                // target, a loop -- is not a candidate. ghq prints nothing
                // for any of them (W0.4 cases 3, 4, 5 and 7), so the
                // resolution failure stays at the debug level rather than
                // putting a line on stderr that the oracle does not have.
                if is_symlink {
                    match std::fs::metadata(&child_path) {
                        Ok(meta) if meta.is_dir() => {}
                        Ok(_) => continue,
                        Err(err) => {
                            tracing::debug!("{}: {err}", child_path.display());
                            continue;
                        }
                    }
                }

                if is_repo_path(&child_path) {
                    // Not descended: ghq prunes a repository's own subtree,
                    // and never descends a symlink at all. jwalk reads
                    // neither a `read_children = None` directory nor -- with
                    // `follow_links(false)` -- any symlink.
                    child.read_children = None;
                    child.client_state = true;
                }
            }
        });

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            // ADR-9 rule (v): every child `process_read_dir` could not
            // build reaches this arm too, so reporting here covers both of
            // the walker's error paths exactly once. Exit status stays 0.
            // Only a permission error is warned; see `report_walk_error`.
            Err(err) => {
                report_walk_error(&err);
                continue;
            }
        };

        // A directory whose own `read_dir` failed never reaches
        // `process_read_dir` -- jwalk hangs the error on the entry instead
        // -- so rule (v)'s permission-denied case is reported from here.
        if let Some(err) = entry.read_children.as_ref().and_then(ReadChildren::error) {
            report_walk_error(err);
        }

        // `client_state` is set only where `process_read_dir` identified a
        // repository, so it carries the file-type test already. It is not
        // redundant with one here: under rule (iii) a symlinked repository
        // is emitted, and its own file type is a symlink, not a directory.
        if !entry.client_state {
            continue;
        }

        let repo_root = normalize_repo_root(entry.path());
        maybe_push_repo(repo_root, root, out, primary, seen);
    }

    span.record("dirs_read", dirs_read.load(Ordering::Relaxed));
    span.record("excluded", excluded.load(Ordering::Relaxed));
    span.record("repos", out.len() - before);
    Ok(())
}

/// ADR-9 rule (viii): whether `rel`, a root-relative path, matches any
/// configured exclusion.
///
/// Patterns are matched against the whole root-relative path and are
/// therefore anchored at the root: `foo` excludes `<root>/foo` and not
/// `<root>/bar/foo`, and a pattern that is to reach further down says so
/// (`*/foo`, or `**/foo`). `NO_MATCH_SLASH_LITERAL` is git's own
/// `WM_PATHNAME`, under which `*` and `?` stop at a `/` while `**` crosses
/// it -- the semantics a `.gitignore` reader already expects.
///
/// Matching is case-sensitive, as git's is: `IGNORE_CASE` is deliberately
/// not set, so a pattern must use the on-disk spelling even on a
/// case-insensitive filesystem such as the default APFS layout.
fn is_excluded(patterns: &[String], rel: &[u8]) -> bool {
    patterns.iter().any(|pattern| {
        wildmatch(
            pattern.as_bytes().as_bstr(),
            rel.as_bstr(),
            gix_glob::wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
        )
    })
}

/// ADR-9 rule (vi): whether this root is worth handing to the walker.
///
/// A root that does not exist is skipped silently -- a `scap.root` naming a
/// directory the user has not created yet is not an error, and ghq skips it
/// the same way. A root that exists but cannot be read, and a root whose
/// `stat` fails for any other reason, are skipped *with* a warning: they
/// hide repositories that would otherwise be listed, so silence would make
/// the shorter output look authoritative. (ghq dereferences a nil
/// `FileInfo` in the third case and panics, so it cannot be the oracle for
/// it -- registered in ADR-13.)
fn root_is_walkable(root: &Path) -> bool {
    match std::fs::metadata(root) {
        Ok(meta) => {
            if metadata_is_readable(&meta) {
                return true;
            }
            tracing::warn!("{}: Permission denied", root.display());
            false
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => {
            warn_unwalkable_root(root, &err);
            false
        }
    }
}

/// ghq's `local_repository.go:310-318` readability test: any of the three
/// read bits set.
#[cfg(unix)]
fn metadata_is_readable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    meta.permissions().mode() & 0o444 != 0
}

#[cfg(not(unix))]
fn metadata_is_readable(_meta: &std::fs::Metadata) -> bool {
    true
}

/// ADR-9 rule (v): report an entry the walk could not read, then carry on.
///
/// The walk still exits 0 either way. A `list` that aborted on the first
/// unreadable directory would be less useful than one that lists what it
/// can and says what it skipped, which is what ghq does
/// (`local_repository.go:301-306`).
///
/// Only a permission error reaches stderr by default, and it carries ghq's
/// own wording rather than the OS string so it does not vary by platform.
/// Everything else is a debug line: ghq prints nothing for those, and a
/// single dangling symlink anywhere in a corpus -- the maintainer's has one,
/// a stale documentation link -- would otherwise put a line on every `scap
/// list`, which teaches users to stop reading its stderr and so costs more
/// than it reports. Under rule (iii)'s `follow_links(false)` the walk no
/// longer manufactures that class at all: an unresolvable symlink is
/// reported from `process_read_dir`'s own `metadata` call, at debug.
fn report_walk_io_error(path: &Path, err: &io::Error) {
    // Both EACCES and EPERM map to `PermissionDenied`.
    if err.kind() == io::ErrorKind::PermissionDenied {
        tracing::warn!("{}: Permission denied", path.display());
    } else {
        tracing::debug!("{}: {err}", path.display());
    }
}

fn report_walk_error(err: &jwalk::Error) {
    match (err.path(), err.io_error()) {
        (Some(path), Some(io_err)) => report_walk_io_error(path, io_err),
        // A jwalk error with no `io::Error` behind it -- a depth or
        // recursion complaint of its own. Symlink loops no longer arrive
        // here: with `follow_links(false)` the walk never resolves a link,
        // so a loop surfaces as the ELOOP that rule (iii)'s own `metadata`
        // call returns, and is logged there. ghq is silent for both.
        (Some(path), None) => tracing::debug!("{}: {err}", path.display()),
        (None, _) => tracing::debug!("{err}"),
    }
}

/// ADR-9 rule (vi): report a root the walk cannot use.
///
/// Unlike rule (v)'s per-entry errors this always warns, because a root
/// scap cannot walk hides every repository beneath it rather than one
/// entry, and the listing that results looks complete. ghq warns for the
/// unreadable root as well; the non-ENOENT case is scap's own, since ghq
/// panics there (ADR-13).
fn warn_unwalkable_root(root: &Path, err: &io::Error) {
    if err.kind() == io::ErrorKind::PermissionDenied {
        tracing::warn!("{}: Permission denied", root.display());
    } else {
        tracing::warn!("{}: {err}", root.display());
    }
}

fn maybe_push_repo(
    full_path: PathBuf,
    root: &Path,
    out: &mut Vec<DiscoveredRepo>,
    primary: Option<&Path>,
    seen: &mut HashSet<PathBuf>,
) {
    if !seen.insert(full_path.clone()) {
        return;
    }

    if let Some(rel_parts) = rel_parts(root, &full_path) {
        let is_under_primary = primary.map(|p| full_path.starts_with(p)).unwrap_or(false);
        let rel_path = rel_parts.join("/");
        out.push(DiscoveredRepo { full_path, rel_path, rel_parts, is_under_primary });
    }
}

/// ADR-9 rule (ii): a directory whose *own name* ends in `.git` is a
/// repository without being opened, and rule (iii) applies that test to a
/// symlink's name rather than its target's -- so `link -> upstream.git` is
/// not a repository, while `link.git -> upstream.git` is (W0.4 case 7).
fn is_repo_path(path: &Path) -> bool {
    if path.file_name().and_then(|n| n.to_str()).is_some_and(|name| name.ends_with(".git")) {
        return true;
    }

    path.join(".git").exists()
}

fn normalize_repo_root(path: PathBuf) -> PathBuf {
    if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
        return path.parent().map_or_else(|| path.clone(), |p| p.to_path_buf());
    }

    path
}

fn rel_parts(root: &Path, full: &Path) -> Option<Vec<String>> {
    let rel = full.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_owned()))
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() { Some(vec![".".to_owned()]) } else { Some(parts) }
}

fn filter_repos(repos: Vec<DiscoveredRepo>, args: &ListArgs) -> Vec<DiscoveredRepo> {
    let Some(query) = args.query.as_deref() else {
        return repos;
    };

    if args.exact {
        repos.into_iter().filter(|r| r.matches_exact(query)).collect()
    } else {
        let (host_filter, q) = split_authority_prefix(query);
        let lower = q.to_lowercase();
        let smart_case = q == lower;
        repos
            .into_iter()
            .filter(|r| {
                if let Some(host) = host_filter
                    && r.rel_parts.first().map(String::as_str) != Some(host)
                {
                    return false;
                }

                let hay = r.non_host_path();
                if smart_case { hay.to_lowercase().contains(&lower) } else { hay.contains(&q) }
            })
            .collect()
    }
}

// ghq cmd_list.go: looksLikeAuthorityPattern detection for "host/<rest>" queries.
fn split_authority_prefix(query: &str) -> (Option<&str>, String) {
    if let Some((head, tail)) = query.split_once('/')
        && looks_like_authority(head)
    {
        return (Some(head), tail.to_owned());
    }
    (None, query.to_owned())
}

fn looks_like_authority(s: &str) -> bool {
    s.contains('.') && !s.contains(' ')
}

// ghq cmd_list.go: --unique de-dup logic.
fn format_unique(repos: &[DiscoveredRepo]) -> Vec<String> {
    let mut subpath_count: HashMap<String, usize> = HashMap::new();
    let mut repos_count: HashMap<String, usize> = HashMap::new();

    for r in repos {
        let rel = r.rel_path();
        if *repos_count.get(&rel).unwrap_or(&0) == 0 {
            for p in r.subpaths() {
                *subpath_count.entry(p).or_insert(0) += 1;
            }
        }
        *repos_count.entry(rel).or_insert(0) += 1;
    }

    let mut out = Vec::with_capacity(repos.len());
    for r in repos {
        let rel = r.rel_path();
        if *repos_count.get(&rel).unwrap_or(&0) > 1 && !r.is_under_primary {
            continue;
        }
        for p in r.subpaths() {
            if *subpath_count.get(&p).unwrap_or(&0) == 1 {
                out.push(p);
                break;
            }
        }
    }
    out
}
