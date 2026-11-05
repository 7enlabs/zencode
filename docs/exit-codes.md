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
| 3 | not found: import of a missing file; unknown tool in tool-help; unknown session id in sessions archive/restore/delete; review against a missing ref | lib.rs, session_mgmt.rs, review.rs |
| 4 | validation: clap usage errors; unrecognized top-level command; --min-pass-rate out of range; review outside a git repository; unparseable model output in review; invalid skill name | main.rs, lib.rs, review.rs, skills.rs |
| 5 | forbidden: deleting a session without --yes on non-interactive stdin; installing a skill without --yes; evaluating a non-free model | session_mgmt.rs, skills.rs, eval.rs |
| 6 | rate limit: review provider failures where the provider reports a rate limit (429) | review.rs |
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

Do not renumber in a patch that is not about exit codes; land each
queue item with its own tests.
