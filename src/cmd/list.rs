use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Args;

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
    rel_parts: Vec<String>,
    is_under_primary: bool,
}

impl DiscoveredRepo {
    fn rel_path(&self) -> String {
        self.rel_parts.join("/")
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
        walk_for_repos(root, root, &mut repos, primary.as_deref())?;
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
    sorted.sort();
    for line in sorted {
        println!("{}", line);
    }
    Ok(())
}

fn walk_for_repos(
    root: &Path,
    cursor: &Path,
    out: &mut Vec<DiscoveredRepo>,
    primary: Option<&Path>,
) -> anyhow::Result<()> {
    let entries = match fs::read_dir(cursor) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(e) => {
            return Err(anyhow::Error::from(e).context(format!("read_dir {}", cursor.display())));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("entry under {}", cursor.display()))?;
        let path = entry.path();
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let real_path = if ft.is_symlink() {
            match fs::canonicalize(&path) {
                Ok(p) => p,
                Err(_) => continue,
            }
        } else {
            path.clone()
        };
        let meta = match fs::metadata(&real_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_dir() {
            continue;
        }
        if is_git_repo(&real_path) {
            if let Some(rel_parts) = rel_parts(root, &real_path) {
                let is_under_primary = primary.map(|p| real_path.starts_with(p)).unwrap_or(false);
                out.push(DiscoveredRepo {
                    full_path: real_path,
                    rel_parts,
                    is_under_primary,
                });
            }
            continue;
        }
        walk_for_repos(root, &path, out, primary)?;
    }
    Ok(())
}

fn is_git_repo(dir: &Path) -> bool {
    if dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".git"))
    {
        return true;
    }
    dir.join(".git").is_dir()
}

fn rel_parts(root: &Path, full: &Path) -> Option<Vec<String>> {
    let rel = full.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_owned()))
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() { None } else { Some(parts) }
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
