use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

use clap::Args;
use memchr::{memchr, memmem, memrchr_iter};

use crate::config;
use crate::walk::{self, Pattern, RootListing, WalkOptions};

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

/// One repository the walk found, as the post-processing passes see it.
///
/// Nothing here owns a path. `rel` points into the arena of the
/// [`RootListing`] it came from and `root` into the resolved root list, so
/// filtering, `--unique` and the byte-order sort all move 24-byte handles
/// rather than strings — a listing of corpus a+b never allocates per
/// repository after the walk.
struct Repo<'a> {
    /// The root this repository was found under, exactly as
    /// `config::resolve_roots` returned it.
    root: &'a Path,
    /// Root-relative path bytes, or `.` for a root that is itself a
    /// repository.
    rel: &'a [u8],
    /// Whether the repository lies under the *primary* root, which is the
    /// tiebreak `--unique` applies to a path more than one root holds
    /// (ghq `cmd_list.go:doList`).
    under_primary: bool,
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
    let primary = roots.first().map(std::path::PathBuf::as_path);

    // One `WalkOptions` for every root: the pattern set and the thread count
    // are process-wide, and the detection strategy is resolved once so a
    // multi-root listing cannot walk two roots two different ways.
    let opts = WalkOptions::new(
        walk::threads_from_env(),
        config::snapshot().list_exclude().iter().map(|p| Pattern::new(p)).collect(),
    );

    // Rule (vii): roots are walked in order and never deduplicated against
    // each other, so a repository two roots both hold is printed twice.
    let mut listings = Vec::with_capacity(roots.len());
    for root in &roots {
        listings.push(walk_one(root, &opts)?);
    }

    let repos: Vec<Repo<'_>> = roots
        .iter()
        .zip(&listings)
        .flat_map(|(root, listing)| {
            let root_under_primary = primary.is_some_and(|p| root.starts_with(p));
            // The whole root answers for every repository under it, except
            // where the primary root lies *below* this one — then the answer
            // is per repository, and only then is a path built to ask.
            let needs_per_repo =
                !root_under_primary && primary.is_some_and(|p| p.starts_with(root));
            listing.repos().map(move |rel| Repo {
                root: root.as_path(),
                rel,
                under_primary: root_under_primary
                    || (needs_per_repo && under_primary(primary, root, rel)),
            })
        })
        .collect();

    // ADR-9's instrumentation rule again: every field a `Span::record` will
    // write has to be declared at creation or the write silently no-ops.
    let span = tracing::debug_span!(
        "scap::list::postprocess",
        filter_us = tracing::field::Empty,
        sort_us = tracing::field::Empty,
        format_us = tracing::field::Empty,
    );
    let _entered = span.enter();

    let started = Instant::now();
    let repos = filter_repos(repos, args);
    let filtered = Instant::now();

    // `--unique` and the default form both print sub-slices of the walk's own
    // arena; only `-p` needs bytes that do not exist yet, and it builds them
    // into one buffer rather than one string per repository. `--unique` wins
    // over `-p` when both are given, as it did before, so the paths are not
    // built at all then -- on corpus a+b that is 1,826 joins and ~94 KB
    // nothing would have read.
    let full_path_only = args.full_path && !args.unique;
    let mut full_paths = Vec::new();
    let mut full_spans = Vec::with_capacity(if full_path_only { repos.len() } else { 0 });
    if full_path_only {
        for repo in &repos {
            let start = full_paths.len();
            push_full_path(&mut full_paths, repo.root, repo.rel);
            full_spans.push(start..full_paths.len());
        }
    }

    let mut lines: Vec<&[u8]> = if args.unique {
        format_unique(&repos)
    } else if full_path_only {
        full_spans.iter().map(|span| &full_paths[span.clone()]).collect()
    } else {
        repos.iter().map(|repo| repo.rel).collect()
    };
    let formatted = Instant::now();

    // Byte order on the rendered line, which is what `Vec<String>::sort` was
    // doing before: `String`'s ordering is its bytes'.
    lines.sort_unstable();
    let sorted = Instant::now();

    let mut buf = Vec::with_capacity(lines.iter().map(|line| line.len() + 1).sum());
    for line in &lines {
        buf.extend_from_slice(line);
        buf.push(b'\n');
    }
    let done = Instant::now();

    span.record("filter_us", micros(filtered - started));
    span.record("sort_us", micros(sorted - formatted));
    span.record("format_us", micros((formatted - filtered) + (done - sorted)));
    drop(_entered);

    write_stdout(&buf)
}

/// One `Instant` delta as the whole microseconds a span field can carry.
///
/// `tracing` has no `u128` value, and the saturation can only be reached by a
/// listing that spent 584,000 years in one pass.
fn micros(elapsed: std::time::Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

/// Walks one root, recording ADR-9's counters on the `scap::walk::root` span.
///
/// The span is opened for every root the command was given, including one the
/// walk turns out not to be able to read: a root that contributed nothing is
/// exactly the case where the counters are worth having, and `dirs_read = 0`
/// says so unambiguously.
fn walk_one(root: &Path, opts: &WalkOptions) -> anyhow::Result<RootListing> {
    let span = tracing::debug_span!(
        "scap::walk::root",
        path = %root.display(),
        dirs_read = tracing::field::Empty,
        excluded = tracing::field::Empty,
        repos = tracing::field::Empty,
        threads = opts.threads,
    );
    let _entered = span.enter();

    let listing = walk::walk_root(root, opts)?;
    span.record("dirs_read", listing.dirs_read());
    span.record("excluded", listing.excluded());
    span.record("repos", listing.len());
    Ok(listing)
}

/// Appends `<root>/<rel>` to `sink`, the way `Path::join` would have rendered
/// it.
///
/// Two cases are not a plain concatenation. A root that is itself a
/// repository carries the relative path `.`, and ghq prints the root for it
/// rather than `<root>/.`; and a root spelled with a trailing separator —
/// only `/` survives `clean_path` and `canonicalize` — must not gain a second
/// one.
fn push_full_path(sink: &mut Vec<u8>, root: &Path, rel: &[u8]) {
    let root = root.as_os_str().as_encoded_bytes();
    sink.extend_from_slice(root);
    if rel == b"." {
        return;
    }
    if !root.ends_with(b"/") {
        sink.push(b'/');
    }
    sink.extend_from_slice(rel);
}

/// Whether the repository at `rel` under `root` also lies under `primary`.
///
/// Reproduces `Path::starts_with`, which compares whole components: under a
/// primary root `/a/b`, the path `/a/bc/x` is not a match even though its
/// bytes begin with the same nine. Both paths come from
/// `config::resolve_roots`, so both are absolute and already cleaned, and a
/// component-boundary test on the bytes is the same predicate.
fn under_primary(primary: Option<&Path>, root: &Path, rel: &[u8]) -> bool {
    let Some(primary) = primary else {
        return false;
    };
    let primary = primary.as_os_str().as_encoded_bytes();
    let mut full = Vec::with_capacity(root.as_os_str().len() + rel.len() + 1);
    push_full_path(&mut full, root, rel);
    full == primary
        || (full.len() > primary.len()
            && full.starts_with(primary)
            && (full[primary.len()] == b'/' || primary.ends_with(b"/")))
}

/// Every tail of `rel` that starts on a component boundary, shortest first.
///
/// ghq `local_repository.go:Subpaths`. The tails are sub-slices rather than
/// joins because `rel` is already the components joined with `/`, so the
/// `i`th tail *is* a suffix of it — which is what lets `--unique` count
/// sub-paths in a `HashMap<&[u8], usize>` with no allocation at all.
fn subpaths(rel: &[u8]) -> impl Iterator<Item = &[u8]> {
    memrchr_iter(b'/', rel).map(move |i| &rel[i + 1..]).chain(std::iter::once(rel))
}

/// ghq `local_repository.go:NonHostPath` — the path without its leading host
/// component, and empty when there is no other component.
fn non_host_path(rel: &[u8]) -> &[u8] {
    match memchr(b'/', rel) {
        Some(i) => &rel[i + 1..],
        None => b"",
    }
}

/// The leading component of `rel`, which is the host segment of the layout.
fn host_component(rel: &[u8]) -> &[u8] {
    match memchr(b'/', rel) {
        Some(i) => &rel[..i],
        None => rel,
    }
}

fn filter_repos<'a>(repos: Vec<Repo<'a>>, args: &ListArgs) -> Vec<Repo<'a>> {
    let Some(query) = args.query.as_deref() else {
        return repos;
    };

    if args.exact {
        let needle = query.as_bytes();
        return repos.into_iter().filter(|r| subpaths(r.rel).any(|p| p == needle)).collect();
    }

    let (host_filter, q) = split_authority_prefix(query);
    let host_filter = host_filter.map(str::as_bytes);
    let lower = q.to_lowercase();
    // ghq's smart case: an all-lowercase query matches case-insensitively,
    // a query carrying any uppercase matches literally.
    let smart_case = q == lower;
    let finder = memmem::Finder::new(if smart_case { lower.as_bytes() } else { q.as_bytes() });
    // One lowercased copy of the haystack at a time, in a buffer reused for
    // every repository.
    let mut folded = Vec::new();

    repos
        .into_iter()
        .filter(|r| {
            if let Some(host) = host_filter
                && host_component(r.rel) != host
            {
                return false;
            }

            let hay = non_host_path(r.rel);
            if !smart_case {
                return finder.find(hay).is_some();
            }
            finder.find(lowercase(&mut folded, hay)).is_some()
        })
        .collect()
}

/// Lowercases `hay` into `sink` and hands the result back.
///
/// The ASCII path is the one every real repository layout takes and it costs
/// one pass with no allocation. Anything else goes through
/// [`str::to_lowercase`], which is what the previous `String`-based filter
/// called and so is the only spelling that keeps the exotic cases — the
/// Kelvin sign folding to `k`, a final sigma, a character whose lowercase is
/// longer than itself — matching byte for byte. A name that is not UTF-8 at
/// all cannot reach `to_lowercase`; it is folded as ASCII, which is more than
/// the previous walker managed (it dropped undecodable components outright).
fn lowercase<'a>(sink: &'a mut Vec<u8>, hay: &[u8]) -> &'a [u8] {
    sink.clear();
    match std::str::from_utf8(hay) {
        Ok(s) if !s.is_ascii() => sink.extend_from_slice(s.to_lowercase().as_bytes()),
        _ => {
            sink.extend_from_slice(hay);
            sink.make_ascii_lowercase();
        }
    }
    sink
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
fn format_unique<'a>(repos: &[Repo<'a>]) -> Vec<&'a [u8]> {
    let mut subpath_count: HashMap<&[u8], usize> = HashMap::new();
    let mut repos_count: HashMap<&[u8], usize> = HashMap::new();

    for r in repos {
        if repos_count.get(r.rel).copied().unwrap_or(0) == 0 {
            for p in subpaths(r.rel) {
                *subpath_count.entry(p).or_insert(0) += 1;
            }
        }
        *repos_count.entry(r.rel).or_insert(0) += 1;
    }

    let mut out = Vec::with_capacity(repos.len());
    for r in repos {
        if repos_count.get(r.rel).copied().unwrap_or(0) > 1 && !r.under_primary {
            continue;
        }
        for p in subpaths(r.rel) {
            if subpath_count.get(p).copied() == Some(1) {
                out.push(p);
                break;
            }
        }
    }
    out
}

/// Writes the whole listing to fd 1 in one call.
///
/// One `write_all` rather than a line-at-a-time `BufWriter`: the listing is
/// already one contiguous buffer, so buffering it again would only copy it a
/// second time.
///
/// A closed pipe is not an error. `scap list | head -1` is the ordinary way
/// to use the command, and whether the previous writer even noticed depended
/// on the size of the listing against the kernel's pipe buffer: a listing the
/// buffer accepted whole was written before `head` exited and raised no error
/// at all, while a larger one failed part-way and left the command exiting 1.
/// Measured on this machine, a 44 KB listing exited 0 and a 94 KB one exited
/// 1 — the status depended on how many repositories the machine happened to
/// hold. It is 0 in both cases now.
fn write_stdout(buf: &[u8]) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    match out.write_all(buf).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
