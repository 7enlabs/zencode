# CONSTRAINTS.md

Rules a change must keep, each with the command that decides it. Agents and humans
read this before they touch code. `scripts/floor_guard.py` enforces the Floor and the
Ratchet on every pull request (`.github/workflows/floor-guard.yml`).

## Floor

- Do not skip a test to get green: no new `#[ignore]` in Rust. Check: `python3 scripts/floor_guard.py --base origin/main`.
- Do not silence a lint or warning: no new `#[allow(...)]` or `#![allow(...)]`. Check: same command.
- Do not ship stubs: no new `todo!()` or `unimplemented!()`. Check: same command.
- Do not soften CI: no `continue-on-error: true`, no `|| true`, no removed `-D warnings` in `.github/workflows/`. Check: same command.
- Do not commit secrets. Check: the `secrets` job in `ci.yml` (gitleaks).
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

## Out of scope for the guard

Existing `#[allow]` and `#[ignore]` lines on main are left as they are; the guard only
reads what a change adds or removes, plus the test count.
