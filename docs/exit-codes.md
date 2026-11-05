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

## Current map (measured on main b8b9574, 2026-11-05 reaudit)

| Code | Current use | Source |
| --- | --- | --- |
| 0 | success | all subcommands |
| 1 | blanket error from main (anyhow), eval below pass rate, ci-fix failing checks, models --best with no working model | crates/zen-cli/src/main.rs, lib.rs, models.rs |
| 2 | clap usage errors; unrecognized top-level command; review run errors (not a git repo, no API key, unparseable model output); JSON schema validation failure | clap default, lib.rs, review.rs, agent_run.rs |
| 3 | import of a missing file; unknown tool in tool-help | lib.rs |
| 130 | interrupted by SIGINT in the REPL | repl.rs |

## Migration queue

- Q1: usage and validation errors move 2 to 4 (clap parse errors, the
  unrecognized-command fail-fast, invalid flag values such as
  --min-pass-rate out of range, which today exits 1 via the blanket
  error path).
- Q2: auth failures (missing or rejected API key) move to 2; today the
  review no-key path exits 2 by accident of its generic error code and
  other paths exit 1.
- Q3: review operational errors split by cause: not a git repository is
  validation (4), a missing resource is not found (3), provider
  failures stay general (1) or rate limit (6) when the provider says so.
- Q4: forbidden (5) and conflict (7) are not emitted anywhere yet;
  assign them when the first permission-denied and state-conflict
  surfaces are specified.
- Q5: the blanket exit 1 in main.rs stays as the fallback, but every
  new error path must pick its table code at the source instead of
  relying on the fallback.

Do not renumber in a patch that is not about exit codes; land each
queue item with its own tests.
