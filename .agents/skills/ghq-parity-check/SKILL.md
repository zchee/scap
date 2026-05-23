---
name: ghq-parity-check
description: Compare a proposed scap command, flag, or config key against ghq's surface before implementing. Run before adding new CLI surface area to catch unintended divergence from ghq.
---

# ghq-parity-check

scap is intentionally a close port of [x-motemen/ghq](https://github.com/x-motemen/ghq). Surface drift (a renamed subcommand, a missing flag, an inconsistent config key) is the single biggest risk to that goal.

Run this skill before adding or renaming:

- a subcommand
- a top-level flag
- a `ghq.*` / scap config key
- a path-layout rule

## Procedure

1. Identify the proposed surface element from the user's request (e.g. "add `scap sync` subcommand", "add `--depth` flag to `get`").

2. Check ghq's actual surface. Prefer real tool output over memory:
   - `which ghq` — if installed locally, use `ghq help` and `ghq help <subcommand>` for ground truth.
   - Otherwise fetch the relevant section of https://github.com/x-motemen/ghq/blob/master/README.md via WebFetch.
   - For config keys, inspect ghq's source under `cmd*.go` and `config.go` in the upstream repo.

3. Produce a short report with three sections:
   - **In ghq**: exact name, flags, behavior.
   - **Proposed in scap**: what the user is asking for.
   - **Verdict**: one of `match` (proceed as-is) / `align` (rename/reshape to match ghq, here's how) / `intentional-divergence` (explain why ghq's design is wrong or insufficient for scap).

4. If verdict is `intentional-divergence`, require the divergence to be documented in:
   - a doc comment on the relevant `clap` struct/enum, and
   - the PR description.

## Out of scope

This skill does not implement the command. It only checks parity. Hand back to the user (or the executor) once the verdict is clear.
