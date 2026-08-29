//! Shared fixtures for the integration tests (plan §5, "Test support").
//!
//! Declared as `mod support;` by every integration test file that needs it.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// A recording `git` — a real `git`, not a mock.
///
/// Writes an executable shell script named `git` into a temporary directory,
/// appends every invocation's arguments to a log file as one line, and then
/// `exec`s the real `git` so the command actually runs. Prepending
/// [`RecordingGit::path_prepend`] to a child's `PATH` and exporting
/// [`RecordingGit::env`] turns "scap must not spawn `git config`" from an
/// unobservable claim into an assertion over [`RecordingGit::lines`].
pub struct RecordingGit {
    _tmp: TempDir,
    bin_dir: PathBuf,
    log: PathBuf,
    real_git: PathBuf,
}

impl RecordingGit {
    /// Create the wrapper. Panics if no real `git` can be found on `PATH`,
    /// since every test that wants a recording wrapper needs one to delegate
    /// to.
    pub fn new() -> Self {
        let real_git = which_git().expect(
            "no `git` on PATH: the recording wrapper delegates to the real binary and cannot \
             substitute for it",
        );
        let tmp = TempDir::new().expect("tempdir for the recording git wrapper");
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("mkdir wrapper bin dir");
        let log = tmp.path().join("git.log");
        fs::File::create(&log).expect("create the wrapper log");

        let script = bin_dir.join("git");
        // `printf '%s ' "$@"` would add a trailing space and mangle empty
        // arguments; `"$*"` joins on the first character of IFS, which is a
        // space here, and writes exactly one line per invocation.
        let mut f = fs::File::create(&script).expect("create the wrapper script");
        f.write_all(
            b"#!/bin/bash\n\
              printf '%s\\n' \"$*\" >> \"$SCAP_TEST_GIT_LOG\"\n\
              exec \"$SCAP_TEST_REAL_GIT\" \"$@\"\n",
        )
        .expect("write the wrapper script");
        drop(f);
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("chmod +x the wrapper script");

        Self { _tmp: tmp, bin_dir, log, real_git }
    }

    /// The current `PATH` with this wrapper's directory prepended, ready for
    /// `Command::env("PATH", …)`.
    pub fn path_prepend(&self) -> OsString {
        let mut out = OsString::from(&self.bin_dir);
        if let Some(existing) = std::env::var_os("PATH")
            && !existing.is_empty()
        {
            out.push(":");
            out.push(existing);
        }
        out
    }

    /// The two variables the wrapper script reads: where to append, and which
    /// binary to hand the invocation on to.
    pub fn env(&self) -> Vec<(String, OsString)> {
        vec![
            ("SCAP_TEST_GIT_LOG".to_owned(), self.log.clone().into_os_string()),
            ("SCAP_TEST_REAL_GIT".to_owned(), self.real_git.clone().into_os_string()),
        ]
    }

    /// Every recorded invocation, in order, one entry per `git` call.
    pub fn lines(&self) -> Vec<String> {
        let contents = fs::read_to_string(&self.log).expect("read the wrapper log");
        contents.lines().map(str::to_owned).collect()
    }

    /// The real `git` this wrapper delegates to.
    pub fn real_git(&self) -> &Path {
        &self.real_git
    }
}

/// Resolve `git` by walking `PATH` the way a shell does: the first entry
/// holding an executable regular file named `git` wins.
fn which_git() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("git");
        let Ok(meta) = fs::metadata(&candidate) else {
            continue;
        };
        if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

/// An empty directory to use as an entire `PATH`, for the plan's empty-PATH
/// probe: with this as `PATH`, no `git` can be found, so any command that
/// still succeeds proves it never needed to spawn one.
pub fn empty_path_dir() -> TempDir {
    TempDir::new().expect("tempdir for the empty-PATH probe")
}
