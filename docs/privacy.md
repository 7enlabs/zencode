# ZenCode privacy

This page describes the one piece of data ZenCode collects: the
anonymous usage counter (Viec 7). It is part of the product contract -
the design lives in `docs/design/user-counter.md` and the backend is
open source in `workers/user-counter/`.

## What is counted

Once per 24 hours, at startup, the CLI sends one ping containing
exactly four fields:

| Field | Example | Why |
|---|---|---|
| `install_id` | a random UUID v4 | generated on first run, stored in `~/.zen/user-counter.json`; identifies an install, never a person |
| `version` | `0.1.0` | which releases are in use |
| `os` | `linux` | platform distribution |
| `arch` | `x86_64` | architecture distribution |

From these pings the service publishes aggregate counts: total
installs, daily / weekly / monthly active installs, and the version
distribution. Multiple machines count as multiple installs - that is
stated openly wherever the numbers are shown.

## What is never collected

- No IP address. The server never reads or stores it; the database
  schema has no IP column.
- No email, account, name, or API key. The counter does not use any
  API key (BYOK-clean).
- No prompts, completions, code content, file names, or paths.
- No third-party analytics SDK. The only request goes to the
  project's own counter endpoint.

The raw `install_id` is hashed with SHA-256 on arrival and only the
hash is stored. Pings are limited to one accepted ping per install
per 12 hours server-side, and one per 24 hours client-side.

## Opt out

Counting is opt-out. Any one of these turns it off completely, and
the CLI checks them before every ping:

- `zen telemetry off` (writes `"telemetry": "off"` to `~/.zen/settings.json`)
- `DO_NOT_TRACK=1`
- `ZENCODE_TELEMETRY=0`
- `"telemetry": "off"` in `~/.zen/settings.json`

The first run prints a one-line notice naming these opt-out paths.

## Retention and transparency

Raw daily rows are kept for 13 months; aggregates are kept
indefinitely. The aggregates are public: `GET /stats` on the counter
endpoint returns `{installs, mau, wau, dau, versions, updated_at}`.
The counter endpoint is `https://count.runzen.dev`.
