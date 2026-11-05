# Security model - ZenCode CLI

Status: draft for review. Z Red reviews this as critic once it is a PR.
Scope: the `zen` CLI (crates `zen-cli`, `zen-agent`, `zen-core`, `zen-tools`, `zen-ext`, `zen-mcp`).
Not in scope: Desktop and Sen apps, model providers' servers.

This document is the map: trust boundaries, assets, attackers, controls that exist today,
what is out of scope, and what is still open. It complements, and does not replace,
[docs/threat-model.md](docs/threat-model.md) (what scanners should test, severity rubric)
and [SECURITY.md](SECURITY.md) (how to report). Every control below names where it is documented
or enforced; a control with no such pointer is listed under open questions, not here.

## Trust boundaries

| Boundary | Trusted side | Untrusted side |
| --- | --- | --- |
| Model to tools | the permission layer | tool calls and shell commands proposed by any model, local or remote |
| Repo to config | `~/.zen/` (always loads) | project `.zen/settings*.json`, `.mcp.json`, hooks, project commands and skills (load only in a trusted folder) |
| Repo to prompt | the system prompt | file contents, READMEs, comments (data, not instructions) |
| CLI to network | the configured provider endpoint | provider responses, MCP server results, web fetch results |
| Skill install | ed25519-signed ZenCode skills | community skills (hash-pinned after your approval) and unverified skills (sandboxed: read-only, no network) |
| Source to binary | this repo at a reviewed commit | whatever `install.sh` / `zen update` fetches from the branch it is told to build |

## Assets

1. Provider API keys and tokens.
2. Files outside the working directory and approved roots.
3. The user's shell (commands run with the user's own privileges).
4. Session data and exports (may contain code and secrets pasted into prompts).
5. Integrity of the stream-json contract that Desktop and Sen parse.
6. Integrity of what gets installed and what CI builds.

## Attackers

- A malicious or compromised model or provider endpoint (crafted tool calls, crafted responses).
- A hostile repository (prompt injection in files, malicious project config, hooks, MCP servers).
- A malicious skill or plugin author.
- A malicious dependency or a tampered install source (supply chain).
- Another local process or user on a shared or CI machine reading config, sessions or logs.
- Not an attacker here: someone who already runs code as the same user (see out of scope).

## Controls that exist today

| Control | What it does | Where |
| --- | --- | --- |
| Permission modes and rules | allow, ask, deny per tool; plan mode refuses writes up front; project deny rules always apply | README (Plan mode, Folder trust), `crates/zen-tools` |
| Folder trust | project hooks, `permissions.allow`, `additionalDirectories`, `env`, `verify`, provider `baseUrl` and `apiKeyEnv`, MCP servers and project `allowed-tools` load only in a trusted folder; headless never asks | README "Folder trust" |
| Skill trust tiers | verified (ed25519), community (hash-pinned, no auto-update), unverified (sandboxed); install shows the permission diff and needs `--yes` | README "Skills", `docs/skills.md`, `docs/skill-registry.md` |
| Hooks | PreToolUse validation; PostToolUse can replace what the model sees | `docs/hooks.md` |
| Export redaction | `zen export --redact` | SPEC.md, `docs/session-format.md` |
| Atomic file writes | writes go through a temp file and rename | `crates/zen-tools/src/atomic.rs` |
| Account-safety pacing | backoff, no retry on billing errors, versioned User-Agent | `docs/account-safety.md` |
| CI: dependency scans | `cargo audit` (RustSec), OSV-Scanner, `cargo deny` | `.github/workflows/ci.yml`, `deny.toml` |
| CI: secret scan | gitleaks full history, redacted report, reviewed baseline in `.gitleaksignore` | `.github/workflows/ci.yml`, `.gitleaks.toml` |
| Locked builds | `cargo ... --locked` in CI | `.github/workflows/ci.yml` |
| Human gate for sensitive files | workflows, installer and release scripts, permissions, secrets, auto-update, new network hosts need a person | REVIEW.md |

## Out of scope

- Attacks that need local code execution as the same user (the CLI has that level of trust).
- Persuading the user to approve a malicious command. Bypassing the approval UI is in scope; the user's choice is not.
- Vulnerabilities in model providers' servers.
- Denial of service by exhausting the user's own API quota.
- Desktop and Sen apps, which have their own models.

## Open questions

Each one is a gap between what a document says and what was verified, or a risk with no control yet.
Checked against `main` at 647fcc0.

1. Secret storage. `docs/faq.md` says `zen auth login` stores keys in the OS keyring, and
   `docs/threat-model.md` lists keyring use and 0600 fallback as in scope. A search of `crates/`
   finds no keyring dependency and no `zen auth` subcommand in `cli.rs`; keys are read through
   `apiKeyEnv`. Which is the truth? The docs or the code need to change.
2. Install source is unpinned. `install.sh` builds the tip of `main` by default (`ZENCODE_BRANCH`),
   and `zen update` rebuilds from source. There is no tag, commit or checksum check.
3. No binary releases yet, so no SHA256SUMS and no signature to verify. The release checklist
   gets this as a requirement (see CONTRIBUTING.md).
4. OS-level sandbox for the Bash tool: not verified in this draft. State whether one exists, per platform.
5. Session files and exports on disk: file permissions and redaction defaults are not documented.
6. Hooks run commands from trusted project config. Is a trusted-folder prompt shown with enough
   detail (which hooks, which MCP servers) for an informed yes?
7. Prompt-injection through MCP and web fetch results: the model is told they are data; no
   technical separation is documented.

## Hardening checklist

The Z Red hardening checklist is a team draft and is not in this repository yet. When it lands,
link it here and map each of its items to a row above or to an open question.
