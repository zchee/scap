use std::time::Duration;

use super::*;

/// A repository under the test root `/r`, named by that root.
///
/// `rel` is a tail of `abs` in the product code too — `run` slices both out
/// of one buffer — so building them any other way would test a shape that
/// cannot occur.
fn repo(abs: &str) -> Repo<'_> {
    let rel = abs.strip_prefix("/r/").expect("test repositories live under /r");
    Repo { abs: abs.as_bytes(), rel: rel.as_bytes(), under_primary: true }
}

/// A repository named by some root other than the primary one.
fn foreign<'a>(abs: &'a str, rel: &'a str) -> Repo<'a> {
    Repo { abs: abs.as_bytes(), rel: rel.as_bytes(), under_primary: false }
}

fn args(query: Option<&str>) -> ListArgs {
    ListArgs {
        exact: false,
        vcs: None,
        full_path: false,
        unique: false,
        bare: false,
        query: query.map(str::to_owned),
    }
}

fn rendered(lines: &[&[u8]]) -> Vec<String> {
    lines.iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect()
}

fn rels(repos: &[Repo<'_>]) -> Vec<String> {
    repos.iter().map(|r| String::from_utf8_lossy(r.rel).into_owned()).collect()
}

// -- path arithmetic -------------------------------------------------------

/// ghq `local_repository.go:Subpaths` yields the tails shortest first, and
/// both callers depend on the order rather than the set: `--unique` prints
/// the first tail nothing else shares, so a longest-first iterator would
/// print the whole path every time and still satisfy any assertion that only
/// checked membership.
#[test]
fn subpaths_yields_every_component_tail_shortest_first() {
    assert_eq!(
        rendered(&subpaths(b"github.com/zchee/scap").collect::<Vec<_>>()),
        vec!["scap", "zchee/scap", "github.com/zchee/scap"]
    );
    assert_eq!(rendered(&subpaths(b"solo").collect::<Vec<_>>()), vec!["solo"]);
    // A root that is itself a repository: its one tail is what ghq prints.
    assert_eq!(rendered(&subpaths(b".").collect::<Vec<_>>()), vec!["."]);
}

#[test]
fn non_host_path_drops_the_leading_component_and_host_component_keeps_it() {
    assert_eq!(non_host_path(b"github.com/zchee/scap"), b"zchee/scap".as_slice());
    assert_eq!(host_component(b"github.com/zchee/scap"), b"github.com".as_slice());
    // One component: ghq's `NonHostPath` is empty there, so a bare query can
    // never match such a repository.
    assert_eq!(non_host_path(b"solo"), b"".as_slice());
    assert_eq!(host_component(b"solo"), b"solo".as_slice());
}

#[test]
fn push_full_path_joins_the_way_path_join_renders_it() {
    let mut buf = Vec::new();
    push_full_path(&mut buf, Path::new("/roots/one"), b"github.com/a/x");
    assert_eq!(buf.as_slice(), b"/roots/one/github.com/a/x".as_slice());

    // A root that is itself a repository prints as the root, not `<root>/.`
    // -- checked against ghq 1.8.0 for a plain repository root and a bare
    // `*.git` one.
    buf.clear();
    push_full_path(&mut buf, Path::new("/roots/one"), b".");
    assert_eq!(buf.as_slice(), b"/roots/one".as_slice());

    // The one root spelling that already ends in a separator. `Path::join`
    // does not double it and neither does this.
    buf.clear();
    push_full_path(&mut buf, Path::new("/"), b"github.com/a/x");
    assert_eq!(buf.as_slice(), b"/github.com/a/x".as_slice());

    // Bytes, not text: a name the local encoding cannot decode survives.
    buf.clear();
    push_full_path(&mut buf, Path::new("/roots/one"), b"host/\xff\xfe/repo");
    assert_eq!(buf.as_slice(), b"/roots/one/host/\xff\xfe/repo".as_slice());
}

/// The naming rule's whole predicate. It has to land on a component
/// boundary: under a root `/roots/one` the path `/roots/onetwo/x` shares nine
/// bytes and is a different tree. ghq compares raw bytes here and so renders
/// that repository as `../onetwo/x`, a path outside every configured root —
/// the divergence ADR-13 records and `list_sibling_prefix_roots_do_not_escape
/// _the_root` pins against the live oracle.
#[test]
fn root_prefix_matches_on_component_boundaries_only() {
    // The cut is the offset of the naming tail, so the remainder carries no
    // leading separator.
    assert_eq!(root_prefix(b"/roots/one", b"/roots/one/github.com/a/x"), Some(11));
    assert_eq!(&b"/roots/one/github.com/a/x"[11..], b"github.com/a/x".as_slice());

    // A root that *is* the repository: the remainder is empty and `run`
    // renders it as `.`.
    assert_eq!(root_prefix(b"/roots/one", b"/roots/one"), Some(10));

    // The sibling that shares nine bytes and no component.
    assert_eq!(root_prefix(b"/roots/one", b"/roots/onetwo/x"), None);
    // Unrelated, and a root longer than the path.
    assert_eq!(root_prefix(b"/roots/two", b"/roots/one/x"), None);
    assert_eq!(root_prefix(b"/roots/one/deep", b"/roots/one"), None);

    // The one root spelling that already ends in a separator.
    assert_eq!(root_prefix(b"/", b"/github.com/a/x"), Some(1));
    assert_eq!(root_prefix(b"/", b"/"), Some(1));
}

/// ghq's rule end to end: the first configured root that contains a
/// repository names it, so the configured *order* decides what is printed
/// when one root is nested inside another. Both orders are checked against
/// the same tree on disk; the repositories found never change, only their
/// names (`local_repository.go:LocalRepositoryFromFullPath`, measured against
/// ghq 1.8.0).
#[test]
fn the_first_configured_root_that_contains_a_repository_names_it() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outer = tmp.path().join("outer");
    let inner = outer.join("inner");
    for rel in ["github.com/a/dup", "github.com/b/only"] {
        std::fs::create_dir_all(outer.join(rel).join(".git")).expect("fixture");
    }
    std::fs::create_dir_all(inner.join("github.com/c/uniq").join(".git")).expect("fixture");

    let named = |roots: Vec<std::path::PathBuf>| {
        let opts = WalkOptions::new(2, Vec::new());
        let listings: Vec<RootListing> =
            roots.iter().map(|r| walk::walk_root(r, &opts).expect("walk")).collect();
        let (paths, spans) = name_repositories(&roots, &listings);
        let mut out: Vec<String> = spans
            .iter()
            .map(|s| String::from_utf8_lossy(&paths[s.rel..s.end]).into_owned())
            .collect();
        out.sort();
        out
    };

    // Inner first: the repository under `inner` is named by `inner`, even the
    // copy the outer walk found, so its `inner/` segment disappears.
    assert_eq!(
        named(vec![inner.clone(), outer.clone()]),
        vec!["github.com/a/dup", "github.com/b/only", "github.com/c/uniq", "github.com/c/uniq"]
    );
    // Outer first: the same repository is named by `outer` from both walks
    // and keeps the segment. The set on disk did not change.
    assert_eq!(
        named(vec![outer, inner]),
        vec![
            "github.com/a/dup",
            "github.com/b/only",
            "inner/github.com/c/uniq",
            "inner/github.com/c/uniq",
        ]
    );
}

/// The `--unique` tiebreak follows the naming root: a repository the primary
/// root does not contain is the copy that loses.
///
/// The pairs are asserted exactly rather than as a co-variation between the
/// flag and the name. Under a last-match rule both would move together — the
/// repository under `inner` would be named `inner/github.com/b/y` *and* stop
/// being under the primary root — and a test that only related the two would
/// still pass.
#[test]
fn only_the_primary_root_can_name_a_repository_under_primary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outer = tmp.path().join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(outer.join("github.com/a/x").join(".git")).expect("fixture");
    std::fs::create_dir_all(inner.join("github.com/b/y").join(".git")).expect("fixture");

    let roots = vec![inner, outer];
    let opts = WalkOptions::new(2, Vec::new());
    let listings: Vec<RootListing> =
        roots.iter().map(|r| walk::walk_root(r, &opts).expect("walk")).collect();
    let (paths, spans) = name_repositories(&roots, &listings);

    let mut named: Vec<(String, bool)> = spans
        .iter()
        .map(|s| (String::from_utf8_lossy(&paths[s.rel..s.end]).into_owned(), s.under_primary))
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec![
            // Found only by the outer walk, and `inner` does not contain it,
            // so the second root names it and it is not under the primary.
            ("github.com/a/x".to_owned(), false),
            // The same repository twice, once from each walk, and `inner`
            // names it both times because `inner` comes first.
            ("github.com/b/y".to_owned(), true),
            ("github.com/b/y".to_owned(), true),
        ]
    );
}

// -- case folding ----------------------------------------------------------

#[test]
fn lowercase_folds_ascii_in_place_and_unicode_the_way_to_lowercase_does() {
    let mut buf = Vec::new();
    assert_eq!(lowercase(&mut buf, b"GitHub.com/ZChee/SCAP"), b"github.com/zchee/scap".as_slice());

    // U+212A, the Kelvin sign, lowercases to a plain ASCII `k`, so a query
    // for `kelvin` matches it. An ASCII-only fold would miss that, which is
    // why a non-ASCII haystack still goes through `str::to_lowercase`.
    assert_eq!(lowercase(&mut buf, "\u{212a}elvin".as_bytes()), b"kelvin".as_slice());

    // A fold longer than its input: U+0130 becomes `i` plus a combining dot
    // above. No fold-in-place implementation can produce this.
    assert_eq!(lowercase(&mut buf, "\u{130}stanbul".as_bytes()), "i\u{307}stanbul".as_bytes());

    // Not UTF-8 at all: folded as ASCII rather than dropped. The previous
    // filter decoded first and lost the component entirely.
    assert_eq!(lowercase(&mut buf, b"HOST/\xff\xfe/REPO"), b"host/\xff\xfe/repo".as_slice());
}

#[test]
fn looks_like_authority_needs_a_dot_and_no_space() {
    assert!(looks_like_authority("github.com"));
    assert!(!looks_like_authority("zchee"));
    assert!(!looks_like_authority("git hub.com"));
}

#[test]
fn split_authority_prefix_only_splits_a_host_looking_head() {
    assert_eq!(split_authority_prefix("github.com/scap"), (Some("github.com"), "scap".to_owned()));
    assert_eq!(split_authority_prefix("zchee/scap"), (None, "zchee/scap".to_owned()));
    assert_eq!(split_authority_prefix("scap"), (None, "scap".to_owned()));
}

// -- filtering -------------------------------------------------------------

#[test]
fn no_query_keeps_every_repository() {
    let repos = vec![repo("/r/github.com/a/x"), repo("/r/github.com/b/y")];
    assert_eq!(rels(&filter_repos(repos, &args(None))), vec!["github.com/a/x", "github.com/b/y"]);
}

/// `-e` matches a whole component tail and never a substring: `ca` is not
/// `scap`, and `hee/scap` is not `zchee/scap`.
#[test]
fn exact_query_matches_a_component_tail_and_nothing_else() {
    let all = || vec![repo("/r/github.com/zchee/scap"), repo("/r/github.com/zchee/other")];
    let mut exact = args(Some("scap"));
    exact.exact = true;
    assert_eq!(rels(&filter_repos(all(), &exact)), vec!["github.com/zchee/scap"]);

    for hit in ["zchee/scap", "github.com/zchee/scap"] {
        exact.query = Some(hit.to_owned());
        assert_eq!(rels(&filter_repos(all(), &exact)), vec!["github.com/zchee/scap"], "{hit}");
    }
    for miss in ["ca", "hee/scap", "github.com/zchee", "com/zchee/scap"] {
        exact.query = Some(miss.to_owned());
        assert!(filter_repos(all(), &exact).is_empty(), "{miss} should match nothing");
    }
}

/// The query is matched against the path *without* its host component, which
/// is what stops a bare `github` from selecting the whole corpus.
#[test]
fn substring_query_ignores_the_host_component() {
    let all = || vec![repo("/r/github.com/zchee/scap"), repo("/r/gitlab.com/other/thing")];
    assert_eq!(rels(&filter_repos(all(), &args(Some("zchee")))), vec!["github.com/zchee/scap"]);
    assert!(filter_repos(all(), &args(Some("github"))).is_empty());
}

#[test]
fn a_lowercase_query_is_case_insensitive_and_an_uppercase_one_is_literal() {
    let all = || vec![repo("/r/github.com/ZChee/Scap"), repo("/r/github.com/other/thing")];
    // Smart case: an all-lowercase query folds the haystack.
    assert_eq!(rels(&filter_repos(all(), &args(Some("zchee")))), vec!["github.com/ZChee/Scap"]);
    // Any uppercase in the query makes the match literal.
    assert_eq!(rels(&filter_repos(all(), &args(Some("ZChee")))), vec!["github.com/ZChee/Scap"]);
    assert!(filter_repos(all(), &args(Some("ZCHEE"))).is_empty());
}

#[test]
fn a_host_prefixed_query_filters_by_host_before_matching_the_rest() {
    let all = || {
        vec![
            repo("/r/github.com/zchee/scap"),
            repo("/r/gitlab.com/zchee/scap"),
            repo("/r/github.com/other/scap"),
        ]
    };
    assert_eq!(
        rels(&filter_repos(all(), &args(Some("github.com/zchee")))),
        vec!["github.com/zchee/scap"]
    );
    // The head is not host-shaped, so it stays part of the substring.
    assert_eq!(
        rels(&filter_repos(all(), &args(Some("zchee/scap")))),
        vec!["github.com/zchee/scap", "gitlab.com/zchee/scap"]
    );
}

// -- `--unique` ------------------------------------------------------------

#[test]
fn unique_prints_the_shortest_tail_nothing_else_shares() {
    let repos = vec![
        repo("/r/github.com/zchee/scap"),
        repo("/r/github.com/other/scap"),
        repo("/r/github.com/zchee/solo"),
    ];
    // `scap` is ambiguous, so both of its repositories fall back to the
    // owner-qualified tail; `solo` is not, so it prints bare.
    assert_eq!(rendered(&format_unique(&repos)), vec!["zchee/scap", "other/scap", "solo"]);
}

/// A path two roots both hold is printed once, by the primary root — ghq's
/// tiebreak. Without it the same tail appears twice and is then reported as
/// ambiguous against itself.
#[test]
fn unique_keeps_the_primary_root_copy_of_a_duplicated_path() {
    let repos = vec![
        repo("/r/github.com/a/dup"),
        foreign("/second/github.com/a/dup", "github.com/a/dup"),
        foreign("/second/github.com/b/solo", "github.com/b/solo"),
    ];
    assert_eq!(rendered(&format_unique(&repos)), vec!["dup", "solo"]);
}

/// A repository whose every tail is shared prints nothing at all, which is
/// why `--unique` output can be shorter than the listing — 844 lines against
/// 845 on the author's corpus a.
///
/// It takes a repository nested under another whose *suffix* is a second
/// repository's whole path: then the shorter one has no tail of its own, and
/// only the longer one prints. Two copies of the same path do not do it, and
/// assuming they did was this test's first, wrong, shape — the duplicate's
/// tails are counted once, so both copies still find `dup` unambiguous.
#[test]
fn unique_drops_a_repository_whose_every_tail_belongs_to_another() {
    let repos = vec![repo("/r/github.com/a/dup"), repo("/r/mirror/github.com/a/dup")];
    assert_eq!(rendered(&format_unique(&repos)), vec!["mirror/github.com/a/dup"]);
}

/// The counterpart: two roots holding the identical path is not that case.
/// The tails are counted once per distinct path, so the survivor still finds
/// its shortest tail unambiguous.
#[test]
fn unique_does_not_drop_a_duplicated_path_that_is_still_unambiguous() {
    let repos = vec![repo("/r/github.com/a/dup"), repo("/r/github.com/a/dup")];
    // Both survive: neither is skipped, because both are under the primary
    // root, and `dup` is unambiguous for each.
    assert_eq!(rendered(&format_unique(&repos)), vec!["dup", "dup"]);
}

// -- output ----------------------------------------------------------------

#[test]
fn micros_reports_whole_microseconds_and_saturates_rather_than_wrapping() {
    assert_eq!(micros(Duration::from_micros(0)), 0);
    assert_eq!(micros(Duration::from_millis(3)), 3_000);
    // More microseconds than a `u64` holds. A span field that wrapped would
    // report a 584,000-year pass as a fast one.
    assert_eq!(micros(Duration::MAX), u64::MAX);
}

#[test]
fn write_stdout_accepts_an_empty_listing() {
    // An empty root prints nothing and exits 0; the write still has to
    // succeed rather than report a short write.
    write_stdout(b"").expect("empty listing writes cleanly");
}

#[test]
fn walk_one_reports_the_repositories_under_a_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for rel in ["github.com/a/x", "github.com/b/y"] {
        std::fs::create_dir_all(tmp.path().join(rel).join(".git")).expect("repository fixture");
    }
    let listing = walk_one(tmp.path(), &WalkOptions::new(2, Vec::new())).expect("walk");
    let mut found: Vec<String> =
        listing.repos().map(|r| String::from_utf8_lossy(r).into_owned()).collect();
    found.sort_unstable();
    assert_eq!(found, vec!["github.com/a/x", "github.com/b/y"]);
    assert_eq!(listing.excluded(), 0);
}
