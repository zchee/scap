//! The A3 backend: one `git config --list -z --show-origin` spawn.
//!
//! ADR-8 routes the *whole* snapshot here whenever a spawn trigger fires, so
//! git stays the parser of record for every configuration scap cannot
//! reproduce exactly in process. It is also what `SCAP_CONFIG_BACKEND=git`
//! selects unconditionally (R8's escape hatch).

use std::process::Command;

use bstr::{BStr, ByteSlice};

use super::{
    Backend, ConfigError, ConfigSnapshot, Env, Reason, effective_list_exclude, git_boolean,
    interpolate_value, sources,
};

/// Load the whole snapshot through `git`.
pub(super) fn load(env: &Env, reason: Reason) -> Result<ConfigSnapshot, ConfigError> {
    let program = sources::resolve_git_program(env)
        .ok_or(ConfigError::GitRequired { reason: reason_text(reason) })?;

    let mut command = Command::new(&program);
    command.args(["config", "--list", "-z", "--show-origin"]);
    apply_env(&mut command, env);

    let output = command.output()?;
    let listing = match output.status.code() {
        Some(0) => output.stdout,
        // `git config --list` exits 1 when nothing at all is configured.
        Some(1) => Vec::new(),
        Some(status) => {
            return Err(ConfigError::GitConfigFailed {
                key: "--list".to_owned(),
                status,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        None => {
            return Err(ConfigError::GitConfigFailed {
                key: "--list".to_owned(),
                status: -1,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
    };

    Ok(from_listing(&listing, env, reason))
}

/// Mirror the [`Env`] view onto a child `git`.
///
/// Everything else is inherited, which is what keeps `GIT_CONFIG_KEY_<n>` /
/// `GIT_CONFIG_VALUE_<n>` working: `Env` carries only `GIT_CONFIG_COUNT`, and
/// the numbered pairs it refers to come through untouched.
pub(super) fn apply_env(command: &mut Command, env: &Env) {
    set_or_remove(command, "HOME", env.home.as_ref().map(AsRef::as_ref));
    set_or_remove(command, "XDG_CONFIG_HOME", env.xdg_config_home.as_ref().map(AsRef::as_ref));
    set_or_remove(command, "GIT_CONFIG_GLOBAL", env.git_config_global.as_ref().map(AsRef::as_ref));
    set_or_remove(command, "GIT_CONFIG_SYSTEM", env.git_config_system.as_ref().map(AsRef::as_ref));
    set_or_remove(command, "GIT_CONFIG_NOSYSTEM", env.git_config_nosystem.as_deref());
    set_or_remove(command, "GIT_CONFIG_COUNT", env.git_config_count.as_deref());
    set_or_remove(command, "GIT_CONFIG_PARAMETERS", env.git_config_parameters.as_deref());
    set_or_remove(command, "GIT_DIR", env.git_dir.as_ref().map(AsRef::as_ref));
    set_or_remove(command, "GIT_CEILING_DIRECTORIES", env.git_ceiling_directories.as_deref());
    if let Some(path) = &env.path {
        command.env("PATH", path);
    }
    if let Some(cwd) = &env.cwd {
        command.current_dir(cwd);
    }
}

fn set_or_remove(command: &mut Command, key: &str, value: Option<&std::ffi::OsStr>) {
    match value {
        Some(value) => command.env(key, value),
        None => command.env_remove(key),
    };
}

/// Build a snapshot from one `git config --list -z --show-origin` listing.
///
/// The `-z` framing is one NUL-terminated field per record, alternating
/// origin and `key\nvalue`; a valueless key (`[scap] completeUser`) has no
/// newline at all, which is exactly how git spells `--bool` true.
fn from_listing(listing: &[u8], env: &Env, reason: Reason) -> ConfigSnapshot {
    let mut roots = Vec::new();
    let mut url_scoped_roots = Vec::new();
    let mut user = None;
    let mut complete_user = false;
    let mut list_exclude = Vec::new();
    let mut list_cache = false;

    for (key, value) in entries(listing) {
        // git lowercases section and variable names in `--list` output and
        // leaves subsection names verbatim, so the prefix test is exact.
        let Some(rest) = key.strip_prefix("scap.") else {
            continue;
        };
        match rest.rsplit_once('.') {
            None => match rest {
                "root" => roots.extend(value.and_then(|v| interpolate_value(v, env))),
                "user" => {
                    user =
                        Some(value.map(|v| v.to_str_lossy().trim().to_owned()).unwrap_or_default())
                }
                "completeuser" => complete_user = git_boolean(value),
                "listexclude" => {
                    list_exclude.extend(value.map(|v| v.to_str_lossy().into_owned()));
                }
                "listcache" => list_cache = git_boolean(value),
                _ => {}
            },
            Some((_subsection, "root")) => {
                url_scoped_roots.extend(value.and_then(|v| interpolate_value(v, env)));
            }
            Some(_) => {}
        }
    }

    ConfigSnapshot {
        env: env.clone(),
        roots,
        url_scoped_roots,
        user,
        complete_user,
        list_exclude: effective_list_exclude(env, list_exclude),
        list_cache,
        backend: Backend::Git,
        reason,
        // ADR-8 rule (d) is one shared code path, so the A3 snapshot gets
        // the same per-URL `--get-urlmatch` memo the in-process one has.
        urlmatch: Default::default(),
    }
}

/// Split a `-z --show-origin` listing into `(key, value)` pairs.
fn entries(listing: &[u8]) -> Vec<(String, Option<&BStr>)> {
    let mut out = Vec::new();
    let mut fields = listing.split(|byte| *byte == 0);
    // The origin field and the `key\nvalue` field alternate; a trailing
    // empty field after the final NUL is simply never paired.
    while let (Some(_origin), Some(record)) = (fields.next(), fields.next()) {
        if record.is_empty() {
            continue;
        }
        let (key, value) = match record.iter().position(|byte| *byte == b'\n') {
            Some(at) => (&record[..at], Some(record[at + 1..].as_bstr())),
            None => (record, None),
        };
        let Ok(key) = std::str::from_utf8(key) else {
            continue;
        };
        out.push((key.to_owned(), value));
    }
    out
}

fn reason_text(reason: Reason) -> &'static str {
    match reason {
        Reason::EnvOverride => "SCAP_CONFIG_BACKEND=git",
        Reason::GitConfigCount => "GIT_CONFIG_COUNT is set",
        Reason::GitConfigParameters => "GIT_CONFIG_PARAMETERS is set",
        Reason::SystemProbeAmbiguous => "the system gitconfig probe matched more than one file",
        Reason::IncludeifUnevaluated => {
            "an includeIf onbranch:/hasconfig: condition needs git to evaluate it"
        }
        Reason::InProcess | Reason::UrlSections => "the in-process snapshot was declined",
    }
}

#[cfg(test)]
#[path = "git_backend_tests.rs"]
mod tests;
