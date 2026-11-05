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
| Repo to prompt | the system prompt | file contents, READMEs, comments, MCP and web fetch results (no prompt text marks them as data yet, see open question 7) |
| CLI to network | the configured provider endpoint | provider responses, MCP server results, web fetch results |
| Skill install | ed25519-signed ZenCode skills | community skills (hash-pinned after your approval) and unverified skills (sandboxed: read-only, no network) |
| Source to binary | this repo at a reviewed commit | whatever `install.sh` / `zen update` fetches from the branch it is told to build |

## Assets

1. Provider API keys and tokens.
2. Files outside the working directory and approved roots.
3. The user's shell (commands run with the user's own privileges).
4. Session data and exports (may contain code and secrets pasted into prompts).
   The folder trust store `~/.zen/trusted-folders.json` belongs here too (see open question 6).
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
| CI: secret scan | gitleaks full history, redacted report, reviewed baseline in `.gitleaksignore`; the gitleaks binary itself is downloaded without a sha256 check (open question 8) | `.github/workflows/ci.yml`, `.gitleaks.toml` |
| Locked builds (partial) | `--locked` is used only on the eval run (`cargo run --locked`) and on `cargo install cargo-zigbuild --locked`; fmt, clippy, test and the release build steps do not pass it, so they do not enforce `Cargo.lock` | `.github/workflows/ci.yml` |
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

1. Secret storage. Resolved by Leader ruling (msg 5734): the docs were wrong, not the code. Keys come from
   environment variables only (`ZEN_API_KEY`, `OPENAI_API_KEY`, or the variable named by `apiKeyEnv`).
   `docs/faq.md` and `docs/threat-model.md` now say so. OS keyring support, if wanted, is a separate
   roadmap item.
2. Install source is unpinned. `install.sh` builds the tip of `main` by default (`ZENCODE_BRANCH`),
   and `zen update` rebuilds from source. There is no tag, commit or checksum check.
3. No binary releases yet, so no SHA256SUMS and no signature to verify. The release checklist
   requirement (SHA256SUMS per artifact) is in PR #46, not merged yet; it does not exist on `main`
   647fcc0. Link to CONTRIBUTING.md once #46 lands.
4. No OS-level sandbox for the Bash tool. Verified: `crates/zen-tools` and `crates/zen-agent` contain no
   bubblewrap, bwrap, unshare, namespace, seatbelt, pledge, landlock or seccomp. Bash runs with the user's
   privileges, gated only by the permission layer. Known limit; follow-up is a separate work item, owner to be named by Leader.
5. Session files and exports on disk (verified against `main` 647fcc0):
   - Session files are written with `OpenOptions` and no `set_permissions` (`crates/zen-cli/src/session.rs:227`,
     `:362`), so mode follows umask, typically 0644, not 0600.
   - Only the REPL history file gets 0600, Unix only, best effort (`crates/zen-cli/src/repl_input.rs:229`).
   - `zen export` redaction is off by default; it needs `--redact`.
   Decision (Leader, msg 5770): YES. Session files must be 0600 because they hold conversation content that may include secrets.
   Requirement recorded here; the code change is a separate work item and does not block merging this doc.
6. Folder trust prompt is too thin. The prompt is two lines on stderr: `This folder has project config that can run commands: <list>` (`crates/zen-cli/src/lib.rs:551-554`), then `Trust this folder? [y/N]` (`crates/zen-cli/src/lib.rs:555`).
   The list (`gated_project_config`, `crates/zen-ext/src/trust.rs:147`) names only file paths and config keys: for `.zen/settings.json` and `.zen/settings.local.json`, the keys `hooks`, `permissions.allow`, `permissions.additionalDirectories`, `env`, `verify`, `baseUrl`, `apiKeyEnv` and `allowPrivateNetwork` (`trust.rs:89-123`); `.mcp.json` and `.zen/mcp.json` by file name only; `.zen/hooks.json` and `.zen/hooks.local.json` as `hooks`.
   It does not show their contents (which hook commands, MCP servers, allow rules or env values will load), so a yes is not an informed one.
   Decision (Leader, msg 5770): the prompt MUST list what will load (which hooks, MCP servers, allow rules and env vars)
   before asking. A yes without seeing that content is not an informed decision. Prompt change is a separate work item.
   The trust store `~/.zen/trusted-folders.json` (`crates/zen-ext/src/trust.rs:17`) is also an asset: anyone who can
   write it makes a hostile folder trusted, and its project config then loads.
7. Prompt injection through MCP and web fetch results. Verified: no sentence in `crates/zen-cli/src/system_prompt.rs`
   or `crates/zen-agent` tells the model that MCP results, web fetch output or file contents are untrusted data
   rather than instructions; EVIDENCE_LABELING treats tool results as the model's own evidence. No technical
   separation is documented either. Decision (Leader, msg 5770): YES, add a sentence stating that MCP, web and file results
   are untrusted data, not instructions (basic prompt-injection defense). The system prompt change is a separate work item.
8. gitleaks is installed without verification: `ci.yml:66-69` downloads the gitleaks 8.30.1 tarball with curl and
   does not check its sha256. Fix (pin sha256) is approved in msg 5716 item (3) and goes on its own branch;
   it does not block this PR.

## Hardening checklist

The Z Red hardening checklist is a team draft and is not in this repository yet. When it lands,
link it here and map each of its items to a row above or to an open question.
