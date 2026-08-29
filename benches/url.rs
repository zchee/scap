//! Divan micro-benchmarks for repository-input parsing.
//!
//! [`scap::url::from_input`] runs once per target of `scap get`, `scap
//! create` and `scap rm`, and once more per `--parallel` worker, so it sits
//! directly on the path the plan's AC-3a user-CPU budget measures. It is
//! also pure: no filesystem, no subprocess, no clock, which makes it the
//! best-behaved thing in the tree for CodSpeed's simulation instrument.
//!
//! Uninstrumented these run as plain divan (`cargo bench --bench url`);
//! under `cargo codspeed build` the same source compiles against the
//! instrumented harness. Numbers from either are NOT comparable to the
//! hyperfine rows in `docs/benchmarks/`, which are whole-process walltime
//! measured under the plan's D-4 `RUSTFLAGS` regime on a quiet machine.

use divan::black_box;

fn main() {
    divan::main();
}

/// The spellings `scap get` is actually handed, one per distinct branch of
/// [`scap::url::from_input`].
///
/// In order: canonical https, https with the `.git` suffix stripped by
/// `trim_repo_path`, scp-like ssh, explicit ssh with a port, https carrying
/// userinfo on a non-github host, the `owner/repo` short form, the bare
/// name that needs `scap.user` completion, a nested-owner path (GitLab
/// subgroups, which exercise the multi-segment owner join), a
/// region-explicit codecommit ref and the same with a profile, and a
/// non-git scheme that still has to parse.
const INPUTS: &[&str] = &[
    "https://github.com/motemen/ghq",
    "https://github.com/motemen/ghq.git",
    "git@github.com:motemen/pusheen-explorer.git",
    "ssh://git@ghe.example.com:2222/motemen/pusheen-explorer",
    "https://motemen@ghe.example.com/motemen/pusheen-explorer",
    "motemen/ghq",
    "peco",
    "https://gitlab.com/group/subgroup/project",
    "codecommit::us-east-1://my-repo",
    "codecommit::us-east-1://example-profile@my-repo",
    "svn://example.com/repo/trunk",
];

/// One spelling per iteration, so a regression names the branch it is in.
#[divan::bench(args = INPUTS)]
fn from_input(input: &str) -> scap::url::Repo {
    scap::url::from_input(black_box(input), black_box(Some("motemen")), black_box(false))
        .expect("every benchmark input parses")
}

/// The whole corpus per iteration: the aggregate a multi-target `scap get`
/// pays, and a less noisy single number to track per commit than any one
/// spelling above.
#[divan::bench]
fn from_input_corpus() -> usize {
    let mut parsed = 0;
    for input in INPUTS {
        let repo = scap::url::from_input(black_box(input), black_box(Some("motemen")), false)
            .expect("every benchmark input parses");
        parsed += black_box(&repo).name.len();
    }
    parsed
}

/// The codecommit branch, measured through its only public caller.
///
/// `scap::url::is_codecommit_input` -- the matcher that decides this
/// dispatch, and that ADR-13 changed to ghq's `[^]]+` user class -- is
/// `pub(crate)`, so a `benches/` target cannot call it and cannot measure
/// the spellings it *rejects* in isolation: from `from_input`, a rejected
/// spelling either fails to parse or, for a region-absent ref, resolves its
/// region by spawning `aws`, neither of which belongs in a benchmark.
///
/// What is measured is the accepted side, which is the side that runs on a
/// real `scap get`: region-explicit with no profile, with a profile, and
/// with the punctuation the repository-name and `<profile>@` classes both
/// have to admit.
mod codecommit {
    use divan::black_box;

    const PROBES: &[&str] = &[
        "codecommit::us-east-1://my-repo",
        "codecommit::eu-west-2://example-profile@repo_1.x-y",
        "codecommit::ap-southeast-1://user.name@my.repo-name_2",
    ];

    #[divan::bench(args = PROBES)]
    fn dispatch(input: &str) -> scap::url::Repo {
        scap::url::from_input(black_box(input), black_box(None), black_box(false))
            .expect("every codecommit probe parses")
    }
}
