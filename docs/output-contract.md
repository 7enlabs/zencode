# Output contract

The global flags `--quiet`, `--json` and `--raw` follow one contract on
every subcommand:

- `--quiet` suppresses stdout only. It never skips the work, never
  swallows an error, and never changes the exit code: a failing command
  fails identically with and without `--quiet`, with its error on
  stderr.
- `--raw` is machine-readable output (Q1): for `zen doctor --raw` that
  is the same JSON as `--json`, by design.
- In `--json` mode, stdout carries only the JSON payload (Q2): errors
  stay plain text on stderr, never JSON objects, and all diagnostics
  go to stderr.

- Export is the exception that proves the `--quiet` rule: `--quiet`
  never suppresses the export payload. With `--output` the payload is
  still written to the file and only the stderr confirmation is
  silenced; without `--output` the payload still prints to stdout.
- Evidence labels on claims and findings are the model's own claim
  about its evidence, not a tool-verified fact. The model chooses
  [CONFIRMED], [INFERRED] or [UNVERIFIED] following the system
  prompt, and [CONFIRMED] means the model says it checked the claim
  against a cited source; the tool does not re-verify the label. (The
  eval pass line is the exception: it is computed by the tool from
  the per-task results, not assigned by the model.)
