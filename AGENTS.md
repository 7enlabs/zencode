# AGENTS.md - instructions for coding agents working on ZenCode

Short version of the rules. Details live in [CONTRIBUTING.md](CONTRIBUTING.md),
[REVIEW.md](REVIEW.md) and [SPEC.md](SPEC.md). If this file and CONTRIBUTING.md
disagree, CONTRIBUTING.md wins and this file is the bug.

## Hard rules

1. Never push to `main`. One branch (`dev/<name>`, `lead/<name>`, `hotfix/<name>`)
   is one task is one PR. Never self-merge. PRs, commits and code comments are in English.
2. No secrets, tokens, API keys or personal paths in code, tests, fixtures or docs.
   Test fixtures use obviously fake values.
3. Clean-room: never copy code, prompts or tool descriptions from other coding
   tools. Public docs, SDK types and changelogs are the only references.
4. The stream-json contract (`docs/stream-json-v1.md`) is additive only. A breaking
   change needs its own PR and cross-team review.
5. Changes to `.github/workflows/`, installer or release scripts, permissions,
   secrets, auto-update, or network calls to a new host need a human reviewer
   (see "Always needs a person" in REVIEW.md). Do not ship them on an agent's word.

Also: no `unwrap()` in production paths, and code must work on Linux, macOS and
Windows (use the `dirs` crate, never hardcode `/home` or `C:\`).

## Commands

Run from the repo root. These three are what CI runs; "done" means all pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Extra checks by area:

```sh
# agent loop, tools, prompts: the mock eval must stay at 100%
cargo run --locked -q -p zen-cli -- eval --mock --min-pass-rate 1.0

# anything that changes config resolution, trust or permissions: look at the diagnostics
cargo run -q -p zen-cli -- doctor
```

Paths, shells and process handling are checked by the Windows job (`windows-check`)
on the PR; do not call such a change done on a Linux run alone.

## Definition of done

- The commands above pass and you read their real output (not "should pass").
- A bug fix has a test that failed before the fix.
- The diff is minimal: no unrelated refactor, rename or new dependency.
- The PR says what was checked and what was not.
