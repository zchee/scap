---
name: release-prep
description: Run scap's pre-release checklist — fmt, clippy, test, version bump, and a draft Conventional-Commits-style changelog entry. Invoke when the user wants to cut a release.
disable-model-invocation: true
---

# release-prep

User-triggered only. Side effects (version bump, changelog write) should never run without an explicit `/release-prep` invocation.

## Arguments

- `$ARGUMENTS` — optional version bump kind: `patch` | `minor` | `major`. Default: `patch`.

## Procedure

1. **Sanity-check the tree.**
   - `git status --porcelain` must be empty. If not, stop and ask the user to commit or stash.
   - `git rev-parse --abbrev-ref HEAD` should be `main` (or `master` per the repo's primary branch). If not, ask before proceeding.

2. **Run the quality gates.** All must pass:
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `cargo test --all-features`
   - `cargo build --release`

3. **Bump the version** in `Cargo.toml` according to `$ARGUMENTS` (default `patch`). If the project is a workspace, bump all member crates that share the workspace version. Run `cargo update -p scap` afterward so `Cargo.lock` reflects the new version.

4. **Generate a changelog draft.** Collect commits since the last tag:

   ```
   git log $(git describe --tags --abbrev=0)..HEAD --pretty=format:'%s'
   ```

   Group by Conventional Commits type (`feat`, `fix`, `perf`, `refactor`, etc.). Drop `chore`, `ci`, `build`, `docs` (unless user requests otherwise). Format as a draft `CHANGELOG.md` entry under the new version header. Do **not** commit yet — present the draft for review.

5. **Hand off.** Show the user:
   - The new version in `Cargo.toml`.
   - The changelog draft.
   - The next commands they'll likely want (`git commit -S`, `git tag -s v<version>`, `git push --follow-tags`).

   Do not run the tag or push. Releases are user-driven; this skill prepares, the human signs off.
