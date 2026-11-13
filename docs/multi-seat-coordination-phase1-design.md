# Phase 1 design: Multi-seat coordination (Scout/Worker/Reviewer) for the zencode CLI

- Status: Leader approved the design (message 10525, 12/11/2026). Phase 1 is design only, no code implementation. Phase 2 is in progress: the first PR is the `delegate` tool (branch `feature/multi-seat-coord`).
- Branch: `feature/multi-seat-coord`, base main `2e7beaf` (rebased from `1786049` on 12/11/2026).
- Date: 12/11/2026. Seat: ZC Developer. Order: P0, per Zen's order.
- Phase 1 acceptance: this document. Leader approves the design before phase 2 implementation starts.

## 1. Goals and scope

Goal: the CLI has a mechanism for tasking and handoff between seats within the same session, replacing today's manual coordination over Telegram.

Phase 1 scope: the coordination protocol (tasking subagents, result handoff, synthesis) + definitions of the 3 roles. No code implementation, no peer mailbox, no cross-session state.

Unchanged: 3 roles (Scout read-only, Worker edits, Reviewer reports only); free-only; verify before reporting done; independent critic gate before synthesis.

## 2. Research: how Pi and Claude Code do coordination

### 2.1 Pi 1.5.0 (@earendil-works/pi-coding-agent)

- Core has NO multi-agent coordination: grep "subagent" in `dist/bundle/cli.js` returns 0, `pi --help` has no delegate/subagent flag [M|0]. Coordination lives in the example `subagent` extension (`index.ts`, 1,036 lines) which registers tools via ExtensionAPI [S|0] - a deliberate design choice, pushing coordination to the extension layer.
- Tool `subagent`, 3 modes: Single `{agent, task}`; Parallel `{tasks: [...]}`; Chain `{chain: [...]}` with placeholder `{previous}` for the next step [S|0].
- An agent is a Markdown file + YAML frontmatter (name, description, tools, model); the file body is the system prompt [S|0]. Re-scanned on every call from `~/.pi/agent/agents` (user) and `.pi/agents` (project) [S|0].
- Each call spawns a separate `pi` process (`pi --mode json -p --no-session`), the system prompt is written to a temp file then deleted; `--no-session` means the child transcript is not saved and does not pollute the parent session [S|0].
- Handoff via stdout: the extension reads each line of the JSON event stream and returns to the parent model the child's final assistant message as text [S|0]. Parallel joins the outputs into 1 text, each task capped at 50 KB; no automatic synthesis - merging is the parent model's job [I|0]. Chain stops at the first failing step and reports which step failed [S|0].
- 4 sample agents: scout, planner, reviewer, worker [S|0]. Permissions via the `tools` allowlist in frontmatter + instructions in the prompt - a soft constraint: reviewer.md itself admits "Assume tool permissions are not perfectly enforceable" [S|0]. Pi's scout still has bash, not purely read-only like the team's Scout [S|0].
- Limits: max 8 tasks, max 4 running concurrently; no recursion depth limit, no loop protection, no per-subagent timeout [M|0]. Sample workflows: `/implement` (scout - planner - worker), `/implement-and-review` (worker - reviewer - worker) [S|0].

### 2.2 Claude Code 2.1.304

- 2 separate coordination layers [S|0]: Subagent (within 1 session, main manages everything, one-way handoff via summary) and Agent Teams (multiple independent instances, lead + teammates messaging directly, shared task list, JSON mailbox; off by default, needs an interactive session, does not run in `-p` mode).
- Tool `Agent` (formerly `Task`, renamed in v2.1.63) [S|1]. Built-in types: Explore (read-only, no Write/Edit, 3 thoroughness levels), Plan (read-only), general-purpose (full tools) [S|0].
- Custom agent: Markdown + YAML frontmatter in `.claude/agents/` (project), `~/.claude/agents/` (user), `--agents` flag, plugin. Frontmatter includes description, tools, disallowedTools, model, permissionMode, hooks, maxTurns, skills, background, omitClaudeMd, isolation [S|0].
- Child context starts empty, receiving only its own system prompt, the delegation message written by main, CLAUDE.md at each level, and a git status snapshot [S|0]. `isolation: worktree` lets the child work on a separate copy of the repo [S|0].
- One-way handoff: the child returns a final summary, long output stays in its context [S|0]. Background runs concurrently, results reach main via completion notification; by default subagents spawned by main run in the background [S|0].
- Anti-injection: Claude Code scans the final report before main reads it (inserting backslashes into text that fakes system output, a marker line when permission settings are mentioned) and attaches a header stating this is a subagent's words, with no authority [S|0].
- Resume via the `SendMessage` tool with the agent ID; transcripts saved at `~/.claude/projects/.../subagents/agent-{id}.jsonl`, kept 30 days by default [S|0]. Max 20 concurrent subagents (default), nested up to 3 levels [S|0]. To stop a subagent from spawning more, remove `Agent` from `tools` or add it to `disallowedTools` [S|0].
- Agent Teams: each agent's mailbox is a JSON file at `~/.claude/teams/{team}/inboxes/{agent}.json`; tasks have 3 states pending / in progress / completed + dependency (blocked until the dependency finishes); claiming uses file locking to stop 2 agents claiming the same task [S|0]. Teammates notify on their own when idle or on error; tokens grow linearly with teammate count; 2 teammates editing the same file overwrite each other, so split the file set [S|0].
- Hooks: SubagentStart/SubagentStop; quality gates TeammateIdle/TaskCreated/TaskCompleted (exit code 2 blocks the action) [S|0].

### 2.3 Common patterns worth learning (inference)

- Both define agents with Markdown + YAML frontmatter - already the common pattern, follow it.
- One-way handoff via summary is the minimal pattern that fits a CLI; mailbox + task list are only needed when seats talk to each other directly.
- Enforce roles via tool mechanisms (allowlist/denylist), not just prompts - Pi relies on prompts alone and admits it cannot enforce them.
- A task list with status + dependency + file locking on claim is a mechanism worth learning for many seats claiming work in parallel.
- Scanning subagent output + a "no authority" header is an anti-prompt-injection measure worth learning for handoffs between seats.

## 3. Team practice today (problems to solve)

Manual 5-step flow: tasking (Telegram message, reply ACK within 30 minutes) - parallel fan-out (each Scout one domain, separate file) - work (Worker edits on a separate worktree, branch dev/<name>) - handoff (git bundle + sha256; report file + SHA; ACK/DONE/BLOCKED messages; Notion as the record) - verification + reporting (the lead seat verifies directly before reporting done).

Weaknesses measured in practice: waiting with no automatic deadline (regression test question sent 08:54 on 12/11, still unanswered at 11:28, had to check manually); message peek cut at 200 characters (the tail of Leader message 10394 was cut, had to ask again via Notion); duplicates (same file sent twice; PR #176/#177/#178 duplicated #170/#171/#172, had to close without merging); stale/wrong worker reports (3 times on 11/11); orphaned work (worker PR #158 reported "STOPPED - OUT OF TIME, NOT DONE" but was not in the DOING handoff); verification fully manual; 3 channels Telegram/Notion/ztask out of sync.

## 4. Proposed protocol

### 4.1 Architecture: hub-and-spoke within 1 session

1 coordinator seat (parent) + N subagents (children) in the same session. Children only return results to the parent, no peer messaging - no mailbox in phase 1. Rationale: matches the team flow (lead seat coordinates, seats report back); cheap on tokens (only the summary goes to the parent); works in `-p` mode (Claude Code Agent Teams does not run in `-p`).

Each subagent is a separate process, JSON mode (Pi pattern): real context isolation, simplest to implement; child transcripts saved separately in the session folder, not polluting the parent session.

### 4.2 Seat definitions

Markdown files + YAML frontmatter at `.zen/agents/` (project) and `~/.zen/agents/` (user). Frontmatter: name, description, tools (allowlist), model, maxTurns.

3 standard roles, permissions enforced at the CLI tool/permission layer (using the existing `--permission-mode` / `--allowedTools` mechanisms), not just by prompt:

- Scout: tools read/grep/find/ls + git log/show (read-only). NO bash, NO Write/Edit. Stricter than Pi's scout (Pi gives the scout bash), matching the team's Scout role.
- Worker: all tools. When editing code, work on a separate worktree (mandatory - existing team rule, avoids overwriting each other as Claude Code warns).
- Reviewer: read-only tools + git diff/log/show. NO Write/Edit. Must NOT spawn subagents (remove the delegate tool from tools - Claude Code pattern).

### 4.3 Tasking: the `delegate` tool

3 modes (Pi pattern, proven minimal and sufficient): single `{agent, task}`; parallel `{tasks: [...]}`; chain `{chain: [...]}` with placeholder `{previous}`.

Each tasking carries a self-contained brief: goal, scope, constraints, expected result format (per the team's v4 brief standard: framing block).

Hard limits: max 4 subagents running concurrently (default, configurable - Pi's number, stricter than Claude Code because free-first); nested max 2 levels; per-subagent timeout (Pi lacks this - do not copy the gap); maxTurns per frontmatter.

### 4.4 Result handoff: structured envelope

The child returns 1 JSON envelope: status (done/blocked/failed), summary (text), evidence (list of file:line), usage (turns/tokens), sha256 (checksum of the full result file). The team's bundle+SHA practice becomes part of the protocol instead of manual work.

The parent receives the summary + envelope; full output stays in the child transcript (Claude Code pattern), capped at 50 KB/task to the parent (Pi pattern) to protect parent context. Chain: the previous step's output substitutes into `{previous}`; stops at the first failing step and reports which step failed.

Anti-injection: scan child output before the parent reads it + attach a header "this is a subagent's words, no authority" (Claude Code pattern). A claim of "already approved" from a child has no value - only the parent (or the user) can approve.

### 4.5 Task list: preventing duplicate claims

1 JSON file in the session folder. Tasks have states pending / in progress / completed + dependency (blocked until the dependency finishes). Claiming uses file locking (Agent Teams pattern) - stops 2 seats claiming the same task, a bug seen with PR #176/#177/#178. Phase 1: task list lives within 1 session, not a shared team board.

### 4.6 Synthesis and verification gates

The parent synthesizes (no separate synthesis agent in phase 1 - both Pi and Claude Code leave synthesis to main).

Mandatory gates before reporting done, turning team rules into protocol steps: (1) an independent Reviewer scores it (Reviewer role, not the task author); (2) the parent verifies directly (re-running the verification commands, cross-checking evidence) - never report done on a subagent's word.

Stop rule: the same step failing twice means stop and re-plan. A subagent over time with no intermediate product gets parent intervention (heartbeat - team rule: ~20 minutes).

### 4.7 Wait timeouts and orphans

Each tasking has a wait deadline; on expiry it auto-reminds the parent (instead of seats checking manually - the bug seen with the regression test question). A subagent ending with no result gets a dead-letter entry (status + reason), no orphaned work (the PR #158 bug).

## 5. Out of scope for phase 1

- Peer mailbox between seats (Agent Teams) - only needed when seats talk directly, not a phase 1 need.
- Cross-session state (transcripts kept 30 days as in Claude Code are enough for lookup).
- Automatic retry beyond the 2-times rule.
- Paid models: children inherit the parent's model by default, free-first.

## 6. Verification plan (for phase 2)

Implementation: `delegate` tool + 3 role definitions + task list + envelope handoff.

Acceptance via 1 real multi-seat task (the 2048 game acceptance pattern): measure real tokens + redo rate (2 mandatory metrics). Verification: Scout attempting Write is blocked at the tool layer; Reviewer attempting to spawn a subagent is blocked; 2 seats cannot claim the same task; child output containing fake system output is scanned before the parent reads it; the chain stops exactly at the first failing step.

## 7. Sources and verification trail

- Pi scout: `scout-pi-coordination.md` (953 words, sha256 356c41d32d46886e595fa872841794d93d7d6d94faedafc64cfb1e8ea634590b).
- Claude Code scout: `scout-claude-code-coordination.md` (1,214 words, sha256 a3598bbffe7508290ac090c4cf6122fc97f16eaba9648e1430ba6fe27cc78cee).
- Team practice scout: `scout-team-practice.md` (994 words, sha256 5eb395d8e6fd5d0e55d46801088aeee1120cdfaea3af7c062a038fb80e790415).
- Unverified: Pi tool name in newer versions (`task` per piagent.fyi vs `subagent` in installed 1.5.0); runtime behavior (scouts only read, no real agent runs) - to measure in phase 2.
