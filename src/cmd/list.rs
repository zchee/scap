use std::collections::{HashMap, HashSet};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::Args;
use jwalk::{DirEntry, Parallelism, WalkDirGeneric};

use crate::config;

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
        if self.rel_parts.len() <= 1 {
            String::new()
        } else {
            self.rel_parts[1..].join("/")
        }
    }

    // ghq local_repository.go:Subpaths — tails of the relative path, shortest first.
    fn subpaths(&self) -> Vec<String> {
        let n = self.rel_parts.len();
        (0..n)
            .map(|i| self.rel_parts[n - (i + 1)..].join("/"))
            .collect()
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
        repos
            .iter()
            .map(|r| r.full_path.display().to_string())
            .collect::<Vec<_>>()
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
    if is_repo_path(root) {
        maybe_push_repo(root.to_path_buf(), root, out, primary, seen);
        return Ok(());
    }

    let walker = WalkDirGeneric::<((), bool)>::new(root)
        .follow_links(true)
        .parallelism(Parallelism::RayonNewPool(4))
        .skip_hidden(false)
        .sort(false)
        .process_read_dir(|_, _, _, children| {
            for child in children.iter_mut() {
                let child = match child {
                    Ok(child) => child,
                    Err(_) => continue,
                };

                if is_repo_dir_entry(child) {
                    child.read_children_path = None;
                    child.client_state = true;
                }
            }
        });

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };

        if !entry.file_type().is_dir() || !entry.client_state {
            continue;
        }

        let repo_root = normalize_repo_root(entry.path());
        maybe_push_repo(repo_root, root, out, primary, seen);
    }

    Ok(())
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
        out.push(DiscoveredRepo {
            full_path,
            rel_path,
            rel_parts,
            is_under_primary,
        });
    }
}

fn is_repo_dir_entry(entry: &DirEntry<((), bool)>) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }

    is_repo_path(&entry.path())
}

fn is_repo_path(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.ends_with(".git"))
    {
        return true;
    }

    path.join(".git").exists()
}

fn normalize_repo_root(path: PathBuf) -> PathBuf {
    if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
        return path
            .parent()
            .map_or_else(|| path.clone(), |p| p.to_path_buf());
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
    if parts.is_empty() {
        Some(vec![".".to_owned()])
    } else {
        Some(parts)
    }
}

fn filter_repos(repos: Vec<DiscoveredRepo>, args: &ListArgs) -> Vec<DiscoveredRepo> {
    let Some(query) = args.query.as_deref() else {
        return repos;
    };

    if args.exact {
        repos
            .into_iter()
            .filter(|r| r.matches_exact(query))
            .collect()
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
                if smart_case {
                    hay.to_lowercase().contains(&lower)
                } else {
                    hay.contains(&q)
                }
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
