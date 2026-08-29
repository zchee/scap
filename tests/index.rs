//! ADR-10's mutation matrix: every change the index is required to notice,
//! checked against a fresh walk of the same tree.
//!
//! Each row is the same experiment. Build a tree, settle it (see [`settle`]),
//! run `scap list --cache` twice so the second run is answered from the index
//! rather than from a walk, mutate the tree, then require `--cache` and
//! `--no-cache` to print the same bytes. `--no-cache` never reads or writes
//! the index, so it is a clean oracle no matter where in a row it runs.
//!
//! The one row that is expected to *fail* — a mutation whose directory
//! timestamps are then rewound with `touch -t` — asserts the failure and
//! asserts that `scap list --cache-check` exits 1 and says so, which is the
//! whole reason that flag exists.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::{fs, io};

use assert_cmd::Command;
use tempfile::TempDir;

/// The timestamp every fixture directory is rewound to.
///
/// 2021-09-13 12:00, in `touch -t` spelling. Any instant far enough in the
/// past would do; what matters is that it is outside the 2-second racy window
/// around the index's own write time, because a tree younger than that window
/// is re-walked entirely and a row would then pass whether or not the index
/// was ever consulted.
const SETTLED: &str = "202109131200.00";

struct Fixture {
    home: TempDir,
    root: TempDir,
    cache: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let home = TempDir::new().expect("home");
        fs::File::create(home.path().join("gitconfig")).expect("gitconfig");
        Self { home, root: TempDir::new().expect("root"), cache: TempDir::new().expect("cache") }
    }

    fn root(&self) -> &Path {
        self.root.path()
    }

    /// A `scap` invocation isolated from the developer's own configuration
    /// and from any ambient walker or cache setting.
    ///
    /// `XDG_CACHE_HOME` points at the fixture's own directory, so a row can
    /// never read or write the real `~/.cache/scap`.
    fn cmd(&self) -> Command {
        let mut cmd = Command::cargo_bin("scap").expect("scap binary");
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.path().join("gitconfig"))
            .env("SCAP_ROOT", self.root.path())
            .env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.cache.path())
            .env_remove("SCAP_CONFIG_BACKEND")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_CEILING_DIRECTORIES")
            .env_remove("SCAP_LIST_EXCLUDE")
            .env_remove("SCAP_LIST_CACHE")
            .env_remove("SCAP_LIST_DETECT")
            .env_remove("SCAP_LIST_THREADS")
            .env_remove("SCAP_LOG")
            .env_remove("RUST_LOG")
            .current_dir(self.home.path());
        cmd
    }

    /// `scap list` with these arguments, required to succeed.
    fn list(&self, args: &[&str]) -> String {
        let assert = self.cmd().arg("list").args(args).assert().success();
        String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 listing")
    }

    /// The listing the index produces, and the listing a walk produces.
    ///
    /// The cached form runs first so the comparison is made against an index
    /// that has just been consulted and rewritten, which is the state a real
    /// second run is in.
    fn agree(&self, label: &str) {
        let cached = self.list(&["--cache"]);
        let fresh = self.list(&["--no-cache"]);
        assert_eq!(cached, fresh, "{label}: the index and a fresh walk must print the same bytes");
    }

    /// Warms the index: one run to build it, one to validate against it.
    fn warm(&self) {
        self.list(&["--cache"]);
        self.list(&["--cache"]);
    }

    /// One counter off the `scap::index` span of a cached run.
    ///
    /// The span carries `hit`, `miss`, `racy`, `invalidated` and `entries`,
    /// and reading them is what distinguishes a row that exercised the index
    /// from one that silently fell back to a walk and compared two walks with
    /// each other. The name is matched with its leading space, so no field can
    /// be found inside another.
    fn index_field(&self, field: &str) -> u64 {
        let assert =
            self.cmd().env("SCAP_LOG", "debug").args(["list", "--cache"]).assert().success();
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
        let needle = format!(" {field}=");
        let found = stderr
            .split("scap::index{")
            .nth(1)
            .and_then(|rest| rest.split_once(needle.as_str()))
            .map(|(_, tail)| tail);
        let Some(tail) = found else {
            panic!("no scap::index span carrying `{field}` in:\n{stderr}");
        };
        tail.split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|digits| digits.parse().ok())
            .unwrap_or_else(|| panic!("unparsable `{field}` in:\n{stderr}"))
    }

    /// The `hit` count, the counter most rows care about.
    fn index_hits(&self) -> u64 {
        self.index_field("hit")
    }

    fn index_file(&self) -> PathBuf {
        let dir = self.cache.path().join("scap");
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap_or_else(|err| panic!("{}: {err}", dir.display()))
            .map(|entry| entry.expect("cache entry").path())
            .collect();
        assert_eq!(files.len(), 1, "one root, one index file: {files:?}");
        files.pop().expect("the index file")
    }
}

/// Creates `<root>/<rel>` as a real git repository.
fn repo(root: &Path, rel: &str) {
    let dest = root.join(rel);
    fs::create_dir_all(&dest).expect("repository fixture");
    let out = StdCommand::new("git")
        .args(["init", "-q"])
        .current_dir(&dest)
        .output()
        .expect("run git init");
    assert!(out.status.success(), "git init failed: {out:?}");
}

/// Creates `<root>/<rel>` as a bare repository, whose directory name ends in
/// `.git` and which ADR-9 rule (ii) therefore never opens.
fn bare(root: &Path, rel: &str) {
    let dest = root.join(rel);
    fs::create_dir_all(&dest).expect("bare repository fixture");
    let out = StdCommand::new("git")
        .args(["init", "-q", "--bare"])
        .current_dir(&dest)
        .output()
        .expect("run git init --bare");
    assert!(out.status.success(), "git init --bare failed: {out:?}");
}

fn dir(root: &Path, rel: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(&path).expect("directory fixture");
    path
}

/// Every directory at or below `path`, deepest last.
fn directories(path: &Path) -> Vec<PathBuf> {
    let mut found = vec![path.to_path_buf()];
    let mut queue = vec![path.to_path_buf()];
    while let Some(next) = queue.pop() {
        let Ok(entries) = fs::read_dir(&next) else { continue };
        for entry in entries.flatten() {
            // `file_type` does not follow links, so a symlink to a directory
            // is not descended — the walk does not descend one either, and
            // rewinding the target's timestamps would reach outside the tree.
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                found.push(entry.path());
                queue.push(entry.path());
            }
        }
    }
    found
}

/// Rewinds every directory in the tree to [`SETTLED`] with `touch -t`.
///
/// Every row needs this before its first cached run. A tree built moments ago
/// is entirely inside the index's 2-second racy window, so every entry is
/// re-walked and the row would compare one walk against another and pass no
/// matter what the index did. Rewinding puts the fixture in the state a real
/// corpus is in almost always — settled, and older than the last index write.
///
/// `touch -t` is also the mutation the index is documented as unable to see,
/// which is why the same tool is used for both: the documented-failure row
/// below is this function applied a second time, after a change.
fn settle(path: &Path) {
    for chunk in directories(path).chunks(200) {
        let out = StdCommand::new("touch")
            .arg("-t")
            .arg(SETTLED)
            .args(chunk)
            .output()
            .expect("run touch -t");
        assert!(out.status.success(), "touch -t failed: {out:?}");
    }
}

/// Copies `from` onto `to` preserving modification times, the way an archive
/// restore does.
fn copy_preserving_times(from: &Path, to: &Path) {
    let out = StdCommand::new("cp")
        .args(["-Rp".as_ref(), from.as_os_str(), to.as_os_str()])
        .output()
        .expect("run cp -Rp");
    assert!(out.status.success(), "cp -Rp failed: {out:?}");
}

// ---------------------------------------------------------------------------
// The index is actually consulted
// ---------------------------------------------------------------------------

#[test]
fn a_warm_run_is_answered_from_the_index_and_not_from_a_walk() {
    // Every other row in this file is only meaningful if this one holds: a
    // row that compares two walks with each other passes whatever the index
    // does.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    repo(f.root(), "github.com/b/y");
    dir(f.root(), "github.com/c/plain");
    settle(f.root());

    f.warm();
    assert!(
        f.index_hits() > 0,
        "a settled tree must validate against the index instead of being re-walked"
    );
    f.agree("a settled tree");
}

// ---------------------------------------------------------------------------
// Mutations the index is required to see
// ---------------------------------------------------------------------------

#[test]
fn a_repository_added_removed_or_renamed_is_seen() {
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    repo(f.root(), "github.com/a/y");
    settle(f.root());
    f.warm();
    f.agree("before any mutation");

    repo(f.root(), "github.com/b/z");
    f.agree("a repository added");

    fs::rename(f.root().join("github.com/a/y"), f.root().join("github.com/a/renamed"))
        .expect("rename");
    f.agree("a repository renamed");

    fs::remove_dir_all(f.root().join("github.com/a/x")).expect("remove");
    f.agree("a repository removed");

    assert_eq!(f.list(&["--no-cache"]), "github.com/a/renamed\ngithub.com/b/z\n");
}

#[test]
fn git_init_inside_a_directory_the_index_already_knew_is_seen() {
    // The hardest of the additive cases: the directory is already an index
    // entry, its parent's mtime does not move, and only the directory's own
    // mtime records that `.git` appeared inside it.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    dir(f.root(), "github.com/b/plain");
    settle(f.root());
    f.warm();
    assert_eq!(f.list(&["--cache"]), "github.com/a/x\n");

    repo(f.root(), "github.com/b/plain");
    f.agree("git init inside a known directory");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x\ngithub.com/b/plain\n");
}

#[test]
fn removing_dot_git_from_a_repository_is_seen() {
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    repo(f.root(), "github.com/a/x-keep");
    settle(f.root());
    f.warm();

    fs::remove_dir_all(f.root().join("github.com/a/x/.git")).expect("rm -rf .git");
    f.agree("a repository that lost its .git");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x-keep\n");
}

#[test]
fn a_directory_swapped_for_a_symlink_and_back_is_seen() {
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    let elsewhere = TempDir::new().expect("elsewhere");
    repo(elsewhere.path(), "hidden");
    settle(f.root());
    f.warm();
    assert_eq!(f.list(&["--cache"]), "github.com/a/x\n");

    // A symlink is never descended, so the repositories under the target must
    // not appear — the validation `stat` is `NOFOLLOW` for exactly this.
    let swapped = f.root().join("github.com/a");
    fs::remove_dir_all(&swapped).expect("remove the directory");
    std::os::unix::fs::symlink(elsewhere.path(), &swapped).expect("symlink in its place");
    f.agree("a directory replaced by a symlink");
    assert_eq!(f.list(&["--no-cache"]), "", "nothing is listed through a link");

    fs::remove_file(&swapped).expect("remove the symlink");
    repo(f.root(), "github.com/a/x");
    f.agree("and swapped back to a real directory");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x\n");
}

#[test]
fn a_bare_repository_added_and_removed_is_seen() {
    // Rule (ii) entries are never probed — a name ending in `.git` is a
    // repository by its name alone — so this row is entirely carried by the
    // parent directory's mtime.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    dir(f.root(), "store");
    settle(f.root());
    f.warm();

    bare(f.root(), "store/upstream.git");
    f.agree("a bare repository added");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x\nstore/upstream.git\n");

    fs::remove_dir_all(f.root().join("store/upstream.git")).expect("remove the bare repository");
    f.agree("a bare repository removed");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x\n");
}

#[test]
fn a_tree_imported_with_cp_p_is_seen() {
    // `cp -p` carries the source's modification times onto the copy, so the
    // imported directories are *older* than the index that does not know
    // them. What saves the listing is the mtime of the directory they were
    // copied into, which moves when the new name appears.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    dir(f.root(), "github.com/imported");
    settle(f.root());
    f.warm();

    let source = TempDir::new().expect("source");
    repo(source.path(), "brought-in");
    settle(source.path());
    copy_preserving_times(&source.path().join("brought-in"), &f.root().join("github.com/imported"));

    f.agree("a tree imported with cp -p");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x\ngithub.com/imported/brought-in\n");
}

#[test]
fn a_tree_restored_over_itself_is_seen() {
    // §4 S1's restore scenario. A backup written back over the live tree —
    // `rsync -a --delete`, or `cp -Rp` where rsync is not installed — carries
    // the *source's* modification times, so the restored directories look
    // untouched. The restore replaces a directory the index knew, and the
    // parent's mtime moves when it does, which is what keeps the listing
    // correct.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    repo(f.root(), "github.com/a/y");
    settle(f.root());

    let backup = TempDir::new().expect("backup");
    copy_preserving_times(&f.root().join("github.com"), backup.path());
    f.warm();

    // Diverge from the backup, then restore it.
    fs::remove_dir_all(f.root().join("github.com/a/y")).expect("remove");
    repo(f.root(), "github.com/a/added-after-the-backup");
    f.agree("after diverging from the backup");

    fs::remove_dir_all(f.root().join("github.com")).expect("clear the tree");
    restore(&backup.path().join("github.com"), &f.root().join("github.com"));
    f.agree("after restoring the backup over it");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x\ngithub.com/a/y\n");
}

/// Restores `from` to `to` with `rsync -a --delete`, falling back to `cp -Rp`
/// where rsync is not installed.
///
/// Both preserve modification times, which is the property §4 S1's restore
/// scenario turns on; rsync is preferred because it is what the plan names,
/// and the fallback keeps the row meaningful on a machine without it rather
/// than skipping it.
fn restore(from: &Path, to: &Path) {
    let mut source = from.as_os_str().to_os_string();
    source.push("/");
    let rsync = StdCommand::new("rsync").arg("-a").arg("--delete").arg(&source).arg(to).output();
    match rsync {
        Ok(out) => assert!(out.status.success(), "rsync -a --delete failed: {out:?}"),
        Err(err) if err.kind() == io::ErrorKind::NotFound => copy_preserving_times(from, to),
        Err(err) => panic!("running rsync: {err}"),
    }
}

#[test]
fn a_repository_six_levels_below_the_root_is_seen() {
    // Validation is top-down: a child entry may only be believed because its
    // parent validated. Six levels is enough for a break anywhere in that
    // chain to show up as a missing repository.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    dir(f.root(), "one/two/three/four/five");
    settle(f.root());
    f.warm();

    repo(f.root(), "one/two/three/four/five/six");
    f.agree("a repository six levels down");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x\none/two/three/four/five/six\n");

    fs::remove_dir_all(f.root().join("one/two/three/four/five/six")).expect("remove");
    f.agree("and removed again");
}

#[test]
fn a_mutation_inside_the_racy_window_is_seen() {
    // No settling here, deliberately: the tree is built and mutated within the
    // 2-second window around the index's own write time. An entry that recent
    // is treated as changed however its mtime reads, which is git's racy rule
    // and the reason a filesystem storing whole seconds cannot make this index
    // miss a change made around the write.
    //
    // Asserting only that the listing is right would not test the rule. The
    // directory a new repository appears in has a *different* mtime, so it is
    // re-walked as an ordinary miss whether or not the racy rule exists. What
    // the rule catches is the other case -- an entry whose mtime still matches
    // what was recorded, but was set too recently for that match to be
    // evidence -- so this row reads the `racy` counter off the span and
    // requires it to have fired. The `touch` immediately before the recording
    // run is what makes that deterministic rather than a race with the test's
    // own speed: the mtime it sets and the write time of the run that records
    // it are adjacent by construction, so the entry sits inside the window
    // however long the rest of the test then takes. The window's arithmetic is
    // pinned at its exact boundary in the unit tests; this is the end-to-end
    // proof that the rule is wired in at all.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    f.warm();
    repo(f.root(), "github.com/a/y");

    f.agree("a mutation inside the racy window");
    assert_eq!(f.list(&["--no-cache"]), "github.com/a/x\ngithub.com/a/y\n");

    let out =
        StdCommand::new("touch").arg(f.root().join("github.com")).output().expect("run touch");
    assert!(out.status.success(), "touch failed: {out:?}");
    f.list(&["--cache"]);
    assert!(
        f.index_field("racy") > 0,
        "an entry whose mtime matches but was set moments before the index was written"
    );
    f.agree("with the racy rule firing");
}

// ---------------------------------------------------------------------------
// The mutation the index cannot see
// ---------------------------------------------------------------------------

#[test]
fn a_backdated_mutation_is_missed_and_cache_check_reports_it() {
    // DOCUMENTED FAILURE. A repository is added and every directory's
    // timestamp is then rewound to the value the index recorded, so nothing
    // on disk says the tree changed. This is §4 S1's residue — "mtime equal,
    // contents different" — and it is why the index is opt-in, why
    // `--no-cache` exists, and why `--cache-check` exists.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    settle(f.root());
    f.warm();
    assert_eq!(f.list(&["--cache"]), "github.com/a/x\n");

    repo(f.root(), "github.com/a/y");
    settle(f.root());

    assert_eq!(
        f.list(&["--cache"]),
        "github.com/a/x\n",
        "a rewound timestamp is a lie the validation pass has no way to catch"
    );
    assert_eq!(
        f.list(&["--no-cache"]),
        "github.com/a/x\ngithub.com/a/y\n",
        "which the walk, and so --no-cache, still sees"
    );

    // `--cache-check` is the whole answer: it walks afresh, prints the
    // difference on stderr, exits 1, and still prints the correct listing on
    // stdout so a pipeline is not left with the stale answer.
    let assert = f.cmd().args(["list", "--cache-check"]).assert().code(1);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    assert_eq!(stdout, "github.com/a/x\ngithub.com/a/y\n", "stdout is the fresh walk");
    assert!(stderr.contains("github.com/a/y"), "the difference is named on stderr: {stderr}");
    assert!(stderr.contains("disagrees with a fresh walk"), "and it says what happened: {stderr}");

    // The check rewrites the index from its own walk, so it leaves the tree
    // in a state where the next check passes.
    f.cmd().args(["list", "--cache-check"]).assert().success();
}

#[test]
fn cache_check_exits_zero_on_a_clean_tree() {
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    repo(f.root(), "store/upstream.git");
    settle(f.root());

    // With no index at all there is nothing to disagree with.
    f.cmd().args(["list", "--cache-check"]).assert().success();
    f.warm();
    let assert = f.cmd().args(["list", "--cache-check"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    assert_eq!(stdout, f.list(&["--no-cache"]));

    // `--cache-check` runs whether or not the index is enabled -- the
    // environment form does not change the verdict.
    f.cmd().env("SCAP_LIST_CACHE", "1").args(["list", "--cache-check"]).assert().success();
}

#[test]
fn cache_check_and_no_cache_are_a_usage_error_rather_than_a_silent_order() {
    // Asking to compare against an index the same command line forbids reading
    // has no honest answer, and either silent precedence would give whoever
    // typed both something neither flag's help text describes.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    settle(f.root());

    let assert = f.cmd().args(["list", "--cache-check", "--no-cache"]).assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    assert!(stderr.contains("--no-cache"), "the message names the offending flag: {stderr}");
    assert_eq!(
        assert.get_output().status.code(),
        Some(2),
        "clap's usage-error status, not the check's exit 1"
    );
    assert!(assert.get_output().stdout.is_empty(), "and no listing is printed");

    // Each on its own is still fine.
    f.cmd().args(["list", "--cache-check"]).assert().success();
    f.cmd().args(["list", "--no-cache"]).assert().success();
}

#[test]
fn a_directory_made_unreadable_after_the_index_was_built_is_the_other_blind_spot() {
    // DOCUMENTED FAILURE, the companion to the backdated mutation above.
    // `chmod` moves a directory's ctime and never its mtime, so a recorded
    // directory whose read bit is taken away still validates, and the index
    // keeps reporting the repositories underneath it that a walk can no longer
    // enumerate. Unlike the `touch -t` case this one does not decay: no later
    // event repairs it. `--cache-check` is again the whole answer.
    //
    // Mode 0111 and not 0000, and the difference is the whole test. Without
    // the execute bit the validation `statat` on a child cannot traverse the
    // directory either, so the index notices and re-walks -- there is no blind
    // spot to demonstrate. Searchable-but-unreadable is the mode where the two
    // paths genuinely diverge: `statat` walks through it happily while
    // `readdir` is refused, so the index sees a subtree the walk cannot.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    let closed = f.root().join("github.com/closed");
    fs::create_dir_all(&closed).expect("directory fixture");
    repo(f.root(), "github.com/closed/inside");
    settle(f.root());
    f.warm();

    let visible = "github.com/a/x
github.com/closed/inside
";
    assert_eq!(f.list(&["--cache"]), visible);

    fs::set_permissions(&closed, fs::Permissions::from_mode(0o111)).expect("chmod 0111");
    assert_eq!(
        f.list(&["--cache"]),
        visible,
        "the index still reports what is underneath a directory it can no longer read"
    );
    assert_eq!(
        f.list(&["--no-cache"]),
        "github.com/a/x
",
        "which the walk cannot enumerate, so --no-cache is short and warns on stderr"
    );

    let assert = f.cmd().args(["list", "--cache-check"]).assert().code(1);
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    assert!(
        stderr.contains("disagrees with a fresh walk"),
        "the check reports the disagreement: {stderr}"
    );
    assert!(stderr.contains("github.com/closed/inside"), "and names it: {stderr}");

    // The short walk is not written back: the index on disk is left as it was,
    // so restoring the mode restores agreement without a rebuild.
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o755)).expect("restore the mode");
    f.cmd().args(["list", "--cache-check"]).assert().success();
    assert_eq!(f.list(&["--cache"]), visible);
}

// ---------------------------------------------------------------------------
// Enablement, exclusions and size
// ---------------------------------------------------------------------------

#[test]
fn no_cache_neither_reads_nor_writes_the_index() {
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    settle(f.root());

    f.list(&["--no-cache"]);
    assert!(
        !f.cache.path().join("scap").exists(),
        "a run that was told not to use the index must not create one"
    );

    f.warm();
    let before = fs::read(f.index_file()).expect("read the index");

    repo(f.root(), "github.com/a/y");
    assert_eq!(
        f.list(&["--no-cache"]),
        "github.com/a/x\ngithub.com/a/y\n",
        "--no-cache walks, so it sees everything"
    );
    assert_eq!(
        fs::read(f.index_file()).expect("read the index"),
        before,
        "and it leaves the index exactly as it found it"
    );

    // `--no-cache` also beats `--cache` and the configured key on the same
    // command line.
    f.cmd().args(["list", "--cache", "--no-cache"]).assert().success();
    assert_eq!(fs::read(f.index_file()).expect("read the index"), before);
}

#[test]
fn the_config_key_and_the_environment_variable_both_enable_the_index() {
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    settle(f.root());

    // The environment form.
    f.cmd().env("SCAP_LIST_CACHE", "1").arg("list").assert().success();
    assert!(f.cache.path().join("scap").exists(), "SCAP_LIST_CACHE=1 enables the index");
    let expected = f.list(&["--no-cache"]);
    let assert = f.cmd().env("SCAP_LIST_CACHE", "1").arg("list").assert().success();
    assert_eq!(String::from_utf8(assert.get_output().stdout.clone()).unwrap(), expected);

    // The configured form.
    fs::write(f.home.path().join("gitconfig"), "[scap]\n\tlistCache = true\n")
        .expect("write gitconfig");
    let assert = f.cmd().arg("list").assert().success();
    assert_eq!(String::from_utf8(assert.get_output().stdout.clone()).unwrap(), expected);
    assert!(f.index_hits() > 0, "the configured key reaches the same code path as the flag");

    // The configured key ALONE, with no environment variable and no flag on
    // the command line. Every other assertion in this row runs `list --cache`
    // one way or another, which would pass even if `scap.listCache` were
    // ignored outright: this is the only one that exercises the path from the
    // configuration snapshot into `index::mode`, and a fresh cache directory
    // is what makes "the index ran" observable without a flag to ask for it.
    let from_config = TempDir::new().expect("a cache only the config key can fill");
    f.cmd().env("XDG_CACHE_HOME", from_config.path()).arg("list").assert().success();
    assert!(
        from_config.path().join("scap").exists(),
        "scap.listCache alone must enable the index, with no flag and no variable"
    );
    let assert = f.cmd().env("XDG_CACHE_HOME", from_config.path()).arg("list").assert().success();
    assert_eq!(String::from_utf8(assert.get_output().stdout.clone()).unwrap(), expected);

    // The variable overrides the key, in the direction that turns it off.
    let off = TempDir::new().expect("a cache the index must not reach");
    let assert = f
        .cmd()
        .env("XDG_CACHE_HOME", off.path())
        .env("SCAP_LIST_CACHE", "0")
        .arg("list")
        .assert()
        .success();
    assert_eq!(String::from_utf8(assert.get_output().stdout.clone()).unwrap(), expected);
    assert!(
        !off.path().join("scap").exists(),
        "SCAP_LIST_CACHE=0 turns a configured index off without a flag"
    );
}

#[test]
fn two_roots_holding_the_same_relative_path_both_print_it() {
    // ADR-9 rule (vii): roots are walked in order and never deduplicated
    // against each other, so a repository two roots both hold is printed
    // twice. The index keeps one file per root, and the whole point of this
    // row is that nothing about that arrangement collapses the two.
    let f = Fixture::new();
    let alpha = dir(f.root(), "alpha");
    let beta = dir(f.root(), "beta");
    repo(&alpha, "github.com/a/x");
    repo(&beta, "github.com/a/x");
    repo(&beta, "github.com/b/only-here");
    settle(f.root());

    let roots = format!("{}:{}", alpha.display(), beta.display());
    let listing = |args: &[&str]| {
        let assert = f.cmd().env("SCAP_ROOT", &roots).arg("list").args(args).assert().success();
        String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 listing")
    };

    // Warm both roots' indexes, then compare.
    listing(&["--cache"]);
    listing(&["--cache"]);
    let cached = listing(&["--cache"]);
    assert_eq!(cached, listing(&["--no-cache"]), "two roots, cached and uncached");
    assert_eq!(
        cached, "github.com/a/x\ngithub.com/a/x\ngithub.com/b/only-here\n",
        "the shared relative path is printed once per root that holds it"
    );

    // Two index files, one per root, neither claiming the other's tree.
    let files = fs::read_dir(f.cache.path().join("scap")).expect("cache directory").count();
    assert_eq!(files, 2, "one index file per root");

    // A mutation under the second root is seen through its own index.
    repo(&beta, "github.com/c/added");
    assert_eq!(listing(&["--cache"]), listing(&["--no-cache"]), "after mutating one of the two");
}

#[test]
fn every_printed_form_agrees_between_the_index_and_a_walk() {
    // The index feeds `list`'s post-processing rather than bypassing it, so
    // every form that post-processing produces has to come out identical. The
    // plain form would not catch a divergence in the absolute paths, because
    // it prints the relative ones; `-p` and `--unique` are the two that read
    // the other parts of the same buffer.
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    repo(f.root(), "github.com/b/x");
    repo(f.root(), "github.com/b/y");
    repo(f.root(), "store/upstream.git");
    settle(f.root());
    f.warm();

    for form in
        [vec![], vec!["-p"], vec!["--unique"], vec!["-p", "--unique"], vec!["-e", "x"], vec!["x"]]
    {
        let mut cached = form.clone();
        cached.push("--cache");
        let mut fresh = form.clone();
        fresh.push("--no-cache");
        assert_eq!(f.list(&cached), f.list(&fresh), "the {form:?} form");
    }

    // And the forms really do differ from one another, so the loop above is
    // comparing something.
    assert_ne!(f.list(&["--cache"]), f.list(&["-p", "--cache"]));
    assert_ne!(f.list(&["--cache"]), f.list(&["--unique", "--cache"]));
}

#[test]
fn an_excluded_subtree_stays_excluded_and_comes_back_when_the_pattern_does() {
    let f = Fixture::new();
    repo(f.root(), "github.com/a/x");
    repo(f.root(), "vendor/bundled");
    settle(f.root());

    let excluded = |cmd: &mut Command| {
        cmd.env("SCAP_LIST_EXCLUDE", "vendor");
    };
    let mut warm = f.cmd();
    excluded(&mut warm);
    warm.args(["list", "--cache"]).assert().success();
    let mut second = f.cmd();
    excluded(&mut second);
    let assert = second.args(["list", "--cache"]).assert().success();
    assert_eq!(
        String::from_utf8(assert.get_output().stdout.clone()).unwrap(),
        "github.com/a/x\n",
        "an excluded subtree is neither walked nor listed under the index"
    );

    // Mutating inside the excluded subtree changes nothing, because the
    // subtree was never in the index to be validated.
    repo(f.root(), "vendor/second");
    let mut third = f.cmd();
    excluded(&mut third);
    let assert = third.args(["list", "--cache"]).assert().success();
    assert_eq!(String::from_utf8(assert.get_output().stdout.clone()).unwrap(), "github.com/a/x\n");

    // Dropping the pattern has to bring the subtree back. No directory's
    // mtime moved, so only the exclusion set recorded in the index can reveal
    // the change.
    assert_eq!(
        f.list(&["--cache"]),
        "github.com/a/x\nvendor/bundled\nvendor/second\n",
        "the index records the patterns it was built under, so removing one invalidates it"
    );
}

#[test]
fn the_index_stays_well_under_two_megabytes_for_twenty_five_thousand_directories() {
    // AC-5's size bound. The layout is the shape a repository root actually
    // has -- host, owner, repository -- so the recorded paths are the length
    // they would really be.
    //
    // Measured margin at the time of writing: 1,329,613 bytes against the 2 MB
    // bound, about 66 % of it, for 25,120 directories -- roughly 53 bytes per
    // encoded entry. It is recorded here because the margin is a function of
    // path *length* and not of entry count: the maintainer's own unexcluded
    // corpus averages about 114 bytes per entry and passes 2 MB at only 17,778
    // of them. A layout change here, or a format change that grows an entry,
    // should move this figure visibly rather than quietly eating the headroom.
    let f = Fixture::new();
    let mut made = 0usize;
    for owner in 0..160 {
        let owner_dir = f.root().join(format!("github.com/owner-{owner:03}"));
        fs::create_dir_all(&owner_dir).expect("owner directory");
        made += 1;
        for project in 0..156 {
            fs::create_dir(owner_dir.join(format!("project-{project:03}"))).expect("project");
            made += 1;
        }
    }
    assert!(made >= 25_000, "the fixture is the size the bound is stated for: {made}");

    f.list(&["--cache"]);
    let size = fs::metadata(f.index_file()).expect("index metadata").len();
    assert!(size <= 2 * 1024 * 1024, "index for {made} directories is {size} bytes, over 2 MB");
}
