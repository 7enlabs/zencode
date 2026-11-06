# ZenCode CLI visual redesign (REPL)

Status: design + implementation in progress (branch `dev/repl-model-picker`).
Date: 2026-11-06. Requested by Zen after he found the working screen
unpolished and `/model` undiscoverable.

## References (design language only, not code)

Claude Code working screens (screenshots supplied by Zen, 2026-11-06) plus
public write-ups of its terminal UX (competitive research, DeepWiki pages
on its Ink/React TUI). We borrow the *visual language* — layout, rhythm,
markers — and write all ZenCode code originally. No Claude Code code,
prompts or tool descriptions are copied (clean-room rule, AGENTS.md).

## What Claude Code does that ZenCode should adopt

| Pattern | Claude Code | ZenCode adaptation (line-based REPL) |
| --- | --- | --- |
| Welcome dashboard | Bordered box: version, greeting, pixel mascot, model + plan, cwd; right column Recent activity + What's new; adaptive to terminal width | Bordered box: `ZenCode CLI vX.Y.Z`, greeting, Zen ASCII mark, model + provider, cwd; Recent activity (last sessions) + What's new (CHANGELOG top items) stacked below when the terminal is narrow; condensed to a minimal header when there is nothing to show |
| Bottom status bar | `? for shortcuts` left, mode right (`Thinking off (tab to toggle)`); model + session + tokens | A dim rule + status line printed before each prompt: model, cwd, branch, permission mode; `?` hint stays in the welcome text |
| Single composer | One input line at the bottom; `>` prompt; natural language + `/` commands + `@` mentions in one place | Already the case (rustyline). Keep `> `, coloured by theme |
| Assistant marker | `●` prefix on assistant text | Prefix assistant turns with a coloured `● ` |
| Tool-call timeline | Each tool call is its own collapsible line: `● Bash(ls -la)`, `└` output, collapsed to `+N lines (ctrl+o to expand)` | ZenCode already has numbered tree cards with `└` and collapsed results (`/expand`). Align the format: `● <Tool>(<args>)`, `└` summary, `+N lines (/expand to show)` |
| Thinking indicator | Friendly verb + elapsed time, accent colour (`Skedaddling…`, `Simmering…`, `Conjuring…`); spinner | A verb line in accent colour with the real elapsed time from the heartbeat (`Thinking… 3s`, Zen-flavoured verbs); reuse the existing still-working line |
| Todos | `● Update Todos` + `└ □ item` / `☑ done` | Style `/todos` output the same way: `●` header, `└ □` / `└ ☑` items |
| Dialogs | Bordered box, tabs (`Discover | Installed | … (tab to cycle)`), `Select: Enter · Back: Esc` footer | Keep ZenCode's existing confirm prompts; style new pickers (see `/model`) as a bordered list with the same footer wording |
| Colours | Dark background, orange/coral accent, dim grey secondary, green success | ZenCode brand: blue accent (Z-Check `#2563EB`), dim grey secondary, green success; keep `NO_COLOR` / `TERM=dumb` behaviour |

## What we deliberately do NOT copy

- The fullscreen alternate-screen TUI (React/Ink). ZenCode stays a
  line-based REPL (rustyline). No ratatui/fullscreen rewrite in this change.
- Claude Code's mascot (pixel robot), its exact wording, or its orange.
  ZenCode gets its own ASCII mark and its own blue accent.
- Any Claude Code code, prompts, or tool descriptions (clean-room).

## Changes in this branch

1. `/model` picker (fixes "don't know where to change the model"):
   `/model` with no args lists the available models (probed pool cache when
   it matches the provider, else the static free catalog) with the current
   model marked; `/model <number>` or `/model <name>` switches (exact id
   wins, else a unique substring). `available_model_ids`,
   `format_model_list`, `pick_model` are pure and unit-tested.
2. `/help` shows the one-line description of every command (from
   `slash_command_catalog`), so the list is no longer bare names.
3. Welcome: brand line, model + cwd on their own indented lines, and the
   hint now names `/model` first (the discoverability gap).
4. Welcome dashboard box, status-line restyle, `●` assistant prefix,
   tool-card format alignment, thinking-verb line, todo styling, blue
   accent — per the table above (items 4–8, in progress).

## Acceptance

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` all pass; the mock eval stays at 100%.
- `/model`, `/help`, welcome, and a tool-call turn are eyeballed on a real
  terminal (Windows PowerShell + Linux) before the PR is marked ready.
- No new dependency. No change to the stream-json contract. No change to
  `main` except through a reviewed PR (never self-merge).
