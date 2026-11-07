# ZenCode CLI visual redesign v2 (REPL)

Status: design only — no code changed in this document's pass.
Date: 2026-11-06. Requested by Zen after seeing the v1 screenshot
(`~/workspace/zen-repl-shot.png`, PR #56): "design needs more learning
and improvement".

Supersedes the v1 design (`repl-visual-redesign.md`) for the welcome box,
mark, colours, `/model` list, tool cards, thinking line and `/todos`.
The v1 functional work (the `/model` picker itself, `/help`
descriptions, the status line following live model switches) stays.

## 0. Framing

- Goal: make the first screen and the working screen look deliberate —
  one brand voice, one visual hierarchy — without changing what the REPL
  is: a line-based, scrollback-friendly prompt (rustyline), not a
  fullscreen TUI.
- Keep: everything that already works in v1 (picker semantics, `/help`
  descriptions, live-model status line, `●` assistant prefix).
- Change: the mark, the box geometry and colour, the picker columns,
  the tool-card/result lines, the thinking line, the todo rendering,
  and the colour table.
- Hard constraints (from repo AGENTS.md + v1 doc): no new dependency;
  no fullscreen/alternate-screen rewrite; respect `NO_COLOR` and
  `TERM=dumb` (already handled by `Theme::effective()` in
  `repl_polish.rs`); cross-platform (Windows PowerShell + Linux);
  clean-room — learn the visual language only, never copy code,
  prompts, tool descriptions, mascots, or brand colours from another
  coding tool.

## 1. References (design language only, not code)

- Claude Code working screens (screenshots supplied by Zen + public
  write-ups): LogoV2 dashboard (adaptive full/condensed layout,
  FeedColumn with Recent Activity / What's New / Onboarding, condensed
  mode when there is nothing to show); `●` assistant marker; tool-call
  timeline with `└` results collapsed to `+N lines`; spinner verbs —
  one playful verb + `…` + elapsed, from a fixed list
  (`src/constants/spinnerVerbs.ts`, ~185–204 verbs, user-overridable);
  bottom status bar (`? for shortcuts` left, mode right); `ListItem`
  `❯` selection marker; tabbed dialogs with a footer
  (`Select: Enter · Back: Esc`).
- OpenCode TUI (OpenTUI, alternate screen — not copied, only the
  information design): home screen = logo + composer + tips + a footer
  line with cwd/branch, MCP status, `/status` hint and version;
  status bar shows active model + mode + connection state; the model
  picker shows model name, provider, token count, context limit, cost
  estimate and latency; dialogs are centred modal overlays.
- Aider (line-based REPL, the closest structural cousin to ZenCode):
  startup prints version, model and git status as plain lines; model
  switching is CLI-only (`--model`, `--list-models`) — no visual
  picker, which is exactly the gap Zen hit. Lesson: a REPL *can* ship
  a picker (ZenCode v1 does); Aider shows what happens without one.
- Gemini CLI (Ink TUI): `? for shortcuts` hint above the input;
  window-title status icons (Ready `◇`, Working `✦`, Action `✋`);
  `/model set <name>` with an optional `--persist` flag — the
  session-only vs default distinction is worth copying as a *concept*
  (see §5.3).
- Charmbracelet / Lip Gloss (Go TUI styling, design principles only):
  rounded borders (`╭╮╰╯`) for emphasis boxes; adaptive colours with
  distinct light/dark variants; styles named by *purpose* (`Error`,
  `Success`), not by appearance; every styled write checks TTY first
  so piped output stays clean; internal padding (1 vertical, 2
  horizontal) as breathing room inside boxes.

## 2. V1 critique — what is wrong in the screenshot, point by point

Read from `~/workspace/zen-repl-shot.png` (render of the real v1
output). Ordered by severity.

### 2.1 The block-Z mark is the biggest weakness [HIGH]

`ZEN_MARK` (`repl.rs:43`) is 5 rows of `█`:

```text
██████
    ██
   ██
  ██
██████
```

- In the screenshot it does **not** read as a Z. The diagonal is 2
  cells wide and steps only 1 row at a time, so the eye sees three
  horizontal bars plus two floating squares — it looks like a chunky
  `2`, a staircase, or a glitch. A mark must read instantly at a
  glance; this one needs a second look, and the second look concludes
  "probably a Z".
- It is monochrome white on black with no colour, so it carries zero
  brand identity — it could be any tool's mark.
- It is 6 columns wide inside a ~55-column box, so it sits in a sea
  of empty space (see 2.2).
- Root cause: a 5-row block font is too coarse for a diagonal. Block
  characters (`█`) are for filled areas, not for strokes. Claude
  Code's Clawd works because it is a detailed multi-line pixel
  character; a 5-line Z cannot compete on the same technique.

### 2.2 The box is a tall, mostly-empty rectangle [HIGH]

`welcome_box` (`repl.rs:47`) sizes the box to the longest content
line — in practice the `cwd` line (`~/workspace/system/cli-aislop-dot2`,
~40 chars). Result: a ~55-column box whose content is a 6-column
mark and short left-aligned lines. ~30 columns of dead space on the
right of every line. Claude Code's dashboard *fills* its width with
two columns (logo left, feeds right); v1 has one narrow column in a
wide box, so the box draws attention to emptiness.
Fix: cap the width and shorten the cwd (see §5.1).

### 2.3 No visual hierarchy inside the box [HIGH]

Every line is the same colour and weight: `ZenCode CLI 0.1.0` (the
product name — the most important line) looks identical to `model …`
and `cwd …` (metadata). `welcome_box` does not even take a `Theme` —
it is plain text. Claude Code uses bold/colour for the version line
and dim for metadata; Lip Gloss names styles by purpose for the same
reason. Fix: pass the theme in, paint the brand line accent-bold,
labels dim, values plain (see §5.1, §6).

### 2.4 "Welcome back!" is wrong on first run [MEDIUM]

v1 prints it unconditionally. Claude Code shows the full dashboard
only when there is something to show and falls back to a condensed
logo otherwise. ZenCode has the data to do the same:
`list_sessions_for_cwd(home, cwd)` (`session.rs:451`) already exists.
Fix: greet "Welcome back!" only when a previous session exists for
this cwd; otherwise print a neutral line (or nothing — the brand line
already says hello).

### 2.5 Two hint lines back to back [MEDIUM]

The box footer says `/model to switch models · /help for commands`
and the very next `println!` (`run_repl`, after the `welcome_box`
call) says `Type /help for commands, !cmd for shell, @file …`. The
`/help` pointer appears twice in two lines. Fix: keep one hint line
in the box footer and make the long line first-run-only (it teaches
`!cmd`/`@file`/`#note`, which a returning user already knows).

### 2.6 The status line repeats the full model id every turn [MEDIUM]

`status_line` (`repl.rs:16`) prints the full id
(`google/gemini-4-argon:free`, 28 chars) + full cwd + branch before
every prompt. On a narrow terminal it wraps; on the first screen the
model appears three times (box, status line, `/model` output). Fix:
split the id visually — provider in dim, short model name in accent
(`google/` dim + `gemini-4-argon:free` accent) — and middle-truncate
the cwd with `…` when the line would exceed ~80 columns (see §5.4).

### 2.7 The `/model` list is a bare list [MEDIUM]

`format_model_list` (`repl.rs:762`) prints `  N. <full id>` and marks
the current model with a trailing `  *` — ~30 characters from the
number, where the eye never lands. No provider column (the id embeds
it, but the eye must parse the prefix), no context window, no
latency, no free badge. OpenCode's picker shows name + provider +
context + cost + latency for a reason: those are the four facts a
person uses to choose. ZenCode already *has* three of them:
`ModelProbe` (`models.rs:103`) carries `context_window` and
`latency_ms`, and `CatalogModel` (`models.rs:9`) carries
`context_window` — but `available_model_ids` (`repl.rs:740`)
discards everything except the id string. Fix: thread the probe data
through and render columns (see §5.3).

### 2.8 Tool cards: number good, colour and result line weak [MEDIUM]

`card_lines` (`repl_progress.rs:109`) renders
`├─ 1. Bash(cmd)` and `│  -> ok, N lines: …`. The number is good
(it maps to `/expand`) and must stay. Two problems:
- The tool name is uncoloured, so at a glance a tool call looks like
  assistant prose. Claude Code colours the tool line; the eye should
  land on the tool name first.
- The result line reads like a log (`│  -> ok, N lines:`). Claude
  Code uses `└` for the result — a closing corner that visually says
  "this call is done" — and collapses to `+N lines`. v1's
  `(collapsed - /expand {n} for the full output)` is also wordy.
- Inconsistency: the final answer gets a `●` prefix (`print_turn`,
  `repl.rs:519`) but the in-progress assistant text in `card_lines`
  has no marker at all.
Fix: §5.5.

### 2.9 The thinking line is a sentence, not a status [MEDIUM]

`render()` (`repl_progress.rs:313`): `Still working - {tool} has
been running for {N}s...`. It is long, it restates the tool name
(already shown in the card), it has no spinner character, and it
never tells the user how to stop the turn (ZenCode's interrupt is
Ctrl-C; a first-time user does not know that). Claude Code's form is
one verb + `…` + elapsed, accent-coloured, with `(esc to interrupt)`.
Fix: §5.6 — including a small *original* Zen verb list (clean-room:
do not copy Claude's 185-verb list; write our own).

### 2.10 `/todos` is unstyled [LOW]

`repl.rs:1492` prints `[status] content` — raw status strings, no
marker, no checkbox. The v1 doc already specified `●` header +
`└ □` / `└ ☑` items (Claude Code's `● Update Todos` pattern); it was
never implemented. Fix: §5.7. Low priority because `/todos` is rarely
on screen.

### 2.11 Colour table: Light accent is magenta [MEDIUM]

`repl_polish.rs:59-68`: Dark Accent = 94 (bright blue), Light Accent
= 35 (magenta). The same semantic role is blue on dark and magenta
on light — a brand inconsistency, not a theme adaptation. Dark
Prompt = 36 (cyan) vs Light Prompt = 34 (blue) is a legitimate
adaptation (cyan is unreadable on white); the accent flip is not.
Fix: §6.

### 2.12 Box corners are square [LOW]

`┌─┐└─┘` (NormalBorder). Lip Gloss best practice and Claude Code's
dialogs both use rounded corners (`╭╮╰╯`) for emphasis boxes — softer,
more deliberate. One-character change per corner. Fix with §5.1.

## 3. What the research says, distilled

| Source | Lesson for ZenCode (line-based REPL) |
| --- | --- |
| Claude Code LogoV2 | Full dashboard only when there is content; condensed logo otherwise. Two-column fill, never a wide box with one narrow column. Feeds (Recent / What's New) are *data*, not decoration — they exist because the data is cheap to read. |
| Claude Code spinner verbs | One verb + `…` + elapsed. The verb is personality, not information — keep the list short, original, and deterministic per turn (no random flicker). |
| Claude Code tool timeline | `● Tool(args)` + `└` result + `+N lines` collapse. The corner character does the work of a whole sentence. |
| OpenCode picker | A model row answers four questions: name, provider, context, latency/cost. Anything less forces the user to guess. |
| OpenCode home screen | Footer line carries cwd/branch + version + a hint — the *status line* and the *welcome* share one vocabulary. |
| Aider | A REPL with no picker is the failure mode Zen already hit. The picker is the feature; the columns are the polish. |
| Gemini CLI | Session-only vs persisted model switch (`--persist`) is a real user question — answer it in the footer, not in docs. |
| Lip Gloss | Rounded borders for emphasis; adaptive colours per theme; styles named by purpose; TTY check before styling; padding as breathing room. |

## 4. Design principles for v2

1. **The check is the brand.** The product logo is Z-Check (blue
   `#2563EB`, 5.17:1 on white). Its essence is the *check*, not the
   Z. Lead with `✓`, not with a drawn Z.
2. **A bad mark is worse than no mark.** ASCII art only ships if it
   reads instantly on DejaVu Sans Mono (Linux) *and* Cascadia/Consolas
   (Windows PowerShell). Otherwise use type, not art.
3. **One hint line, not two.** Every screen answers "what can I do
   next" exactly once.
4. **Numbers stay.** `/expand N` and `/model N` depend on visible
   numbers. Never trade a number for a prettier marker.
5. **Colour is semantic, never decorative.** Accent = brand/model
   identity. Dim = metadata. Green = ok, red = error. Nothing else
   gets colour.
6. **Data must be cheap or it does not ship.** Recent activity comes
   from a directory listing; What's new is baked at compile time. No
   network, no slow reads on the startup path.

## 5. V2 design, component by component

### 5.1 Welcome box v2

File: `crates/zen-cli/src/repl.rs`, `welcome_box()` (now takes a
`Theme`) + `run_repl` call site. Tests: extend `welcome_box_tests`
(`repl.rs:1906`).

```text
╭────────────────────────────────────────────────╮
│ ✓ zen 0.1.0                                    │
│                                                │
│ Welcome back!                                  │
│   model   gemini-4-argon:free  ·  google       │
│   cwd     ~/workspace/system/cli-aislop-dot2   │
│   branch  dev/repl-model-picker                │
│                                                │
│   Last session 11:24 · 3 turns · 12k tokens   │
│   New in 0.1.0: /model picker lists models     │
│                                                │
│ /model to switch · /help for commands          │
╰────────────────────────────────────────────────╯
```

Rules:
- **Rounded corners** `╭╮╰╯`, border painted `Dim` (or Accent at
  50% — no, keep it simple: `Dim`). One-character change per corner.
- **Brand line**: `✓ zen 0.1.0` — `✓` + `zen` in Accent, ` 0.1.0`
  in Dim. No ASCII-art mark (see §5.2 for the alternative). The
  version is the only thing in accent on this line, so the eye lands
  on the product first.
- **Width cap**: box inner width = `min(longest line, 60)`; cwd is
  middle-truncated with `…` to fit (`~/…/cli-aislop-dot2`). The v1
  bug was letting the cwd set the width; v2 never lets a path set the
  width. Minimum inner width 40 so the box never collapses on short
  content.
- **Greeting**: `Welcome back!` only when
  `list_sessions_for_cwd(home, cwd)` is non-empty; otherwise omit the
  line entirely (the brand line is the greeting).
- **Facts**: labels (`model`/`cwd`/`branch`) in Dim, values plain.
  Model row splits provider: short name in Accent, `· provider` in
  Dim. Branch only when inside a git repo (already available from
  `git_info`, as `status_line` does).
- **Recent activity** (one line, Dim, only when data exists):
  `Last session <HH:MM> · <N> turns · <tokens> tokens`, from
  `list_sessions_for_cwd` + `load_usage` (`session.rs:487`). If the
  counts are unavailable, fall back to just the time. Never block
  startup on this — directory listing only.
- **What's new** (one line, Dim, only when data exists): first bullet
  under `## Unreleased` in `CHANGELOG.md`, truncated to the box
  width. The installed binary has no CHANGELOG next to it, so bake
  the top 2 bullets at compile time in `build.rs` (`include_str!` +
  parse → `const WHATS_NEW: &[&str]`), with an empty fallback. This
  is the only new build-time step; it touches no runtime path.
- **Footer**: one hint line, Dim: `/model to switch · /help for
  commands`. Delete the duplicate long `println!` after the box in
  `run_repl` for returning users; keep it only on first run (no
  session history), where it teaches `!cmd` / `@file` / `#note`.
- **Ephemeral**: keep the `EPHEMERAL MODE` line, painted `Error`
  (it is a warning, not metadata).

### 5.2 The mark — two options, one decision

**Option A (recommended): no art, wordmark + check.**
`✓ zen 0.1.0` as specified in §5.1. Renders identically on every
font (✓ U+2713 is in DejaVu, Cascadia, Consolas, JetBrains Mono).
Zero risk, and it *is* the logo (Z-Check) reduced to its essence.
Cost: nothing.

**Option B (if Zen wants a graphic mark): slash-Z.**
Replace `ZEN_MARK` with a 4-row line-drawn Z using `/` (ASCII slash,
renders everywhere) and box-drawing corners:

```text
┌────┐
     /
   /
└────┘
```

The `/` gives a true 45° diagonal in one character per row — the
thing the block version fails at. Paint the mark Accent. Test on
Windows PowerShell (Consolas/Cascadia) and Linux (DejaVu) before
accepting; if the diagonal looks broken in either, fall back to
Option A. Do **not** use `╱` (U+2571): it is missing from some
Windows console fonts and renders as a tofu box there. Do **not**
use half-blocks (`▄`/`▀`): same coarse-diagonal problem as `█`.

Decision: implement Option A; keep Option B as a one-line swap in the
doc for Zen to choose. Either way, delete the 5-row `█` mark — it is
the single biggest visual liability in v1.

### 5.3 `/model` picker v2

Files: `crates/zen-cli/src/repl.rs`
(`available_model_ids` → return probes, `format_model_list`,
`pick_model` unchanged) + `crates/zen-cli/src/models.rs` (already has
the data: `ModelProbe.context_window` / `latency_ms`,
`CatalogModel.context_window`).

New row type (pure, unit-testable):

```rust
struct ModelRow {
    id: String,                 // full id, e.g. google/gemini-4-argon:free
    provider: String,           // "google"
    short: String,              // "gemini-4-argon:free"
    context_window: Option<u32>,
    latency_ms: Option<u64>,   // None when unknown (catalog fallback)
    current: bool,
}
```

Format (columns aligned, current row marked *leading*):

```text
Models — current: gemini-4-argon:free (google)
▸ 1. gemini-4-argon:free        google   1M ctx    210ms
  2. qwen3-coder:free            qwen     262K ctx  180ms
  3. deepseek-r1:free            deepseek 131K ctx  —
  4. nemotron-3-ultra-550b:free  nvidia   131K ctx  —
  5. north-mini-code:free        cohere   131K ctx  —
Enter a number, name, or unique part — /model 2, /model nemotron
```

Rules:
- **Leading marker**: `▸` in Accent before the number of the current
  row (replaces the trailing `  *`, which sat ~30 chars from the
  number). Number stays — `/model N` and `/expand`-style selection
  depend on it.
- **Columns**: short name (Accent when current, plain otherwise),
  provider (Dim), context humanised (`1M` / `262K` / `131K`, Dim),
  latency (`210ms`, Dim; `—` when the row comes from the static
  catalog, which has no probe). No `free` badge: every row in the
  catalog is free today, so the badge is noise; add it only if a
  non-free model ever appears.
- **Header**: `Models — current: <short> (<provider>)`, current name
  in Accent. Footer: one line with examples (replaces the bare
  `switch with /model <number> or /model <name>`).
- **Session vs default**: borrow Gemini CLI's concept — after a
  switch, print `model set to <id> (this session; /model set --default
  to keep it)` only if a `--default` path exists; if it does not
  exist yet, the footer should *not* promise it. For v2, keep the
  current session-only behaviour and add the sentence to the v2
  follow-up list, not to the picker.
- **Data plumbing**: change `available_model_ids` to return
  `Vec<ModelRow>` (probes when the cache matches `base_url`, catalog
  rows otherwise). `pick_model` keeps taking `&[String]` of ids or is
  generalised to `&[ModelRow]` — either way its tests stay green.
- **Narrow terminals**: if the row would exceed 80 columns, drop the
  latency column first, then the context column. Never wrap a row.

### 5.4 Status line v2

File: `crates/zen-cli/src/repl.rs`, `status_line()` (signature
unchanged: theme, model, cwd).

```text
google/ gemini-4-argon:free  ·  ~/…/cli-aislop-dot2  ·  dev/repl-model-picker
└─dim─┘ └──── accent ────┘   └────────────── dim ───────────────┘
```

Rules:
- **Split the id**: provider in Dim, short name in Accent. The eye
  lands on the model, the provider is still there for disambiguation.
- **Middle-truncate the cwd** with `…` when the whole line would
  exceed 80 columns (keep the first `~/` and the last two path
  segments). Branch stays, Dim.
- **No new fields in v2**: context % and cost belong here eventually
  (Claude Code's status bar, OpenCode's composer readout), but they
  need a cheap, always-correct source; `UsageStats` + the model
  context window can supply it — put it in the follow-up list, not
  in v2, so the line stays a one-line change.
- Keep printing before each prompt (that *is* the REPL's status bar).

### 5.5 Tool cards v2

File: `crates/zen-cli/src/repl_progress.rs`, `card_lines()` +
`tool_result_summary()`; tests in the same file's `mod tests`.

```text
● 1. Bash(ls -la)
└ ✓ 12 lines · total 48
● 2. Read(src/main.rs)
└ ✗ error: file not found
● 3. Edit(src/lib.rs)
└ ✓ 3 lines · +N lines (/expand 3 to show)
```

Rules:
- **Tool line**: `● {n}. {Tool}({args})` — `●` in Accent, tool name
  plain (bold if `paint` gains a bold role — see §6, optional),
  args in Dim. Number stays (maps to `/expand N`).
- **Result line**: `└` + `✓`/`✗` + summary. `✓` in Success green,
  `✗` in Error red. Collapsed form: `└ ✓ 12 lines · <first line>…
  (+N lines — /expand {n})`. Drop the `-> ok,` log prefix and the
  wordy `(collapsed - /expand {n} for the full output)`.
- **Assistant in-progress text**: prefix with `● ` in Accent in
  `card_lines` (the Assistant arm), so it matches `print_turn`'s
  `● ` on the final answer. One marker, everywhere.
- **Nested calls**: keep the existing 2-space indent for
  `parent_tool_use_id.is_some()`.

### 5.6 Thinking indicator v2

File: `crates/zen-cli/src/repl_progress.rs`, `render()`,
`AgentEvent::ToolProgress` arm (currently line ~313).

```text
Composing… Read · 12s · Ctrl-C to stop
```

Rules:
- **Format**: `{Verb}… {Tool} · {elapsed}s · Ctrl-C to stop` —
  verb in Accent, rest Dim. When no tool is running, omit the tool:
  `{Verb}… {elapsed}s · Ctrl-C to stop`.
- **Verbs (original, Zen-flavoured — clean-room: do not copy
  Claude's list)**: `Thinking`, `Composing`, `Weaving`, `Polishing`,
  `Digging`, `Reading`, `Writing`, `Searching`, `Checking`,
  `Zening`. Pick deterministically per turn (e.g. hash of the tool
  name or the turn index mod list length) so the word does not flicker
  randomly between heartbeats; one verb per turn.
- **Interrupt hint**: `Ctrl-C to stop` is the point — ZenCode's
  interrupt key was never shown anywhere. Keep it short; it is the
  equivalent of Claude Code's `(esc to interrupt)`.
- **Elapsed**: keep the real elapsed seconds from the heartbeat
  (already available as `elapsed_ms`).

### 5.7 `/todos` v2

File: `crates/zen-cli/src/repl.rs`, the `"/todos"` arm (~line 1492).

```text
● Todos
└ ☑ fix the flaky test
└ □ write the v2 doc
└ □ open the PR
```

Rules: header `● Todos` (Accent `●`), one line per item with `└`
+ `☑` (done, Success green) / `□` (pending, plain). Empty list
stays `(no todos)`. Map `TodoItem.status` to the checkbox; do not
change the status strings anywhere else.

### 5.8 `/help` grouping (small, high value)

File: `crates/zen-cli/src/repl.rs` (`/help` handler) +
`crates/zen-cli/src/commands.rs` (`slash_command_catalog`).

Group the flat list under four Dim headers instead of one long list:

```text
Session & model:   /help /model /cost /status /resume /exit …
Context:           /clear /compact /context /memory /memories …
Code & review:     /diff /review /commit /undo /plan /ci-fix …
Tools:             /agents /mcp /hooks /skills /todos /loop …
```

Rules: groups are display-only (derive them from the existing
catalogue + a small static group map in `commands.rs`); command
names and descriptions do not change; column alignment stays
`{:<16}` within a group. If grouping turns out to be more than a
small diff, ship v2 without it and put it in the follow-up list —
the welcome/picker/cards matter more.

## 6. Colour table v2

`Theme::paint` (`repl_polish.rs:53`) currently takes a single `u8`
ANSI code (`\x1b[{code}m`). A 256-colour accent needs `38;5;N`, which
does not fit a `u8`. Change `paint` to take a small `Sgr` value
(e.g. an enum `Sgr::Code(u8)` / `Sgr::Color256(u8)` rendered as
`\x1b[{code}m` / `\x1b[38;5;{n}m`) — one focused refactor, tests in
`repl_polish.rs` `mod tests` updated accordingly. `Theme::None` and
`NO_COLOR`/`TERM=dumb` behaviour are untouched.

Nearest xterm-256 to Zen blue `#2563EB` = rgb(37, 99, 235), computed
by Euclidean distance over the 256-colour cube (verified by script,
2026-11-06): codes **26** (`#005fd7`) and **27** (`#005fff`) tie at
d²=1785. Pick **27** — the brighter of the two, closest in vividness
to the brand blue. (For reference, code 69 `#5f87ff` is *not* the
nearest; do not use it.)

| Role | Dark (v1 → v2) | Light (v1 → v2) | Note |
| --- | --- | --- | --- |
| Prompt | 36 cyan → keep | 34 blue → keep | Prompt already differs per theme legitimately. |
| Accent | 94 bright blue → **256:27** (`#005fff`) | 35 magenta → **34 blue** | Fixes the magenta inconsistency; 27 is the verified nearest to `#2563EB`. On a terminal without 256-colour support the escape degrades to the terminal's fallback — acceptable; `NO_COLOR`/`TERM=dumb` still disables all colour. |
| Error | 31 → keep | 31 → keep | |
| Success | 32 → keep | 32 → keep | |
| Dim | 90 → keep | 90 → keep | Bright black reads as grey on both; verify on a light terminal during eyeballing. |
| (new) Bold | — → **optional** `1` combined with Accent for the brand line and tool names | same | Only if the `Sgr` refactor makes it cheap; otherwise skip — colour alone carries the hierarchy. |

## 7. v1 vs v2 comparison

| Area | v1 (PR #56) | v2 (this doc) |
| --- | --- | --- |
| Mark | 5-row `█` block Z, monochrome, reads as `2` | No art: `✓ zen 0.1.0` (Option A); slash-Z as an optional swap (Option B) |
| Box corners | Square `┌─┐└─┘`, uncoloured border | Rounded `╭╮╰╯`, border Dim |
| Box width | Longest line (usually cwd) sets it — wide, empty | Capped at 60 inner, min 40; cwd middle-truncated |
| Box colour | None (plain text, no theme) | Brand line Accent, labels Dim, values plain; ephemeral line Error |
| Greeting | `Welcome back!` always | Only when a previous session exists for this cwd |
| Hint lines | Two (box footer + long println) | One in the box; long line first-run-only |
| Recent / What's new | Absent | One Dim line each when data exists (dir listing / compile-time CHANGELOG) |
| `/model` rows | `N. <full id>` + trailing `*` | `▸ N. <short>  <provider>  <ctx>  <latency>`; leading marker; columns |
| `/model` data | Ids only (probe data discarded) | `ModelRow` threads `context_window`/`latency_ms` through |
| Status line | Full id accent + cwd + branch | Provider Dim + short name Accent; cwd middle-truncated at 80 cols |
| Tool line | `├─ N. Tool(args)`, uncoloured | `● N. Tool(args)`, `●` Accent, args Dim |
| Result line | `│  -> ok, N lines:` | `└ ✓/✗ <summary>`; collapse `(+N lines — /expand N)` |
| Assistant text | `●` only on final answer | `●` on in-progress text too |
| Thinking line | `Still working - {tool} has been running for {N}s...` | `{Verb}… {Tool} · {N}s · Ctrl-C to stop`, original Zen verb list |
| `/todos` | `[status] content` | `● Todos` + `└ ☑/□` |
| `/help` | Flat list | Grouped under 4 Dim headers (if small; else follow-up) |
| Accent colour | Dark 94, Light 35 (magenta) | Dark 256:27, Light 34 (both blue) |

## 8. Implementation plan (ordered, one PR or stacked PRs)

1. **Colour table** (`repl_polish.rs`): `Sgr` refactor + v2 table +
   tests. No visible change except Light accent magenta → blue and
   Dark accent 94 → 256:27. Verify on a real dark and a real light
   terminal.
2. **Welcome box v2** (`repl.rs` `welcome_box` + call site): theme
   parameter, rounded corners, width cap + cwd truncation, split
   model row, greeting conditional on `list_sessions_for_cwd`,
   Recent line, What's-new line (needs the `build.rs` bake — do that
   first if it is not trivial), single hint line. Delete `ZEN_MARK`.
   Extend `welcome_box_tests`.
3. **`/model` picker v2** (`repl.rs` + `models.rs`): `ModelRow`,
   column format, leading `▸`, narrow-terminal column dropping.
   Update/extend the `model_picker_tests`.
4. **Tool cards + thinking line** (`repl_progress.rs`): `●` tool
   lines, `└ ✓/✗` results, `●` on assistant text, verb line.
   Update the `mod tests` expectations (`Still working` string goes
   away — update the test at old line ~402).
5. **`/todos` + `/help` grouping** (`repl.rs`, `commands.rs`):
   smallest diffs last.
6. Eyeball on a real terminal (Linux + Windows PowerShell) at 80
   and 120 columns, Dark and Light, and with `NO_COLOR=1` (must be
   byte-identical to plain text). Then PR.

## 9. Acceptance criteria

- `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` all pass;
  mock eval stays at 100% (`cargo run --locked -q -p zen-cli --
  eval --mock --min-pass-rate 1.0`).
- Eyeballed on a real terminal: welcome box (first run + returning,
  80-col + 120-col, Dark + Light, `NO_COLOR=1`), `/model` list and
  switch, a tool-call turn with a collapsed result, the thinking
  line, `/todos`, `/help`.
- No new dependency. No change to the stream-json contract. No change
  to `main` except through a reviewed PR (never self-merge).
- The `█` mark is gone. The Light accent is blue, not magenta. The
  box never exceeds 60 inner columns. The `/model` list shows
  provider, context and latency where the data exists.

## 10. What NOT to do (clean-room + scope)

- Do not copy Claude Code's Clawd mascot, its orange, its exact
  wording, its 185-verb spinner list, or any of its code/prompts/tool
  descriptions. The Zen verb list in §5.6 is written from scratch.
- Do not copy OpenCode's/Gemini CLI's fullscreen TUI, sidebar, or
  modal overlays. ZenCode stays a line-based REPL; the status line
  *is* the status bar.
- Do not add a TUI framework (ratatui, Bubble Tea, Ink, OpenTUI) or
  any new crate for this.
- Do not add network calls, telemetry, or analytics to the welcome
  screen. Recent activity is a local directory listing; What's new is
  baked at compile time.
- Do not promise the session-vs-default `/model` distinction, context
  % in the status line, or a persistent model default in v2 — those
  are follow-ups (see §8 + §5.3/§5.4 notes), not v2 scope.
- Do not "fix" the design by adding more boxes, more colours, or more
  lines. v2 removes as much as it adds: one mark deleted, one hint
  line deleted, one colour (magenta) deleted.

## 11. Sources consulted (design language only)

- Claude Code LogoV2 / dashboard / design-system write-ups
  (DeepWiki pages on `LogoV2.tsx`, `FeedColumn`, `ListItem`,
  `ThemedText`); spinner-verb lists (gist by nitrocode, deepakness,
  waka-ds, XDA) — vocabulary *described*, not copied.
- OpenCode TUI skill reference (screen layout, status bar, model
  picker fields) and the `tui-patterns` OpenCode layout analysis
  (home screen, composer, footer line).
- Aider terminal guide (BetterStack) and the agent-UI competition
  scan (chump PRODUCT-011) — Aider's CLI-only model switching as the
  negative example.
- Gemini CLI settings schema (window-title status icons, `? for
  shortcuts`, `/model set --persist`) and the 9-command comparison
  across Claude Code / Codex / Gemini CLI / OpenCode (dev.to).
- Charmbracelet Lip Gloss README + the gh-aw terminal-stylist
  analysis (rounded borders, adaptive colours, purpose-named styles,
  TTY checks) and the snappy TUI modernisation plan.
- xterm-256 colour cube (Wikipedia / xterm.codes): nearest to
  `#2563EB` computed by script, not guessed — codes 26/27 tie.
- ZenCode repo itself: `repl.rs` (`status_line`, `ZEN_MARK`,
  `welcome_box`, `format_model_list`, `pick_model`, `print_turn`,
  `/todos` arm), `repl_polish.rs` (`Theme::paint`, `ColorRole`),
  `repl_progress.rs` (`card_lines`, `render` ToolProgress arm),
  `models.rs` (`ModelProbe`, `CatalogModel`), `session.rs`
  (`list_sessions_for_cwd`, `load_usage`), `CHANGELOG.md`
  (`## Unreleased`).
