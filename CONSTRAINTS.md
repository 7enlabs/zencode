# CONSTRAINTS.md

Rules a change must keep, each with the command that decides it. Agents and humans
read this before they touch code. `scripts/floor_guard.py` enforces the Floor, the Ratchet and the
Task guard on every pull request (`.github/workflows/floor-guard.yml`).

## Floor

- Do not skip a test to get green: no new `#[ignore]` in Rust. Check: `python3 scripts/floor_guard.py --base origin/main`.
- Do not silence a lint or warning: no new `#[allow(...)]` or `#![allow(...)]`. Check: same command.
- Do not ship stubs: no new `todo!()` or `unimplemented!()`. Check: same command.
- Do not soften CI: no `continue-on-error: true`, no `|| true`, no removed `-D warnings` in `.github/workflows/`. Check: same command.
- Do not commit secrets. Check: the `secrets` job in `ci.yml` (gitleaks).
- No unmarked inline-disable in runtime Rust code: no `#[allow(...)]` or `#![allow(...)]` in `crates/*/src` outside test files and `#[cfg(test)]` modules unless the line carries `// floor-guard: <reason>`; the count of justified runtime allows never exceeds `max_runtime_allows` in `ci/floor-baseline.json`. Check: `python3 scripts/arch_lint.py`.
- Do not loosen this file to get a change through: removing or rewording a Floor line, or lowering a number in `ci/floor-baseline.json`, fails the guard. Check: same command.

A line that truly needs one of these carries `// floor-guard: <reason>` on the same line
or the line above it. A human reviewer who accepts the reason adds the
`floor-change-approved` label to the pull request; that label is the only override.

## Ratchet

- Rust test functions (`#[test]`, `#[tokio::test]` under `crates/`) never fall below
  `min_rust_tests` in `ci/floor-baseline.json`. When the guard reports a higher count,
  raise the number in the same or the next PR so the gain is locked in.
- Clippy stays at zero warnings. Check: `cargo clippy --workspace --all-targets -- -D warnings`.
- Formatting stays clean. Check: `cargo fmt --all -- --check`.
- Runtime `#[allow]` count never exceeds `max_runtime_allows` in `ci/floor-baseline.json`; raising the number fails the guard (needs the `floor-change-approved` label). Check: `python3 scripts/arch_lint.py`.

## Task guard

- One task = one branch = one pull request (P0-3 section 4.3 layer 2). The branch
  name or the PR body must name a task code: `B-N`, `STU-N`, `APP-N`, `DV-N`,
  `DS-N`, `C-N`, `Việc N`, or `P0-N step M` (a step code, e.g. `P0-3 step 6`,
  counts as the task; the bare project code alone does not distinguish steps).
  Check: `python3 scripts/floor_guard.py --base origin/main`.
- Two open pull requests sharing a task code are flagged; the Leader decides
  which one goes on. The guard never cancels or closes the other PR itself.
  Check: same command (needs the open-PR list from CI).
- Task-guard findings are warnings until the date in
  `FLOOR_GUARD_TASK_WARN_UNTIL` in `.github/workflows/floor-guard.yml`
  (one week from merge), then they block the PR. A human reviewer who accepts
  the reason adds the `floor-change-approved` label to the pull request; that
  label is the only override.
- Dependabot branches are exempt: they are machine-made and carry no task code.
- Do not loosen this section to get a change through: removing or rewording a
  Task guard line fails the guard. Check: same command.

## Out of scope for the guard

Existing `#[allow]` and `#[ignore]` lines on main are left as they are; the guard only
reads what a change adds or removes, plus the test count.
