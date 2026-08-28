use std::path::{Path, PathBuf};

use crate::url::Repo;

// ghq local_repository.go:75-86
pub fn dest_path(root: &Path, repo: &Repo, bare: bool) -> PathBuf {
    root.join(rel_path(repo, bare))
}

pub fn rel_path(repo: &Repo, bare: bool) -> PathBuf {
    debug_assert_eq!(
        repo.host,
        repo.host.to_ascii_lowercase(),
        "Repo.host must already be lowercased upstream",
    );

    let mut path = PathBuf::new();
    path.push(&repo.host);
    for segment in repo.owner.split('/').filter(|s| !s.is_empty()) {
        path.push(segment);
    }
    if bare {
        path.push(format!("{}.git", repo.name));
    } else {
        path.push(&repo.name);
    }
    path
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
