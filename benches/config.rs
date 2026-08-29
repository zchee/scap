//! Divan micro-benchmarks for the ADR-8 in-process configuration load.
//!
//! [`scap::config::load`] runs exactly once per process, before any command
//! does work, so its cost is pure startup latency -- the ≈0.1 ms the plan's
//! §0 measurement attributed to configuration. Every case here drives it
//! through an injected [`Env`], which is why that type is public: nothing
//! below reads or mutates the process environment, so the numbers do not
//! depend on the machine's `HOME`, its `/etc/gitconfig`, or whether the
//! benchmark happens to run inside a git repository.
//!
//! Every fixture stays on the in-process path on purpose. `git` is never
//! spawned: a benchmark that forked a subprocess would measure the host's
//! process table, not this crate.
//!
//! Uninstrumented these run as plain divan (`cargo bench --bench config`);
//! under `cargo codspeed build` the same source compiles against the
//! instrumented harness. Numbers from either are NOT comparable to the
//! hyperfine rows in `docs/benchmarks/`, which are whole-process walltime
//! measured under the plan's D-4 `RUSTFLAGS` regime on a quiet machine.

use std::ffi::OsString;
use std::path::PathBuf;

use divan::{Bencher, black_box};
use scap::config::{Env, load};
use tempfile::TempDir;

fn main() {
    divan::main();
}

/// A temp gitconfig tree plus the hermetic [`Env`] view of it.
///
/// Built once per benchmark function and reused across iterations: writing
/// the files is setup, not work, and rewriting them every iteration would
/// measure the filesystem's page cache.
struct Fixture {
    _tmp: TempDir,
    home: PathBuf,
    global: PathBuf,
}

impl Fixture {
    /// A global gitconfig with `includes` included files, plus the
    /// `scap.*` keys the loader reads.
    ///
    /// The two `scap.root` values are what rules (c) and (e) choose
    /// between; the first is written with a `~` so the `--path`
    /// interpolation runs too, and each included file contributes one more
    /// key so `gix-config`'s include resolution is on the measured path
    /// rather than short-circuited.
    fn new(includes: usize, url_sections: usize) -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize the tempdir");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("mkdir home");

        let mut global = String::from(
            "[scap]\n\
             \troot = ~/repos\n\
             \troot = /srv/scap\n\
             \tuser = motemen\n\
             \tcompleteUser = false\n\
             \tlistExclude = node_modules\n",
        );
        for i in 0..includes {
            let name = format!("include-{i}.gitconfig");
            std::fs::write(home.join(&name), format!("[scap]\n\tlistExclude = vendor-{i}\n"))
                .expect("write included gitconfig");
            global.push_str(&format!("[include]\n\tpath = {name}\n"));
        }
        for i in 0..url_sections {
            global.push_str(&format!(
                "[scap \"https://ghe-{i}.example.com/\"]\n\troot = /srv/scoped-{i}\n"
            ));
        }

        let path = home.join("gitconfig");
        std::fs::write(&path, global).expect("write global gitconfig");
        Self { _tmp: tmp, home, global: path }
    }

    /// A fixture with no gitconfig at all: `GIT_CONFIG_GLOBAL` names a path
    /// that does not exist, which is how git suppresses the level.
    fn empty() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize the tempdir");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("mkdir home");
        let global = home.join("absent.gitconfig");
        Self { _tmp: tmp, home, global }
    }

    /// `n` plain `scap.root` lines and nothing else, for the
    /// [`root_for_url`](scap::config::ConfigSnapshot::root_for_url) rule
    /// (c) case that varies the root count rather than the include count.
    fn with_roots(n: usize) -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize the tempdir");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("mkdir home");

        let mut global = String::from("[scap]\n");
        for i in 0..n {
            global.push_str(&format!("\troot = /root-{i}\n"));
        }
        let path = home.join("gitconfig");
        std::fs::write(&path, global).expect("write global gitconfig");
        Self { _tmp: tmp, home, global: path }
    }

    /// The isolated view: the system probe is skipped outright, the global
    /// file is the fixture's, and `cwd` is `None` so no repository is
    /// discovered and no repository-level file joins the source list.
    fn env(&self) -> Env {
        Env {
            home: Some(self.home.clone()),
            git_config_global: Some(self.global.clone()),
            git_config_nosystem: Some(OsString::from("1")),
            cwd: None,
            system_probe_candidates: Vec::new(),
            ..Env::default()
        }
    }
}

/// The floor: no configuration file exists anywhere, which is what a user
/// who has never set `scap.root` pays on every command.
#[divan::bench]
fn load_absent(bencher: Bencher) {
    let fixture = Fixture::empty();
    bencher
        .with_inputs(|| fixture.env())
        .bench_values(|env| black_box(load(black_box(&env)).expect("an absent gitconfig loads")));
}

/// The chartered shape: one global file plus three includes.
#[divan::bench]
fn load_three_includes(bencher: Bencher) {
    let fixture = Fixture::new(3, 0);
    bencher
        .with_inputs(|| fixture.env())
        .bench_values(|env| black_box(load(black_box(&env)).expect("the fixture gitconfig loads")));
}

/// The same, plus four url-scoped `[scap "<url>"]` sections.
///
/// These make the snapshot's reason `url_sections` -- the shape that sends
/// `root_for_url` to git for its answer -- while the load itself stays in
/// process, so this isolates what enumerating the subsections costs.
#[divan::bench]
fn load_url_sections(bencher: Bencher) {
    let fixture = Fixture::new(3, 4);
    bencher
        .with_inputs(|| fixture.env())
        .bench_values(|env| black_box(load(black_box(&env)).expect("the fixture gitconfig loads")));
}

/// How the include count scales, which is the one dimension of a real
/// user's gitconfig that varies by an order of magnitude.
#[divan::bench(args = [0, 1, 8, 32])]
fn load_by_include_count(bencher: Bencher, includes: usize) {
    let fixture = Fixture::new(includes, 0);
    bencher
        .with_inputs(|| fixture.env())
        .bench_values(|env| black_box(load(black_box(&env)).expect("the fixture gitconfig loads")));
}

/// [`ConfigSnapshot::root_for_url`](scap::config::ConfigSnapshot::root_for_url),
/// ADR-8 rules (a)-(e) -- the one config hot path the whole-load benchmarks
/// above cannot see, because it runs on a snapshot already in hand rather
/// than during `load`. Every fixture's snapshot is built once, before the
/// timed body starts; only the lookup itself is measured.
///
/// Rule (d) (a visible `[scap "<url>"]` section) is deliberately absent: it
/// spawns `git config --get-urlmatch`, which every benchmark in this file
/// avoids on principle (module doc).
mod root_for_url {
    use divan::{Bencher, black_box};
    use scap::config::{Env, load};

    use super::Fixture;

    /// Rule (c): no url-scoped section is visible, so the last plain
    /// `scap.root` wins, raw.
    #[divan::bench]
    fn rule_c_plain_last(bencher: Bencher) {
        let fixture = Fixture::new(0, 0);
        let snapshot = load(&fixture.env()).expect("the fixture gitconfig loads");
        let url = "https://github.com/x/y";
        bencher.bench(|| black_box(snapshot.root_for_url(black_box(url)).expect("root_for_url")));
    }

    /// Rule (a): `SCAP_ROOT` wins outright over every configured root.
    #[divan::bench]
    fn rule_a_scap_root_env(bencher: Bencher) {
        let fixture = Fixture::new(0, 0);
        let env = Env { scap_root: Some("/env-root".into()), ..fixture.env() };
        let snapshot = load(&env).expect("the fixture gitconfig loads");
        let url = "https://github.com/x/y";
        bencher.bench(|| black_box(snapshot.root_for_url(black_box(url)).expect("root_for_url")));
    }

    /// Rules (b)+(e): a codecommit target skips urlmatch and lands on the
    /// canonicalised primary root instead of rule (c)'s raw last value.
    #[divan::bench]
    fn rule_b_codecommit(bencher: Bencher) {
        let fixture = Fixture::new(0, 0);
        let snapshot = load(&fixture.env()).expect("the fixture gitconfig loads");
        let url = "codecommit://us-east-1/codecommit/my-repo";
        bencher.bench(|| black_box(snapshot.root_for_url(black_box(url)).expect("root_for_url")));
    }

    /// Rule (c) again, over 8 plain roots instead of 2: `Vec::last` is O(1),
    /// so this isolates whether the surrounding lookup scales with root
    /// count at all.
    #[divan::bench]
    fn rule_c_eight_plain_roots(bencher: Bencher) {
        let fixture = Fixture::with_roots(8);
        let snapshot = load(&fixture.env()).expect("the fixture gitconfig loads");
        let url = "https://github.com/x/y";
        bencher.bench(|| black_box(snapshot.root_for_url(black_box(url)).expect("root_for_url")));
    }
}
