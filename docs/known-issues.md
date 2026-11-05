# Known issues

Each entry has the same six fields so it can be scanned quickly:
**Where** it shows up, **What you see**, **What is not affected**, **Tip** (the fast workaround),
**Why** it happens, and **What to do**. Run `zen doctor` (or `zen doctor --json`) first: it prints the
resolved configuration, folder-trust state and environment, and most entries below are explained by it.

Remove an entry when it is fixed, and say so in the PR.

## KI-1: No binary release, install builds from source

- **Where:** `install.sh`, `install.ps1`, `zen update`.
- **What you see:** `error: cargo not found.` or a build that takes several minutes.
- **What is not affected:** a binary you already built keeps working; no sudo or administrator rights are used.
- **Tip:** install Rust with rustup, then rerun the install command from the README.
- **Why:** no binary releases exist yet. Because of that there is also no `SHA256SUMS.txt` and no signed binary to verify; the installer clones the repo and builds it.
- **What to do:** install a Rust toolchain and rerun. When binary releases start, the release checklist in CONTRIBUTING.md requires checksums and a stated signing status.

## KI-2: Project hooks, MCP servers or `permissions.allow` are ignored in headless runs

- **Where:** `zen -p ...` in CI or scripts, inside a repo that has `.zen/settings*.json`, `.mcp.json` or `.zen/mcp.json`.
- **What you see:** one line on stderr naming the skipped project config; stream-json `system/init` shows `folder_trusted: false`; the hook or MCP tool you expected never runs.
- **What is not affected:** everything under `~/.zen/`, and project deny rules, always apply.
- **Tip:** `zen doctor` shows the folder-trust state.
- **Why:** folder trust. Project config that can run commands, widen permissions or redirect provider requests loads only in a folder you trust, and headless mode never asks.
- **What to do:** if you trust the repo, pass `--trust-folder` or set `ZEN_TRUST_FOLDER=1`. Do not set it for repos you have not reviewed.

## KI-3: A script that checks for exit code 2 on usage errors stops matching

- **Where:** any wrapper script around `zen` that reads the exit code.
- **What you see:** a bad flag or unknown command now exits 4, not 2; exit 2 now means an authentication failure.
- **What is not affected:** exit 0 (ok) and exit 1 (general error).
- **Tip:** `docs/exit-codes.md` has the full table and the list of what each code covers today.
- **Why:** the team exit-code table was fixed (0 ok, 1 general, 2 auth, 3 not found, 4 validation, 5 forbidden, 6 rate limit, 7 conflict). Usage errors moved from clap's default 2 to 4 on 2026-11-05.
- **What to do:** update the script to the table. Some paths still fall back to 1; those are listed in `docs/exit-codes.md`.

## KI-4: Windows test `session::tests::latest_session_picks_newest` failed once on `main`

- **Where:** the Windows test job (`windows-check`, step `cargo test --workspace`), CI run 41992695809 after the merge of PR #44.
- **What you see:** `latest_session_for_cwd` returned `None` in that test (panic at `crates/zen-cli/src/session.rs:779`); 524 passed, 1 failed.
- **What is not affected:** the same test passed on the PR #44 branch and on the three earlier Windows runs of `main` (after #40, #42 and #43). The Linux workflow was still running when this was recorded, so its result is not known here.
- **Tip:** rerun the Windows job once; a repeat failure means it is real.
- **Why:** not proven. The test creates two sessions 30 ms apart; a timing or file-time effect on Windows is a guess, not a finding.
- **What to do:** if it fails again, reproduce on a Windows runner and fix the test or the code. Delete this entry once resolved.
