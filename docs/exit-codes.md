# Exit codes: current map and migration queue

Status: migration queue recorded under team coding law B2 (2026-11-05).
The target table is fixed; commands that do not match it yet are listed
here instead of being changed silently in unrelated patches.

## Target table (law B2)

| Code | Meaning |
| --- | --- |
| 0 | ok |
| 1 | general error |
| 2 | auth |
| 3 | not found |
| 4 | validation |
| 5 | forbidden |
| 6 | rate limit |
| 7 | conflict |

## Current map (after the Q1-Q5 migration, 2026-11-05)

| Code | Current use | Source |
| --- | --- | --- |
| 0 | success | all subcommands |
| 1 | blanket fallback error from main (anyhow), eval below pass rate, ci-fix failing checks, models --best with no working model, review provider failures, review verdict at/above --fail-on | crates/zen-cli/src/main.rs, lib.rs, models.rs, review.rs |
| 2 | auth: review with no API key configured; auth-shaped failures from the agent layer (401, unauthorized) mapped in main | review.rs, lib.rs `exit_code_for_error` |
| 3 | not found: import of a missing file; unknown tool in tool-help; unknown session id in sessions archive/restore/delete; review against a missing ref; agent-layer provider failures reporting 404, mapped in main | lib.rs, session_mgmt.rs, review.rs |
| 4 | validation: clap usage errors; unrecognized top-level command; --min-pass-rate out of range; review outside a git repository; unparseable model output in review; invalid review scope arguments (--base together with --commit, empty ref or sha); invalid skill name | main.rs, lib.rs, review.rs, skills.rs |
| 5 | forbidden: deleting a session without --yes on non-interactive stdin; installing a skill without --yes; evaluating a non-free model; agent-layer provider failures reporting 403, mapped in main | session_mgmt.rs, skills.rs, eval.rs, lib.rs |
| 6 | rate limit: review provider failures where the provider reports a rate limit (429); agent-layer provider failures reporting 429, mapped in main | review.rs, lib.rs |
| 7 | conflict: archiving a session that is already archived; restoring a session that is not archived or whose active copy already exists; creating a skill that already exists | session_mgmt.rs, skills.rs |
| 130 | interrupted by SIGINT in the REPL | repl.rs |

## Migration queue

Landed 2026-11-05 (all five items, each with tests):

- Q1: usage and validation errors moved 2 to 4 (clap parse errors via
  `try_parse` in main, the unrecognized-command fail-fast, invalid flag
  values such as --min-pass-rate out of range). Note: this changes the
  exit code scripts may have relied on for usage errors (2 was clap's
  default); the Desktop and Sen consumers were flagged through the
  team channel when this landed.
- Q2: auth failures (missing or rejected API key) exit 2: the review
  no-key path picks AUTH at the source, and auth-shaped agent-layer
  failures are mapped in `exit_code_for_error`.
- Q3: review operational errors split by cause: not a git repository
  is validation (4), a missing ref is not found (3), provider failures
  stay general (1) or rate limit (6) when the provider says so.
- Q4: forbidden (5) and conflict (7) are now emitted at their first
  specified surfaces (see the map above): policy refusals that require
  an explicit --yes or break the free-only rule are forbidden; state
  mismatches (already archived, not archived, already exists) are
  conflict.
- Q5: the blanket exit 1 in main.rs stays as the fallback, but error
  paths pick their table code at the source via `CodedError`
  (`zen_cli::coded_error`); `exit_code_for_error` in lib.rs is the
  single place main consults.

Landed 2026-11-11 (bug hunt B P2-5, with tests):

- The agent/headless path now maps provider status shapes in
  `exit_code_for_error`: 429 to rate limit (6), 403 to forbidden (5),
  404 to not found (3), instead of the blanket 1. Previously only the
  review path split 429 out; scripts driving `zen -p` could not tell a
  rate limit from a general failure.

Landed 2026-11-12 (issue #165, with tests):

- The 2026-11-11 item only mapped errors that propagate to `main`:
  the `-p` headless turn reports a provider failure as a
  `result/error_during_execution` event whose text is
  `HTTP <status>: ...`, so it never reached `exit_code_for_error`
  and always exited 1. The headless path now maps the status from
  the result text itself (`exit_code_for_headless_result` in lib.rs):
  401 to auth (2), 403 to forbidden (5), 404 to not found (3),
  409 to conflict (7), 429 to rate limit (6), 5xx stays general
  (1); errors without an HTTP status stay 1, success stays 0.
  The same patch also maps 409 to conflict (7) in
  `exit_code_for_error`, which previously fell through to 1.

Do not renumber in a patch that is not about exit codes; land each
queue item with its own tests.
