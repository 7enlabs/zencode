# Companion WebSocket Contract v1 (APP-2 M1)

Status: M1 contract + schema only. The plan for this milestone was approved by
the project lead (Q1-Q10 decided, 2026-10-31). This document defines the
contract; it ships **no transport code** - no WebSocket server or client is
implemented in M1.

## 1. Purpose and scope

The companion channel connects a **companion** (the mobile-side app of APP-2)
with a **host** (the machine running `zen`). It carries the existing
stream-json v1 records (see `docs/stream-json-v1.md`) as payload inside
WebSocket frames over TLS, and adds three new companion events.

In scope for M1:

- This contract document.
- JSON Schemas for the WebSocket envelope, every existing stream-json v1
  record type/subtype, and the three new events (section 6).
- A fixture-driven schema test suite (section 8).

Out of scope for M1:

- Any running WebSocket/TLS transport (server or client code).
- Any change to the stream-json v1 records on the stdout channel. Desktop
  keeps driving the CLI through the subprocess stdout channel exactly as
  before.
- Operational certificate issuance beyond the pairing rules in section 3.

## 2. Roles and topology

- **Host**: the machine running `zen`. It terminates the WebSocket/TLS
  connection and emits the run's stream-json records.
- **Companion**: the mobile-side app. It connects to the host, receives the
  record stream, and may send the new companion events (section 6).
- The channel is scoped to the **local network (LAN)**. It is not designed
  for exposure to the public internet in this version.

## 3. Transport and security

- Transport is WebSocket over TLS (`wss`). Plaintext `ws` is not part of
  this contract.
- The host presents a **self-signed TLS certificate**.
- **Pairing is mandatory.** Before a companion is trusted, the user pairs it
  with the host by scanning a **QR code** shown by the host. The QR pairing
  step is also how the companion learns and pins the host's self-signed
  certificate; a companion must reject a different certificate for an
  already-paired host.
- An unpaired companion must not be able to open a run or receive record
  streams. The pairing handshake itself is a connection-level concern; it is
  not one of the three record-level events in section 6.

## 4. Framing and the WebSocket envelope

- Framing unit: **one stream-json record = one WebSocket text frame**
  (the NDJSON line boundary of the stdout channel becomes the frame
  boundary). Records are never split or merged across frames.
- Every frame carries a WebSocket **envelope** that wraps exactly one
  payload. The envelope is a layer outside the stream-json record:

  - The **payload** is either an unchanged stream-json v1 record or one of
    the three new companion events (section 6).
  - The envelope adds no fields of its own in M1: its fields are the
    stream-json v1 record fields themselves (`type`, `v`, `uuid`,
    `session_id`), so carrying a payload never changes the record shapes
    defined by stream-json v1.

- The envelope field set is finalised (Q1-Q10 decision, transcribed
  2026-11-01): the envelope reuses the stream-json v1 record fields -
  `type`, `v`, `uuid`, and `session_id`. Record payloads carry them exactly
  as stream-json v1 defines them (`uuid` is absent only on `system/init`);
  companion event payloads carry all of them, with `type` set to the event
  name (section 6). No additional wrapper fields are added in M1.
- Maximum message size: **4 MiB per message** (one record or event = one
  text frame). An oversized message is never truncated silently and never
  split across frames in M1: the host closes the connection and ends the
  run with an error `result` record.

## 5. Record stream semantics (inherited from stream-json v1)

The payload records keep the semantics of `docs/stream-json-v1.md`:

- Every record has `type` (one of `system`, `assistant`, `user`,
  `stream_event`, `result`), `v` = `1`, and a `session_id` shared by all
  records of a run. `system` records have a `subtype`.
- `system/init` is the first record of a run; exactly one `result` record
  ends the run. The `result` record - not the socket close - is the
  end-of-run signal for the companion.
- There is no `seq` field in v1. Receivers that need ordering use frame
  arrival order.
- Key order is not contractual. Unknown fields must be ignored
  ("ignore what you do not know").
- Versioning is additive-only, inherited from stream-json v1: new record
  types, subtypes, or optional fields do not bump `v`; a breaking change
  bumps `v` and is recorded in the CHANGELOG.

Connection lifecycle:

- Process exit codes (0/1/130/143) exist only on the host side; they have no
  direct WebSocket equivalent. A socket that closes before the run's
  `result` record arrives means the run ended abnormally from the
  companion's point of view.
- Application-level liveness during long tool calls is already provided by
  the `system/tool_progress` records (about every 5 seconds). Transport
  ping/pong is a separate layer and is not a substitute for it.

## 6. The three new companion events

The channel adds **three new events** on top of the stream-json v1 records.
Their names, directions (host -> companion and/or companion -> host), and
payload fields were decided by the project lead together with Z App
(APP-2 plan approval, messages 591 and 1596-1597 on 2026-10-31).

The three events are transcribed verbatim from the Q1-Q10 decision text
(project lead, 2026-10-31; decision text re-issued 2026-11-01). Each event
is a payload in the same envelope as the record payloads (section 4):
`type` carries the event name, and the envelope fields are the record v1
fields (`v` = 1, a fresh `uuid`, and the run's `session_id`). The events
are additive in v1 - `v` is not bumped.

| event | direction | required fields | optional fields |
|---|---|---|---|
| `companion_prompt` | companion -> host | `text` | `id` |
| `companion_interrupt` | companion -> host | (none) | `reason` |
| `host_status` | host -> companion | `state` (`idle` or `running`) | (none) |

- `companion_prompt` carries the user's prompt in `text`. `id` is an
  optional companion-chosen correlation identifier. The host answers with
  the run's record stream: `system/init` first, exactly one `result` last.
- `companion_interrupt` asks the host to interrupt the current run.
  `reason` is optional free text. The host still ends the run with exactly
  one `result` record; the interrupt does not replace it.
- `host_status` reports whether the host has a run in progress: `state`
  is exactly `idle` (no run in progress) or `running` (a run is in
  progress).

The events do not change any stream-json v1 record shape. There is no
`seq` field and no resume in M1: if the connection drops, the companion
reconnects and starts over. Each event has its own JSON Schema and valid
fixtures in the schema suite (sections 7 and 8).

## 7. Schemas

Schemas live in `docs/schemas/companion-ws-v1/` (JSON Schema draft 2020-12):

| file | covers |
|---|---|
| `envelope.schema.json` | Fields common to every stream-json v1 record and to the three companion events: `type` (enum of 5 record types + 3 event names), `v` (const 1), `session_id` (required). `uuid` is not required at this level because `system/init` has none. |
| `system.schema.json` | Every `system` record: `subtype` required, enum of the 7 contract subtypes. |
| `system-init.schema.json` | `init`: cwd, model, tools, mcp_servers, permissionMode, slash_commands, folder_trusted; optional `mcp_server_errors`. |
| `system-api_retry.schema.json` | attempt, max_retries, retry_delay_ms, error_status, error. |
| `system-permission_denied.schema.json` | tool_name, tool_use_id, decision_reason. |
| `system-hook_message.schema.json` | hook_event_name (open string set), message. |
| `system-compact_boundary.schema.json` | trigger (auto/manual), pre_tokens. |
| `system-prune.schema.json` | pruned, tokens_saved. Written from the contract text; no fixture emits this record yet. |
| `system-tool_progress.schema.json` | tool_use_id, tool_name, elapsed_ms. Written from the contract text; no fixture emits this record yet. |
| `assistant.schema.json` | message with role `assistant`, optional content and tool_calls; parent_tool_use_id. |
| `user.schema.json` | message with role `user`, content array of tool_result entries. |
| `stream_event.schema.json` | event/delta. Written loosely from the single contract example; no fixture emits this record yet. |
| `result.schema.json` | subtype enum (success, error_max_turns, error_during_execution), is_error, duration fields, num_turns, result, total_cost_usd, usage, model, permission_denials, stop_reason. |
| `companion_prompt.schema.json` | Companion -> host prompt event: `text` required, `id` optional; envelope fields (`v`, `uuid`, `session_id`) required. |
| `companion_interrupt.schema.json` | Companion -> host interrupt event: `reason` optional; envelope fields required. |
| `host_status.schema.json` | Host -> companion status event: `state` required, one of `idle`/`running`; envelope fields required. |

All schemas set `additionalProperties: true`: unknown fields stay valid,
matching the v1 ignore-unknown rule. Schemas are self-contained (no
cross-file `$ref`) so each can be used on its own.

Known limits of per-record schemas (inherited from stream-json v1, enforced
at stream/test level, not by these schemas): record ordering (`init` first,
exactly one final `result`), one shared `session_id` per run, unique `uuid`
per record, and process exit codes.

## 8. Schema test suite

`docs/schemas/companion-ws-v1/validate_schemas.py` validates the shipped
fixtures against the schemas. The fixtures in
`docs/schemas/companion-ws-v1/fixtures/stream-json/` are byte-identical
copies of the contract fixtures in `docs/fixtures/stream-json/` (kept next
to the schemas so the suite is self-contained; the copies must be refreshed
whenever the source fixtures change). The companion event fixtures live in
`docs/schemas/companion-ws-v1/fixtures/companion/`.

Run:

```sh
python3 docs/schemas/companion-ws-v1/validate_schemas.py
```

Exit code 0 means PASS. The script uses the `jsonschema` library
(Draft 2020-12) when importable and otherwise falls back to a built-in
subset validator; it installs nothing.

Current results (against the fixtures shipped in this commit):

- Valid cases: 56/56 records validate against the envelope plus their
  per-type (and per-subtype) schemas - 50 records across the 11 stream-json
  fixtures, plus 6 companion event records in
  `fixtures/companion/events.ndjson` (each event covered with and without
  its optional fields).
- Negative cases: 10/10 rejected as expected (missing `v`; `v` = 2;
  unknown `type`; missing `session_id`; `system` record missing `subtype`;
  `result` record missing `usage`; `companion_prompt` missing `text`;
  `companion_prompt` missing `uuid`; `host_status` missing `state`;
  `host_status` with `state` outside the enum).
- Compatibility case: 1/1 - an `assistant` record carrying an unknown extra
  field stays valid.
- Verified with both validators the script supports: the `jsonschema`
  library (Draft 2020-12) and the built-in subset validator give the same
  results.

The existing Rust contract test is unchanged and must stay green:

```sh
cargo test -p zen-cli --test stream_json_contract
```

## 9. Open points

1. Resolved in the M1 supplement commit: the three events are transcribed
   into section 6 with schemas + fixtures, and the envelope field set and
   the 4 MiB message limit (close + error `result`, no silent truncation,
   no frame splitting) are written into section 4.
2. Reconnect/resume: decided for M1 - no `seq`, no resume; a dropped
   connection is re-established from the start. Whether a later milestone
   adds resume, and whether a sequence number would live in the envelope
   rather than in records, remains undecided and out of M1 scope.
3. Three schemas (`stream_event`, `system-prune`, `system-tool_progress`)
   are written from the contract text only; they must be re-checked against
   real records when a run first emits those types.
