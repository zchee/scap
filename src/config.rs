//! Configuration resolution (ADR-8).
//!
//! The gitconfig is parsed **in process** with `gix-config` from an explicit
//! source list, once per process, into a [`ConfigSnapshot`]. `git` is spawned
//! for configuration only when an explicit trigger fires (see
//! [`needs_git_backend`]) or when a url-scoped `[scap "<url>"]` section is
//! visible and [`root_for_url`] has to reproduce `git config
//! --get-urlmatch`.
//!
//! The loader never reads the process environment directly: it takes an
//! [`Env`] view, so unit tests inject values instead of mutating the
//! environment. That is what let W2.1 delete `serial_test` and the tree's
//! last two `set_var` call sites, and with them the last unchecked blocks
//! anywhere in `src/`.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use bstr::{BStr, ByteSlice};

mod git_backend;
pub mod sources;

/// Which parser produced a [`ConfigSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// A4: `gix-config` parsed the explicit source list in this process.
    InProcess,
    /// A3: one `git config --list -z --show-origin` spawn produced the whole
    /// snapshot.
    Git,
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Backend::InProcess => "in_process",
            Backend::Git => "git",
        })
    }
}

/// Why a snapshot has the backend it has (the ADR-8 `reason` span field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// No trigger fired and no url-scoped section is visible.
    InProcess,
    /// Url-scoped `[scap "<url>"]` sections exist. The snapshot stays
    /// in-process; only `root_for_url` delegates (ADR-8 rule d).
    UrlSections,
    /// `GIT_CONFIG_COUNT` is set.
    GitConfigCount,
    /// `GIT_CONFIG_PARAMETERS` is set (git's highest-precedence source).
    GitConfigParameters,
    /// An `includeIf` with an `onbranch:` or `hasconfig:` condition was seen,
    /// which gix cannot evaluate without a repository context scap does not
    /// have.
    IncludeifUnevaluated,
    /// The system-config probe matched more than one file.
    SystemProbeAmbiguous,
    /// `SCAP_CONFIG_BACKEND=git`.
    EnvOverride,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Reason::InProcess => "in_process",
            Reason::UrlSections => "url_sections",
            Reason::GitConfigCount => "git_config_count",
            Reason::GitConfigParameters => "git_config_parameters",
            Reason::IncludeifUnevaluated => "includeif_unevaluated",
            Reason::SystemProbeAmbiguous => "system_probe_ambiguous",
            Reason::EnvOverride => "env_override",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to invoke `git`: {0}")]
    GitSpawn(#[from] std::io::Error),
    #[error("git config {key:?} failed (status {status}): {stderr}")]
    GitConfigFailed { key: String, status: i32, stderr: String },
    #[error("could not determine home directory")]
    NoHomeDir,
    #[error("invalid utf-8 in git config output")]
    InvalidUtf8,
    /// A spawn trigger fired (or `root_for_url` needs `--get-urlmatch`) but
    /// no `git` is reachable. ADR-8 forbids silently falling back to the
    /// in-process snapshot, so this is fatal.
    #[error("`git` is required ({reason}) but no `git` was found on PATH")]
    GitRequired { reason: &'static str },
    #[error("could not parse the git configuration reachable from {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<gix_config::file::init::from_paths::Error>,
    },
}

/// The environment the configuration loader is allowed to see.
///
/// Every variable ADR-8 names, plus the working directory used for repository
/// discovery, plus `PATH` (so the A3 backend and the `--get-urlmatch`
/// delegation resolve `git` from an injected list rather than the ambient
/// process environment — that is what makes the "no `git` on PATH" behaviour
/// testable without mutating the environment).
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub git_config_global: Option<PathBuf>,
    pub git_config_system: Option<PathBuf>,
    pub git_config_nosystem: Option<OsString>,
    pub git_config_count: Option<OsString>,
    pub git_config_parameters: Option<OsString>,
    pub git_dir: Option<PathBuf>,
    pub git_ceiling_directories: Option<OsString>,
    pub scap_root: Option<OsString>,
    pub scap_config_backend: Option<OsString>,
    pub cwd: Option<PathBuf>,
    pub path: Option<OsString>,
    /// The candidate system-config files to probe, in order. Injected so the
    /// 0/1/2-match cases of ADR-8 oracle (iv) are unit-testable;
    /// [`Env::from_process`] fills it with
    /// [`sources::default_system_candidates`].
    pub system_probe_candidates: Vec<PathBuf>,
}

impl Env {
    /// The view of the real process environment.
    pub fn from_process() -> Self {
        Self {
            home: std::env::home_dir(),
            // `XDG_CONFIG_HOME` is the one path variable git treats as
            // unset when empty (`xdg_config_home()` tests `home && *home`).
            xdg_config_home: non_empty_var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            // An empty `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` is a real path
            // to git -- an unopenable one -- so it suppresses its level
            // rather than falling back to the default files.
            git_config_global: std::env::var_os("GIT_CONFIG_GLOBAL").map(PathBuf::from),
            git_config_system: std::env::var_os("GIT_CONFIG_SYSTEM").map(PathBuf::from),
            git_config_nosystem: std::env::var_os("GIT_CONFIG_NOSYSTEM"),
            git_config_count: std::env::var_os("GIT_CONFIG_COUNT"),
            git_config_parameters: non_empty_var_os("GIT_CONFIG_PARAMETERS"),
            git_dir: non_empty_var_os("GIT_DIR").map(PathBuf::from),
            git_ceiling_directories: non_empty_var_os("GIT_CEILING_DIRECTORIES"),
            scap_root: std::env::var_os("SCAP_ROOT"),
            scap_config_backend: std::env::var_os("SCAP_CONFIG_BACKEND"),
            cwd: std::env::current_dir().ok(),
            path: std::env::var_os("PATH"),
            system_probe_candidates: sources::default_system_candidates(),
        }
    }
}

fn non_empty_var_os(key: &str) -> Option<OsString> {
    std::env::var_os(key).filter(|v| !v.is_empty())
}

/// git's answer to one `--get-urlmatch` question: the matched root, or
/// `None` when git exits 1 (no `scap.root` configured anywhere) or prints
/// nothing, both of which send [`ConfigSnapshot::root_for_url`] on to ADR-8
/// rules (c)/(e).
type UrlmatchAnswer = Option<PathBuf>;

/// One memo slot, `None` until the answer is known.
type UrlmatchCell = Arc<Mutex<Option<UrlmatchAnswer>>>;

/// The process's `git config --path --get-urlmatch` memo (ADR-8 rule d).
///
/// A slot per distinct URL rather than one lock over the whole map: the
/// slot's mutex is held across the spawn, so two threads asking the same
/// question share one answer instead of racing to spawn two `git`s, while
/// threads asking different questions never wait on each other -- which is
/// what `get --parallel` does with six workers.
#[derive(Debug, Default)]
struct UrlmatchMemo {
    cells: Mutex<HashMap<String, UrlmatchCell>>,
    /// How many `--get-urlmatch` processes this one has actually run; the
    /// `urlmatch_spawns` span field.
    spawns: AtomicUsize,
}

/// Lock a memo mutex, ignoring poisoning.
///
/// A panicking thread leaves nothing half-written here -- a slot holds
/// either the answer or nothing at all -- so refusing every later caller
/// because an unrelated thread panicked would only make a working process
/// worse.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The `scap.*` configuration of this process, as one immutable value.
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    env: Env,
    /// Plain `scap.root` values in git's own order (system, global, local),
    /// `--path`-interpolated. `resolve_roots` applies the reversal.
    roots: Vec<PathBuf>,
    /// Every `[scap "<sub>"] root`, `--path`-interpolated, in file order.
    /// Equals `git config --path --get-regexp '^scap\..+\.root$'`.
    url_scoped_roots: Vec<PathBuf>,
    user: Option<String>,
    complete_user: bool,
    list_exclude: Vec<String>,
    list_cache: bool,
    backend: Backend,
    reason: Reason,
    /// Rule (d)'s per-URL memo. Every clone of a snapshot shares it, and
    /// [`snapshot`] hands out exactly one snapshot per process, so the spawn
    /// count is per process as ADR-8 requires. It lives here rather than in
    /// a `static` so that unit tests, which build many snapshots over
    /// different fixtures in one process, cannot answer each other's
    /// questions -- and so that the A3 backend gets the same memo for free.
    urlmatch: Arc<UrlmatchMemo>,
}

impl ConfigSnapshot {
    /// Plain `scap.root` values in file order (not yet reversed).
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Every `[scap "<url>"] root` value, in file order.
    pub fn url_scoped_roots(&self) -> &[PathBuf] {
        &self.url_scoped_roots
    }

    /// `scap.user`.
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// `scap.completeUser`, with git's `--bool` truthiness (a valueless key
    /// is `true`).
    pub fn complete_user(&self) -> bool {
        self.complete_user
    }

    /// `scap.listExclude` (multi), for ADR-9 rule (viii) in Phase 2b.
    pub fn list_exclude(&self) -> &[String] {
        &self.list_exclude
    }

    /// `scap.listCache`, for ADR-10 in Phase 4b.
    pub fn list_cache(&self) -> bool {
        self.list_cache
    }

    /// Whether any `[scap "<url>"] root` is visible, which is exactly the
    /// condition under which `git config --get-urlmatch` can return
    /// something other than the plain key (ADR-8 rules c/d).
    pub fn has_url_sections(&self) -> bool {
        !self.url_scoped_roots.is_empty()
    }

    /// Which parser produced this snapshot.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// Why [`ConfigSnapshot::backend`] is what it is.
    pub fn reason(&self) -> Reason {
        self.reason
    }
}

/// The ADR-8 spawn triggers, in precedence order.
///
/// `system_probe_matches` is the number of existing files the system probe
/// matched, and `includeif_unevaluated` reports whether an `includeIf` with
/// an `onbranch:` or `hasconfig:` condition was parsed. Both are arguments
/// rather than fields of [`Env`] because they are only known part-way
/// through [`load`], which calls this once per stage.
pub fn needs_git_backend(
    env: &Env,
    system_probe_matches: usize,
    includeif_unevaluated: bool,
) -> Option<Reason> {
    if env.scap_config_backend.as_deref() == Some(OsStr::new("git")) {
        return Some(Reason::EnvOverride);
    }
    if env.git_config_count.as_deref().is_some_and(git_config_count_applies) {
        return Some(Reason::GitConfigCount);
    }
    if env.git_config_parameters.is_some() {
        return Some(Reason::GitConfigParameters);
    }
    if system_probe_matches > 1 {
        return Some(Reason::SystemProbeAmbiguous);
    }
    if includeif_unevaluated {
        return Some(Reason::IncludeifUnevaluated);
    }
    None
}

/// Whether `GIT_CONFIG_COUNT` contributes anything.
///
/// git reads it with `strtoul`, which yields zero for an empty value
/// without reporting an error, so an empty count adds no keys and is not a
/// trigger -- and neither is an explicit `0`. A value git cannot parse at
/// all makes it die, so that routes to the A3 backend and lets git be the
/// one to report it.
fn git_config_count_applies(value: &OsStr) -> bool {
    let Some(text) = value.to_str() else {
        return true;
    };
    if text.is_empty() {
        return false;
    }
    match text.parse::<u64>() {
        Ok(count) => count > 0,
        Err(_) => true,
    }
}

/// Build the snapshot for `env`.
///
/// Spawns `git` exactly once, and only when [`needs_git_backend`] reports a
/// trigger; otherwise nothing outside this process runs.
pub fn load(env: &Env) -> Result<ConfigSnapshot, ConfigError> {
    // ADR-9's instrumentation lesson: `Span::record` on a field that was not
    // declared at creation silently no-ops, so every field is declared here.
    //
    // `urlmatch_spawns` is *not* declared here, although plan §7 lists it
    // among this span's fields: rule (d) runs from `root_for_url`, long
    // after `load` returned and this span closed, so the only value it
    // could ever carry here is zero. The count is recorded where it is
    // real, on the per-lookup `scap::config::urlmatch` span below.
    let span = tracing::debug_span!(
        "scap::config::load",
        backend = tracing::field::Empty,
        reason = tracing::field::Empty,
        sources = tracing::field::Empty,
        url_sections = tracing::field::Empty,
    );
    let _entered = span.enter();

    // Stage 1: environment-only triggers, decided before a single file is
    // opened.
    if let Some(reason) = needs_git_backend(env, 0, false) {
        let snapshot = git_backend::load(env, reason)?;
        record(&span, &snapshot, 0);
        return Ok(snapshot);
    }

    // Stage 2: enumerate the sources; the probe can itself trigger.
    let list = sources::enumerate(env);
    if let Some(reason) = needs_git_backend(env, list.system_probe_matches, false) {
        let snapshot = git_backend::load(env, reason)?;
        record(&span, &snapshot, list.files.len());
        return Ok(snapshot);
    }

    // Stage 3: parse in process, then check the one trigger only parsing can
    // reveal.
    let file = parse(&list, env)?;
    let includeif_unevaluated = file.as_ref().is_some_and(has_unevaluable_includeif);
    if let Some(reason) = needs_git_backend(env, list.system_probe_matches, includeif_unevaluated) {
        let snapshot = git_backend::load(env, reason)?;
        record(&span, &snapshot, list.files.len());
        return Ok(snapshot);
    }

    let snapshot = from_file(file.as_ref(), env);
    record(&span, &snapshot, list.files.len());
    Ok(snapshot)
}

fn record(span: &tracing::Span, snapshot: &ConfigSnapshot, sources: usize) {
    span.record("backend", tracing::field::display(snapshot.backend));
    span.record("reason", tracing::field::display(snapshot.reason));
    span.record("sources", sources);
    span.record("url_sections", snapshot.url_scoped_roots.len());
}

/// The process-wide snapshot, built once on first use.
///
/// ADR-8 makes an unsatisfiable trigger fatal rather than silently
/// degrading, so a load failure exits 1 with a message naming the trigger
/// instead of being reported per call site.
pub fn snapshot() -> &'static ConfigSnapshot {
    static SNAPSHOT: OnceLock<Result<ConfigSnapshot, ConfigError>> = OnceLock::new();
    match SNAPSHOT.get_or_init(|| load(&Env::from_process())) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            eprintln!("scap: {err}");
            std::process::exit(1);
        }
    }
}

fn parse(list: &sources::SourceList, env: &Env) -> Result<Option<gix_config::File>, ConfigError> {
    let interpolate = interpolate_context(env);
    let conditional = gix_config::file::includes::conditional::Context {
        git_dir: list.git_dir.as_deref(),
        // `onbranch:` needs a checked-out branch scap does not resolve; the
        // condition evaluates to "no match" here and the section itself is
        // what routes the snapshot to the A3 backend instead.
        branch_name: None,
    };
    let options = gix_config::file::init::Options {
        includes: gix_config::file::includes::Options::follow(interpolate, conditional),
        // Comments and whitespace are never written back, so dropping them
        // keeps the parse allocation-light.
        lossy: true,
        ignore_io_errors: false,
    };

    let metadata = list
        .files
        .iter()
        .map(|(path, source)| gix_config::file::Metadata::from(*source).at(path.clone()));
    let mut buf = Vec::with_capacity(4096);
    // `from_paths_metadata` itself delegates here with
    // `err_on_non_existing_paths = true`, which would make a merely absent
    // `~/.gitconfig` fatal. `false` skips it the way git does, and saves the
    // `stat` a pre-filter would need.
    gix_config::File::from_paths_metadata_buf(&mut metadata.into_iter(), &mut buf, false, options)
        .map_err(|source| ConfigError::Parse {
            path: list.files.first().map(|(p, _)| p.clone()).unwrap_or_default(),
            source: Box::new(source),
        })
}

fn interpolate_context(env: &Env) -> gix_config::path::interpolate::Context<'_> {
    gix_config::path::interpolate::Context {
        git_install_dir: None,
        home_dir: env.home.as_deref(),
        // The crate's own `~user/` resolver, the same hook its include
        // resolution uses (gix-config `file/includes/types.rs`). It lives in
        // gix-config-value and reaches us re-exported as
        // `gix_config::path::interpolate::home_for_user`.
        home_for_user: Some(gix_config::path::interpolate::home_for_user),
    }
}

/// `git config --path` semantics for one raw value.
///
/// Expands a leading `~/` and `~user/`; leaves everything else, including
/// relative paths, exactly as written. A value that cannot be interpolated
/// (`%(prefix)/…`, which needs a git installation directory scap does not
/// have) falls back to its raw spelling rather than failing the whole load.
fn interpolate_value(value: &BStr, env: &Env) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    let interpolated = gix_config::Path::from(std::borrow::Cow::Borrowed(value))
        .interpolate(interpolate_context(env))
        .unwrap_or_else(|_| PathBuf::from(value.to_str_lossy().into_owned()));
    Some(interpolated)
}

/// git's boolean truthiness, shared by both backends so they cannot drift.
///
/// `None` is a valueless key (`[scap] completeUser`), which git's `--bool`
/// prints as `true`. git accepts only `yes`/`on`/`true` and `no`/`off`/
/// `false`, case-insensitively, plus any integer (non-zero is true) and the
/// empty value (false). Anything else makes git exit with a fatal error;
/// scap cannot exit over a key it merely happens to read, so it takes the
/// conservative value instead -- registered as a divergence in the README.
pub(crate) fn git_boolean(value: Option<&BStr>) -> bool {
    let Some(value) = value else {
        return true;
    };
    if value.eq_ignore_ascii_case(b"yes")
        || value.eq_ignore_ascii_case(b"on")
        || value.eq_ignore_ascii_case(b"true")
    {
        return true;
    }
    if value.eq_ignore_ascii_case(b"no")
        || value.eq_ignore_ascii_case(b"off")
        || value.eq_ignore_ascii_case(b"false")
        || value.is_empty()
    {
        return false;
    }
    value.to_str().ok().and_then(parse_git_int).is_some_and(|int| int != 0)
}

/// git's `git_parse_int` as the boolean path uses it.
///
/// `strtoimax` with base 0 -- so a leading `0x` is hexadecimal, a leading
/// `0` is octal, and a sign is allowed -- followed by `get_unit_factor`,
/// which accepts an optional `k`/`m`/`g` suffix multiplying by 1024, 1024^2
/// or 1024^3 and rejects anything else. `0x1` and `1k` are therefore true
/// booleans to git, which a plain decimal parse would call invalid.
fn parse_git_int(text: &str) -> Option<i64> {
    // `strtoimax` skips leading whitespace but not trailing: a trailing
    // remainder that is not a unit suffix is an error.
    let body = text.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, body) = match body.as_bytes().first() {
        Some(b'-') => (true, &body[1..]),
        Some(b'+') => (false, &body[1..]),
        _ => (false, body),
    };

    let (radix, digits) =
        if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            (16, hex)
        } else if body.len() > 1 && body.starts_with('0') {
            (8, &body[1..])
        } else {
            (10, body)
        };

    let consumed = digits.find(|c: char| !c.is_digit(radix)).unwrap_or(digits.len());
    if consumed == 0 {
        return None;
    }
    let magnitude = i64::from_str_radix(&digits[..consumed], radix).ok()?;
    let factor: i64 = match &digits[consumed..] {
        "" => 1,
        "k" | "K" => 1024,
        "m" | "M" => 1024 * 1024,
        "g" | "G" => 1024 * 1024 * 1024,
        _ => return None,
    };

    let value = magnitude.checked_mul(factor)?;
    Some(if negative { -value } else { value })
}

/// The last plain `[<section>] <name>` boolean in `file`, read through
/// [`git_boolean`]. `None` when the key is absent everywhere.
///
/// Written against `value_implicit` rather than `File::boolean` because only
/// the former separates a valueless key from one set to the empty string,
/// and those are `true` and `false` respectively.
pub(crate) fn boolean_of(file: &gix_config::File, section: &str, name: &str) -> Option<bool> {
    let mut found = None;
    for candidate in file.sections() {
        let header = candidate.header();
        if !header.name().eq_ignore_ascii_case(section.as_bytes())
            || header.subsection_name().is_some()
        {
            continue;
        }
        if let Some(value) = candidate.value_implicit(name) {
            found = Some(git_boolean(value.as_ref().map(|v| v.as_bstr())));
        }
    }
    found
}

fn has_unevaluable_includeif(file: &gix_config::File) -> bool {
    file.sections().any(|section| {
        let header = section.header();
        header.name().eq_ignore_ascii_case(b"includeIf")
            && header.subsection_name().is_some_and(|condition| {
                let lower = condition.to_ascii_lowercase();
                lower.starts_with(b"onbranch:") || lower.starts_with(b"hasconfig:")
            })
    })
}

fn from_file(file: Option<&gix_config::File>, env: &Env) -> ConfigSnapshot {
    let mut roots = Vec::new();
    let mut url_scoped_roots = Vec::new();
    let mut user = None;
    let mut complete_user = false;
    let mut list_exclude = Vec::new();
    let mut list_cache = false;

    if let Some(file) = file {
        if let Some(values) = file.strings("scap.root") {
            roots.extend(values.iter().filter_map(|v| interpolate_value(v.as_ref(), env)));
        }
        // File order across every source, which is what
        // `git config --get-regexp '^scap\..+\.root$'` prints.
        for section in file.sections() {
            let header = section.header();
            if !header.name().eq_ignore_ascii_case(b"scap") || header.subsection_name().is_none() {
                continue;
            }
            url_scoped_roots.extend(
                section.values("root").iter().filter_map(|v| interpolate_value(v.as_ref(), env)),
            );
        }
        user = file.string("scap.user").map(|v| v.to_str_lossy().trim().to_owned());
        complete_user = boolean_of(file, "scap", "completeUser").unwrap_or(false);
        if let Some(values) = file.strings("scap.listExclude") {
            list_exclude.extend(values.iter().map(|v| v.to_str_lossy().into_owned()));
        }
        list_cache = boolean_of(file, "scap", "listCache").unwrap_or(false);
    }

    let reason = if url_scoped_roots.is_empty() { Reason::InProcess } else { Reason::UrlSections };
    ConfigSnapshot {
        env: env.clone(),
        roots,
        url_scoped_roots,
        user,
        complete_user,
        list_exclude,
        list_cache,
        backend: Backend::InProcess,
        reason,
        urlmatch: Default::default(),
    }
}

// ghq local_repository.go:355-395
pub fn resolve_roots(all: bool) -> Result<Vec<PathBuf>, ConfigError> {
    snapshot().resolve_roots(all)
}

// ghq local_repository.go:123-142, ADR-8 rules (a)-(e).
pub fn root_for_url(url: &str) -> Result<PathBuf, ConfigError> {
    snapshot().root_for_url(url)
}

impl ConfigSnapshot {
    // ghq local_repository.go:355-395
    fn resolve_roots(&self, all: bool) -> Result<Vec<PathBuf>, ConfigError> {
        let snapshot = self;
        let env_roots = snapshot.env_roots();

        let mut roots: Vec<PathBuf> = match &env_roots {
            Some(list) => list.clone(),
            None => {
                let mut from_config = snapshot.roots.clone();
                from_config.reverse();
                from_config
            }
        };

        if roots.is_empty() {
            let home = snapshot.env.home.clone().ok_or(ConfigError::NoHomeDir)?;
            roots.push(home.join("scap"));
        }

        if all && env_roots.is_none() {
            roots.extend(snapshot.url_scoped_roots.iter().cloned());
        }

        let mut seen = std::collections::HashSet::new();
        let mut deduped = Vec::with_capacity(roots.len());
        for root in roots {
            let cleaned = clean_path(&root);
            let resolved = std::fs::canonicalize(&cleaned).unwrap_or(cleaned);
            if seen.insert(resolved.clone()) {
                deduped.push(resolved);
            }
        }

        Ok(deduped)
    }

    // ghq local_repository.go:123-142, ADR-8 rules (a)-(e).
    fn root_for_url(&self, url: &str) -> Result<PathBuf, ConfigError> {
        let snapshot = self;

        // (a) `SCAP_ROOT` wins outright.
        if let Some(mut list) = snapshot.env_roots()
            && !list.is_empty()
        {
            return Ok(clean_path(&list.remove(0)));
        }

        // (b) A codecommit target skips urlmatch entirely, and ghq then
        // takes its primary root -- `primaryLocalRepositoryRoot()`, which
        // cleans, resolves symlinks and absolutises -- rather than the raw
        // plain key rule (c) reproduces (local_repository.go:123-142).
        //
        // Every caller passes the *normalised* remote, and for a codecommit
        // target that is `codecommit://<region>/<owner>/<name>` from
        // `url::finalize_codecommit` -- a spelling `is_codecommit_input`
        // rejects, because its authority class excludes `/`. Keying on the
        // scheme is what makes this rule reachable at all; ghq reaches the
        // same place by substituting `remoteURL.Opaque` before `getRoot`.
        // The raw spelling is accepted too, so a caller that has not
        // normalised yet behaves the same.
        if url.starts_with("codecommit://") || crate::url::is_codecommit_input(url) {
            return snapshot.resolve_roots(false)?.into_iter().next().ok_or(ConfigError::NoHomeDir);
        }

        // (d) Delegate, but only when a url-scoped section can actually
        // match.
        if snapshot.has_url_sections()
            && let Some(value) = snapshot.urlmatch(url)?
        {
            return Ok(clean_path(&value));
        }

        // (c) With no url-scoped section visible, `git config --path
        // --get-urlmatch` prints the LAST plain `scap.root`, raw -- through a
        // symlinked component, without `realpath` -- and ghq uses it that way.
        // Routing through `resolve_roots(false)` would canonicalise it.
        if let Some(last) = snapshot.roots.last() {
            return Ok(clean_path(last));
        }

        // (e) No `scap.root` anywhere: git exits 1 and ghq falls back to its
        // primary root, which is canonicalised.
        snapshot.resolve_roots(false)?.into_iter().next().ok_or(ConfigError::NoHomeDir)
    }

    /// `SCAP_ROOT` split into entries, or `None` when it is unset or empty.
    fn env_roots(&self) -> Option<Vec<PathBuf>> {
        let value = self.env.scap_root.as_ref()?;
        if value.is_empty() {
            return None;
        }
        Some(std::env::split_paths(value).collect())
    }

    /// ADR-8 rule (d): `git config --path --get-urlmatch scap.root <url>`,
    /// memoised per distinct URL for the life of the process.
    ///
    /// This delegation is the only `git` spawn left anywhere on the
    /// configuration path, and only a user with url-scoped `[scap "<url>"]`
    /// sections reaches it at all. Memoising it means `get --parallel` pays
    /// at most one spawn per *distinct* URL rather than one per target --
    /// against ghq's three per target -- and a repeated target pays none.
    ///
    /// An error is deliberately not memoised: every one of them is fatal to
    /// the caller (ADR-8 fails fast rather than degrading), so there is no
    /// second call to answer, and caching a transient `git` failure would
    /// only make a later diagnosis harder.
    fn urlmatch(&self, url: &str) -> Result<UrlmatchAnswer, ConfigError> {
        let span = tracing::debug_span!(
            "scap::config::urlmatch",
            url,
            spawned = tracing::field::Empty,
            urlmatch_spawns = tracing::field::Empty,
        );
        let _entered = span.enter();

        let cell = {
            let mut cells = lock(&self.urlmatch.cells);
            Arc::clone(cells.entry(url.to_owned()).or_default())
        };
        let mut slot = lock(&cell);

        if let Some(answer) = slot.as_ref() {
            span.record("spawned", false);
            span.record("urlmatch_spawns", self.urlmatch.spawns.load(Ordering::Relaxed));
            return Ok(answer.clone());
        }

        let answer = self.urlmatch_spawn(url)?;
        span.record("spawned", true);
        span.record("urlmatch_spawns", self.urlmatch.spawns.load(Ordering::Relaxed));
        *slot = Some(answer.clone());
        Ok(answer)
    }

    /// Run one `git config --path --get-urlmatch` and count it.
    fn urlmatch_spawn(&self, url: &str) -> Result<UrlmatchAnswer, ConfigError> {
        // Resolved here rather than when the snapshot is built: walking
        // `PATH` costs one `stat` per entry, and the default path -- no
        // trigger, no url-scoped section -- never spawns anything.
        let program = sources::resolve_git_program(&self.env).ok_or(ConfigError::GitRequired {
            reason: "url-scoped [scap \"<url>\"] sections require `git config --get-urlmatch`",
        })?;
        let mut command = std::process::Command::new(program);
        command.args(["config", "--path", "--get-urlmatch", "scap.root", url]);
        git_backend::apply_env(&mut command, &self.env);
        let output = command.output()?;
        // Counted once the process has actually run and been reaped: an
        // `exec` that never got off the ground is not a spawn.
        self.urlmatch.spawns.fetch_add(1, Ordering::Relaxed);
        match output.status.code() {
            Some(0) => {
                let stdout =
                    String::from_utf8(output.stdout).map_err(|_| ConfigError::InvalidUtf8)?;
                let trimmed = stdout.trim();
                Ok((!trimmed.is_empty()).then(|| PathBuf::from(trimmed)))
            }
            Some(1) => Ok(None),
            Some(status) => Err(ConfigError::GitConfigFailed {
                key: "scap.root".to_owned(),
                status,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
            None => Err(ConfigError::GitConfigFailed {
                key: "scap.root".to_owned(),
                status: -1,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
        }
    }
}

/// `scap.user`.
pub fn user() -> Result<Option<String>, ConfigError> {
    Ok(snapshot().user.clone())
}

/// `scap.completeUser`.
pub fn complete_user() -> Result<bool, ConfigError> {
    Ok(snapshot().complete_user)
}

fn clean_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from(".") } else { out }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
