//! The explicit gitconfig source list (ADR-8).
//!
//! `gix_config::File::from_globals` is deliberately not used: its
//! `Source::GitInstallation` level runs `git config -l --show-origin`, which
//! is the very spawn this wave removes. The list below is git's own order,
//! built from an [`Env`] view.

use std::path::{Path, PathBuf};

use gix_config::Source;

use super::Env;

/// The enumerated sources, plus what the enumeration observed.
pub struct SourceList {
    /// Candidate files in git's precedence order. A file that does not exist
    /// is skipped at parse time rather than filtered here.
    pub files: Vec<(PathBuf, Source)>,
    /// How many system-config candidates actually exist. More than one is an
    /// ADR-8 spawn trigger, because git reads exactly one
    /// `$(prefix)/etc/gitconfig` and scap cannot tell which.
    pub system_probe_matches: usize,
    /// The `.git` directory containing [`Env::cwd`], if any. For a linked
    /// worktree this is the worktree-private directory, which is the context
    /// `includeIf gitdir:` conditions are evaluated against.
    pub git_dir: Option<PathBuf>,
}

/// The `$(prefix)/etc/gitconfig` locations a git on this platform may use.
///
/// git compiles exactly one of these in; scap cannot ask the binary without
/// spawning it (`git var GIT_CONFIG_SYSTEM` is unsupported by the Homebrew
/// build on the reference machine), so it probes and treats "more than one
/// exists" as ambiguous.
pub fn default_system_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/etc/gitconfig"),
        PathBuf::from("/usr/local/etc/gitconfig"),
        PathBuf::from("/opt/homebrew/etc/gitconfig"),
        PathBuf::from("/opt/local/etc/gitconfig"),
    ]
}

/// The candidates that exist, in the order given.
///
/// At most one `stat` per candidate; the caller decides what more than one
/// match means.
pub fn probe_system_config(candidates: &[PathBuf]) -> Vec<PathBuf> {
    candidates.iter().filter(|path| path.is_file()).cloned().collect()
}

/// Resolve `git` by walking [`Env::path`] the way a shell does.
///
/// Returning the resolved absolute path rather than relying on the child
/// process' own lookup is what makes "a trigger fired but there is no `git`"
/// a deterministic [`super::ConfigError::GitRequired`] instead of a spawn
/// error, and it is what lets a test inject an empty `PATH` without touching
/// the process environment.
pub fn resolve_git_program(env: &Env) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let path = env.path.as_ref()?;
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join("git");
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

/// Build the source list for `env`, in git's order.
pub fn enumerate(env: &Env) -> SourceList {
    let mut files = Vec::with_capacity(5);
    let mut system_probe_matches = 0;

    // (1) system.
    if let Some(explicit) = &env.git_config_system {
        files.push((explicit.clone(), Source::System));
    } else if !truthy(env.git_config_nosystem.as_deref()) {
        let matched = probe_system_config(&env.system_probe_candidates);
        system_probe_matches = matched.len();
        files.extend(matched.into_iter().map(|path| (path, Source::System)));
    }

    // (2) XDG global and (3) the user file, both replaced by
    // `GIT_CONFIG_GLOBAL` when it is set, then deduped by canonical path so
    // one file reachable under two spellings is parsed once.
    let global: Vec<(PathBuf, Source)> = match &env.git_config_global {
        Some(explicit) => vec![(explicit.clone(), Source::User)],
        None => {
            let mut candidates = Vec::with_capacity(2);
            let xdg = env
                .xdg_config_home
                .clone()
                .or_else(|| env.home.as_ref().map(|home| home.join(".config")));
            if let Some(xdg) = xdg {
                candidates.push((xdg.join("git").join("config"), Source::Git));
            }
            if let Some(home) = &env.home {
                candidates.push((home.join(".gitconfig"), Source::User));
            }
            candidates
        }
    };
    let mut seen: Vec<PathBuf> = Vec::with_capacity(global.len());
    for (path, source) in global {
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        files.push((path, source));
    }

    // (4) the repository containing the cwd. The repository-level file
    // lives in the COMMON directory: in a linked worktree the git dir is
    // `<main>/.git/worktrees/<name>`, and git reads `<main>/.git/config`
    // from there, never the worktree-private `config` file.
    let git_dir = discover_git_dir(env);
    if let Some(git_dir) = &git_dir {
        let local = common_dir_of(git_dir).join("config");
        // `config.worktree` is read only when the repository opted in, so
        // reading it unconditionally would resurrect values git ignores.
        // The `stat` comes first: almost no repository has the file, and
        // when it is absent the extension does not need looking up at all.
        let worktree = git_dir.join("config.worktree");
        let worktree_config = worktree.is_file() && worktree_config_enabled(&local);
        files.push((local, Source::Local));
        if worktree_config {
            files.push((worktree, Source::Worktree));
        }
    }

    SourceList { files, system_probe_matches, git_dir }
}

/// git's common directory for `git_dir`.
///
/// A linked worktree's git dir holds a `commondir` file naming the main
/// repository's git directory, usually relatively. A main worktree has no
/// such file and is its own common directory.
fn common_dir_of(git_dir: &Path) -> PathBuf {
    match gix_discover::path::from_plain_file_relative_to_file(&git_dir.join("commondir")) {
        Some(Ok(common)) => common,
        _ => git_dir.to_owned(),
    }
}

/// Whether `extensions.worktreeConfig` is set in the repository-level file.
///
/// git-config(5): `$GIT_DIR/config.worktree` is read "if
/// `extensions.worktreeConfig` is enabled". Answering it needs the local
/// file before the full source list is parsed, so it is read once here
/// without following includes -- git does not follow includes to decide
/// this either, because the extension has to be readable before the
/// repository is fully set up.
fn worktree_config_enabled(local_config: &Path) -> bool {
    let Ok(bytes) = std::fs::read(local_config) else {
        return false;
    };
    let parsed = gix_config::File::from_bytes_no_includes(
        &bytes,
        gix_config::file::Metadata::from(Source::Local),
        Default::default(),
    );
    parsed
        .ok()
        .and_then(|file| super::boolean_of(&file, "extensions", "worktreeConfig"))
        .unwrap_or(false)
}

fn discover_git_dir(env: &Env) -> Option<PathBuf> {
    if let Some(explicit) = &env.git_dir {
        return Some(explicit.clone());
    }
    let cwd: &Path = env.cwd.as_deref()?;
    let ceiling_dirs = env
        .git_ceiling_directories
        .as_ref()
        .map(|value| std::env::split_paths(value).collect::<Vec<_>>())
        .unwrap_or_default();
    let options = gix_discover::upwards::Options {
        ceiling_dirs,
        // git ignores ceilings that do not apply rather than failing.
        match_ceiling_dir_or_error: false,
        current_dir: Some(cwd),
        ..Default::default()
    };
    let (path, _trust) = gix_discover::upwards_opts(cwd, options).ok()?;
    let (git_dir, _work_tree) = path.into_repository_and_work_tree_directories();
    Some(git_dir)
}

/// git's `git_env_bool` truthiness for the environment variables ADR-8
/// reads: unset, empty, and the false spellings are false; everything else
/// is true, so a garbage value errs towards *skipping* the system config
/// rather than silently reading it.
fn truthy(value: Option<&std::ffi::OsStr>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let Some(text) = value.to_str() else {
        return true;
    };
    !matches!(text.to_ascii_lowercase().as_str(), "" | "0" | "false" | "no" | "off")
}

#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
