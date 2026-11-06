# Instruction drift check

Coding agents read `AGENTS.md`, `CLAUDE.md`, `ZEN.md` and `SKILL.md` as instructions and read the
code as tool output. When the two disagree, agents mostly follow the stale file: in arXiv 2610.30571
(Xu and Wu, preprint, Python projects only) agents followed the outdated file in about 90% of choices,
and one added line saying the existing code takes precedence moved that to about 91% following the code.
This check keeps our instruction files honest, and the second half of this page says how to measure
whether the precedence line helps on ZenCode.

## What the check does

`scripts/check_instruction_drift.py` scans every tracked file named `AGENTS.md`, `CLAUDE.md`, `ZEN.md`
or `SKILL.md`. It needs only Python 3 and runs in the `instruction-drift` job on every PR and on `main`.

| Claim in the file | Checked against | Missing means |
|---|---|---|
| Markdown link to a repo file | the file tree | error |
| Inline code path with a known extension or a trailing `/`, under an existing top-level dir (`crates/zen-cli/src/x.rs`, `docs/`) | the file tree | error |
| `cargo ... -p <name>` | workspace members in `Cargo.toml` | error |
| `zen <word>` alone on a line in a code block | `Commands` enum in `crates/zen-cli/src/cli.rs` | error (a lone unknown word exits 4) |
| `zen mcp\|skill\|sessions <sub>` | the matching subcommand enum | error |
| Other paths (`.zen/settings.json`, bare file names) | the file tree | warning (often runtime or user paths) |
| `zen <word> ...` with more words, or after a flag | `Commands` | warning (may be a prompt or a flag value) |
| `allowed-tools` in SKILL.md front matter | built-in tool names in `crates/zen-tools`, case-insensitive | warning (MCP tools are legitimate) |
| A code span that reads like a CI job on a line that says "job" | job ids in `.github/workflows/` | warning |

Errors fail the job. Warnings show as annotations and in the job summary and never fail it. An error on
a line that says the thing is planned (`planned`, `not yet`, `roadmap`, `TODO`, `proposed`, `future`)
becomes a warning.

### Reading and fixing a finding

The job prints `file:line: severity: [kind] message` and annotates the PR diff. Fix the file to match
the code. If the code is what is wrong, fix the code in its own PR and keep the finding until it lands.
When a line must stay as it is (a path from an old release, a deliberate example), add
`<!-- drift-ignore -->` on that line and say why in the PR.

Run locally from the repo root:

```sh
python3 scripts/check_instruction_drift.py          # tracked instruction files
python3 scripts/check_instruction_drift.py --json   # machine-readable
python3 -m unittest scripts/test_check_instruction_drift.py
```

### Limits

It checks names, not meaning. A file that says "use `anyhow`" in a crate that uses `thiserror` passes.
Semantic conventions are what the measurement below is for.

## Measuring the code-precedence line on ZenCode

Goal: show whether the line in `AGENTS.md` telling agents that existing code wins over a conflicting
instruction changes what `zen` does on this repo, without making it ignore instructions that are right.

### Arms

- A (before): `AGENTS.md` at the last commit without the precedence line.
- B (after): `AGENTS.md` at the commit that adds it.

Everything else is fixed: same `zen` build, same model, same `--max-turns`, same commit of the code.

### Scenarios

Build each scenario as a branch from the same base commit with one stale line appended to `AGENTS.md`:

1. Conflict scenarios (at least 8): the line states a convention the code clearly does not follow, and
   the task touches that code. Examples: "errors are returned with `anyhow::bail!`" where the crate
   uses its own error enum; "tests live in `tests/` only" where the module uses `#[cfg(test)]` inline;
   "paths use `std::env::home_dir`" where the code uses the `dirs` crate.
2. Control scenarios (at least 2): the line is right and the code is the part being migrated, and the
   task says so. These catch the risk that the precedence line makes the agent ignore a valid new rule.

Write down for each scenario, before any run, what "follows the code" and "follows the file" look like
in the diff, so the scoring is not decided after seeing the output.

### Runs

Run every scenario at least 3 times per arm:

```sh
for arm in A B; do
  for s in scenarios/*; do
    for i in 1 2 3; do
      git checkout -q "drift-$arm-$(basename "$s")"
      zen -p "$(cat "$s/task.txt")" --max-turns 25 > "runs/$arm-$(basename "$s")-$i.txt"
      git diff > "runs/$arm-$(basename "$s")-$i.diff"
      cargo test --workspace > "runs/$arm-$(basename "$s")-$i.test" 2>&1; echo $? >> "runs/$arm-$(basename "$s")-$i.test"
      zen --json cost >> "runs/$arm-$(basename "$s")-$i.cost.json"
      git checkout -q . && git clean -fdq
    done
  done
done
```

### Score

Per run, from the diff and transcript: follows code, follows file, or mixed; whether the agent named
the conflict; whether `cargo test --workspace` passed; cost from `zen --json cost`.

Report per arm: follow-code rate on conflict scenarios, follow-file rate on control scenarios,
conflict-flag rate, test pass rate, mean cost. Keep the raw run files with the report.

B helps if its follow-code rate on conflict scenarios is clearly higher than A's, its test pass rate is
not lower, and its follow-file rate on control scenarios does not drop. If control scenarios get worse,
reword the line to "when this file clearly conflicts with the code, follow the code and report the
conflict" and run again. With 8 conflict scenarios and 3 runs each, treat a gap under about 15 points
as noise.
