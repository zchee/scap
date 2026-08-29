//! Byte arena holding the repository paths one walk produced.
//!
//! Repository paths are root-relative byte strings, never `PathBuf`s: the
//! walk reads them out of `readdir` as bytes and `list` writes them to fd 1
//! as bytes, so nothing in between needs to decode them (ADR-9's "entry
//! names are bytes end-to-end"). Storing every path in one growable `Vec<u8>`
//! behind `Range<u32>` handles keeps that path allocation-free per
//! repository: a walk of corpus a+b emits 1,822 paths into two allocations
//! that amortise, rather than 1,822 separate ones.
//!
//! Handles are offsets rather than borrowed slices because each worker fills
//! its own arena and the per-root arena is their concatenation: merging two
//! arenas shifts the incoming handles by the current byte length, which a
//! borrowed slice could not survive. The merge itself arrives with the pool
//! in W3.2; until then one walk fills one arena.

use std::ops::Range;

/// Concatenated repository paths plus one handle per path.
///
/// Handles are `u32` because a `usize` pair per repository would double the
/// index's footprint for a range no corpus approaches: the arena would have
/// to hold 4 GiB of path bytes to overflow one.
#[derive(Default, Debug)]
pub(crate) struct Arena {
    bytes: Vec<u8>,
    repos: Vec<Range<u32>>,
}

impl Arena {
    /// Appends one repository path.
    ///
    /// # Panics
    ///
    /// Panics if the arena would grow past 4 GiB, which needs on the order of
    /// 40 million repository paths in one root and means the walk has left
    /// the domain this type was sized for.
    pub(crate) fn push(&mut self, path: &[u8]) {
        let start = Self::offset(self.bytes.len());
        self.bytes.extend_from_slice(path);
        let end = Self::offset(self.bytes.len());
        self.repos.push(start..end);
    }

    /// Number of repository paths held.
    pub(crate) fn len(&self) -> usize {
        self.repos.len()
    }

    /// Whether the walk found no repository at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.repos.is_empty()
    }

    /// Every repository path, in walk order.
    ///
    /// Walk order depends on scheduling, so callers that need a stable
    /// sequence sort what this yields; the walk deliberately does not, since
    /// `list` sorts the concatenation of every root once instead
    /// (ADR-9 rule vii).
    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.repos.iter().map(|r| &self.bytes[r.start as usize..r.end as usize])
    }

    fn offset(len: usize) -> u32 {
        u32::try_from(len).expect("repository path arena grew past 4 GiB")
    }
}

#[cfg(test)]
#[path = "arena_tests.rs"]
mod tests;
