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
