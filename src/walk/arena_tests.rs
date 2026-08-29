use super::*;

fn collect(arena: &Arena) -> Vec<&[u8]> {
    arena.iter().collect()
}

#[test]
fn a_fresh_arena_is_empty() {
    let arena = Arena::default();
    assert!(arena.is_empty(), "a fresh arena holds no repository");
    assert_eq!(arena.len(), 0);
    assert!(collect(&arena).is_empty());
}

#[test]
fn push_keeps_every_path_addressable_at_its_own_offset() {
    let mut arena = Arena::default();
    // Adjacent pushes share the byte buffer, so a handle that were off by one
    // would splice two paths together rather than fail.
    arena.push(b"github.com/a/one");
    arena.push(b"github.com/a/two");
    arena.push(b".");

    assert_eq!(arena.len(), 3);
    assert!(!arena.is_empty());
    assert_eq!(
        collect(&arena),
        vec![&b"github.com/a/one"[..], &b"github.com/a/two"[..], &b"."[..],]
    );
}

#[test]
fn push_preserves_bytes_that_are_not_utf8() {
    // ADR-9: names are bytes end to end. A repository whose name the local
    // encoding cannot decode still has to come back exactly as it went in.
    let mut arena = Arena::default();
    arena.push(b"host/\xff\xfe/repo");
    arena.push("ホスト/ユーザ 名/レポ".as_bytes());

    assert_eq!(
        collect(&arena),
        vec![&b"host/\xff\xfe/repo"[..], "ホスト/ユーザ 名/レポ".as_bytes(),]
    );
}

#[test]
fn iter_yields_paths_in_push_order_and_reports_its_length() {
    let mut arena = Arena::default();
    for path in [&b"b"[..], b"a", b"c"] {
        arena.push(path);
    }
    // Walk order, not sorted order: sorting is `list`'s job, once, over every
    // root's output at the same time (ADR-9 rule vii).
    assert_eq!(arena.iter().len(), 3);
    assert_eq!(collect(&arena), vec![&b"b"[..], b"a", b"c"]);
}
