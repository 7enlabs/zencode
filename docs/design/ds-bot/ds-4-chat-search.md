# DS-4 Chat Search - Design Doc

**Status:** DRAFT for Leader review (design only - no code in this change)
**Task:** DS-4, assigned by ZC Leader (Telegram tin 5718), due 19/11/2026
**Branch:** `design/ds-bot-chat-search`, from `main` 647fcc0
**Scope:** design only. No implementation, no dependency change, no behaviour
change ships with this document.

## 0. Dossier cross-check - READ FIRST (mismatch)

The assignment cites "dossier section 7 / section 8" for the search design.
**That reference does not match the dossier on file.** The dossier
`~/workspace/zencode/study-ds-bot-fei2411-2026-11-05.md`
(SHA256 `c0f94baae4fdd22491ead96ed09eef82206615165c0b6c7f3dc28beff22fec30`,
7502 bytes) has **only 5 sections** and no section 7 or 8:

| Assignment cites | Dossier actually has |
| --- | --- |
| dossier section 7 | nothing - dossier ends at section 5 |
| dossier section 8 | nothing - dossier ends at section 5 |
| (implied) FTS5/trigram/BM25 detail | not in the dossier - dossier section 4 states the author had **not** read the upstream source code (`ds-bot/src/host/*`) |

What the dossier *does* contain that is relevant here:

- **Section 2, point 9** - DS-Bot memory is Markdown (`MEMORY.md`, 4,000 chars;
  topics 16,000 chars; 40 topics per scope) with `journal.jsonl` written
  before/after every change, enabling undo; "memory is data, not a command".
- **Section 4, critic 1** - "Learning from the README is not enough. The
  `ds-bot/src/host/*` source was not read; every claim about secret redaction,
  Main Bot rights, or the memory journal rests on self-description only."

This design therefore does **not** rest on the dossier for the search
mechanics. Section 2 below is new primary-source work: the upstream source
was read directly for this design (commit cited there). **Leader to confirm:**
which dossier sections the assignment meant, or whether the section 7/8
reference was to a different document. Until confirmed, treat every
"dossier section 7/8" claim in the assignment as unverified.

## 1. Goal

Let a user find an old chat again by words: `zen sessions search <query>`
(and the same search from ZenCode Desktop) returns the matching messages,
oldest first, with enough context to recognise them, and a way to open the
full message and its neighbours. Search covers all sessions the user owns on
this machine, across all project directories - not only the current
directory's sessions.

Non-goals (explicit):

- No semantic/vector search, no embeddings, no network call. Search is local
  only and works offline.
- No search across other users, machines, or the cloud. No sync of the index.
- No change to the session journal format (`docs/session-format.md`, v1 frozen)
  and no change to how sessions are written, resumed, or exported.
- No copy of upstream code. DS-Bot is Apache-2.0 and obelisk (the other
  reference in the assignment) is AGPL-3.0; this design borrows **ideas only**
  and every implementation must be written from scratch for ZenCode.
  The upstream code was read to learn the design, not to reuse it.

## 2. What DS-Bot actually does (primary source, read for this design)

Upstream: <https://github.com/Fei2411/DS-Bot>, read at commit
`71d344b` ("Merge pull request #874", version 0.2.7 in `ds-bot/package.json`).
All claims in this section are `[S]` (source read) with file + line.

### 2.1 Where search lives

- `ds-bot/src/host/chat-index.js` - the whole index: schema, incremental
  indexing, search, latest, and around reads. Its header comment states the
  design in one paragraph: "read_own_chat searches a SQLite FTS5 table of
  every part of a Bot's own chat, ranked by BM25. The trigram tokenizer
  matches any run of three or more characters, so Chinese needs no word
  splitting; shorter words fall back to LIKE. The index is derived from the
  Session logs: a part catches up when it is read, an ended part is sealed
  once it is complete, and deleting the file only costs a rebuild. Without
  SQLite, read_own_chat scans the logs as before."
- `ds-bot/src/host/groups.js:1029-1090` - the `read_own_chat` tool exposed to
  the model: parameters `query` (max 80 chars), `around` (anchor, max 40
  chars), `part` (integer). It returns latest messages, search hits, the
  handoff note of an earlier part, or the messages around an anchor.
- `ds-bot/src/host/text.js:26-31` - `excerpt(text, wanted, max)`: if a message
  is longer than the cap, show the window starting 120 chars before the first
  match, so the hit stays visible.
- `ds-bot/src/host/constants.js:83-91` - limits: `OWN_CHAT_LINE_MAX = 400`,
  `ANCHOR_LINE_MAX = 8000`, `OWN_CHAT_BRIEF = 10`, `OWN_CHAT_READ = 12`,
  `OWN_CHAT_SEARCH = 20`, `HISTORY_PAGE = 200`, `HISTORY_PAGES_MAX = 50`.
- `ds-bot/test/history.test.js:64-77` - unit test for `termQuery` (see 2.3).
- `ds-bot/src/host/privacy.js:70-75` - the index files are listed as private
  data: `chat-index.sqlite`, `chat-index.sqlite-wal`, `chat-index.sqlite-shm`
  (next to `usage.sqlite`).

### 2.2 Storage: derived index, logs stay the source of truth

- Sessions live in DeepSeek Harness (DSH) session logs, **not** in SQLite.
  The index is a **derived cache**: `chat-index.js` `install()` opens
  `<home>/chat-index.sqlite` with `node:sqlite` (Node's built-in SQLite,
  no extra dependency) and `PRAGMA journal_mode = WAL`.
- Schema (`createIndex`, `INDEX_VERSION = 3`):

```sql
CREATE VIRTUAL TABLE lines USING fts5(
  text,
  session UNINDEXED, seq UNINDEXED, time UNINDEXED, who UNINDEXED,
  tokenize = 'trigram');
CREATE TABLE marks (
  session TEXT PRIMARY KEY,
  through INTEGER NOT NULL, keep INTEGER NOT NULL,
  sealed INTEGER NOT NULL, turn INTEGER NOT NULL);
-- PRAGMA user_version = 3
```

- `marks` records, per session part, how far indexing has got (`through` =
  last indexed `seq`), whether assistant answers are kept, whether the part
  is sealed, and where the current turn started (`turn`).
- Indexing is lazy and incremental (`catchUp`): when a part is read, only the
  events after its mark are appended, in one transaction; a gap in `seq`
  throws instead of silently skipping. A part is sealed 60 s after it ends
  (`SEAL_AFTER_MS = 60_000`); a sealed part is never re-indexed.
- Corruption handling: if opening fails, the file is deleted once and rebuilt
  from the logs; if that fails too, search degrades to a full log scan
  (`scanOwnChat`) with a warning. Deleting the index file is always safe.
- Only rendered message text is indexed (`rt.historyEntry(event, false)`):
  user/bot messages, assistant answers, question-card answers; setup events,
  handoff markers, and plugin chatter are skipped (`historyEntry` returns
  `undefined`), and empty text is not inserted.
- The current turn is hidden from search (`hidden: { session, from: turn }`):
  the asking turn is already in context, so its own question must not match
  itself.

### 2.3 Query construction (`termQuery`, `chat-index.js:22-42`)

```js
// Each term is a quoted FTS5 phrase when trigrams can match it, else LIKE.
export function termQuery(terms, sessions, limit, hidden) {
  const long = terms.filter(term => [...term].length >= 3)
  const short = terms.filter(term => [...term].length < 3)
  // ... long terms -> lines MATCH '"t1" "t2" ...'
  // ... short terms -> text LIKE '%t%' ESCAPE '\' (with % _ escaped)
  // ... ORDER BY rank when any long term, else time DESC; LIMIT <limit>
}
```

Rules worth borrowing, and one quirk worth fixing:

1. **Split and cap:** the query is lowercased, split on whitespace, deduped,
   capped at `SEARCH_TERMS_MAX = 6` terms (`searchOwnChat`).
2. **Long terms (>= 3 characters) go to FTS5** as quoted phrases joined by
   spaces (FTS5 AND). Quotes inside a term are doubled (`"` -> `""`).
   Results are `ORDER BY rank` - FTS5 BM25, best first.
3. **Short terms (< 3 characters) fall back to `LIKE`** with `%` and `_`
   escaped and `ESCAPE '\'`. When there is no long term at all, results are
   `ORDER BY time DESC` (no BM25 available).
4. **Session scoping:** `session IN (SELECT value FROM json_each(?))` with the
   session ids passed as a JSON array argument.
5. **AND, then OR fallback:** `searchOwnChat` first requires ALL terms; if
   that returns nothing and there is more than one term, it runs each term
   alone (top 50 hits each, `ANY_TERM_HITS = 50`), counts how many terms each
   message matched, and returns the most-matching first. Results are finally
   re-sorted oldest-first for display, and flagged `every: true/false` so the
   caller can say "no message has all of these words; messages with the most
   of these words".
6. **Quirk (documented by its own test):** the >= 3 test counts Unicode code
   points, so a 2-character Chinese word such as `会议` is "short" and goes to
   LIKE, not FTS5 - even though the trigram tokenizer could match it
   (`test/history.test.js:70-76` expects `LIKE '%会议%'`). The trigram claim
   "Chinese needs no word splitting" is therefore only true for terms of 3+
   characters. A ZenCode design should not copy this quirk blindly; see 4.5.

### 2.4 Reads around a hit

- `latestOwnChat`: per part, `SELECT ... WHERE session = ? AND seq < ?
  ORDER BY seq DESC LIMIT ?`, newest part first, then reversed to oldest-first.
- `aroundOwnChat`: parses an anchor `part N #seq` (regex
  `/(?:part\s*)?(\d+)?\s*#\s*(\d+)/i`), returns `AROUND_LINES = 6` messages
  before and after it; the anchor message itself is shown in full
  (`ANCHOR_LINE_MAX = 8000` chars), neighbours are capped at 400 chars.
- Anchors are the user-facing identity of a hit: `[part 2 #1234 · 10-08 14:22]`.

### 2.5 What DS-Bot does NOT search

- **Group chats are not full-text searchable.** `read_group_chat`
  (`groups.js`) shows only the latest messages of a group; only a Bot's
  **own chat with the user** is indexed. The assignment's "chat search"
  should be read with this limit in mind: DS-Bot proves own-chat search,
  not group search.
- There is no pagination cursor: search returns at most 20 lines, latest at
  most 12, and there is no "next page". That is acceptable for a model tool
  (the model asks again with narrower words) but not for a human CLI - see 4.6.

### 2.6 What this design takes from DS-Bot, and what it does not

Take: FTS5 trigram + BM25; derived index with a rebuildable file; incremental
catch-up with a per-session watermark; quoted-phrase terms + LIKE fallback for
short terms; AND-then-OR fallback; excerpt centred on the first match;
anchors (`session#seq`); hidden-current-turn exclusion; WAL; versioned schema
with rebuild on mismatch.

Do not take: Node's `node:sqlite` (ZenCode is Rust - see section 5);
indexing only the current bot's own chat (ZenCode has no bot/user split - it
indexes the user's own sessions); the 2-character-CJK-to-LIKE quirk; the
no-pagination limit; storing the index as the only copy of anything (DS-Bot
already doesn't - the logs are the truth; ZenCode must keep the same rule
with its JSONL journals).

## 3. Where ZenCode stands today (primary source, repo at 647fcc0)

- Sessions are **JSONL journals**, one file per session:
  `~/.zen/projects/<encoded-cwd>/<session-id>.jsonl`
  (`crates/zen-cli/src/session.rs:36-46`, `session_path`; `encode_cwd`
  replaces `/ \ : . ? * " < > |` with `-`). A session is a stream of JSON
  records (`message`, `summary`, `tool_result`, `request`, `response`,
  `usage`, ...); there is no database of any kind in the workspace today
  (no `rusqlite` in `Cargo.toml` / `Cargo.lock` - verified by grep at 647fcc0).
- Discovery is filesystem-based: `list_sessions_for_cwd`
  (`session.rs:451`) lists `<id>.jsonl` files in one project dir sorted by
  mtime; `list_all_sessions` (`session_mgmt.rs:85`) scans all project dirs
  and returns `SessionInfo { id, path, archived, modified, message_count,
  input_tokens, output_tokens, title }`, where `message_count` and the token
  totals come from a full scan of the transcript (`scan_transcript`).
  `find_session_path` (`session.rs:52`) walks `~/.zen/projects` to resolve an
  id to a file.
- CLI surface today (`crates/zen-cli/src/cli.rs:160-188, 385-502`):
  `--resume <id|path>`, `--continue`, `--fork-session`,
  `zen sessions list [--all] | archive | delete | restore`. **There is no
  search subcommand.**
- Export is a separate, frozen format: `docs/session-format.md`,
  `format_version: 1`, produced by `zen export`, consumed by `zen import`
  (import always creates a NEW session id). Search must not change it.
- Skill search already exists (`SPEC.md:124`: `zen skill search <query>`,
  ranked exact name > prefix > substring > tag > description) - the ranking
  vocabulary and the `--json` output conventions there are the closest
  in-repo precedent for a search command's UX, but it searches skill metadata,
  not session text.
- Desktop is a separate repo (`github.com/7enlab/zencode-desktop`); this
  design covers the shared part only (section 4.7) and defers Desktop-side
  UI/commands to a follow-up design owned with the Desktop line.

## 4. Design

### 4.1 Principles

1. **The JSONL journal stays the source of truth.** The index is a derived
   cache in one file. Deleting it costs a rebuild, never data. A session
   file that fails to parse is skipped with a warning, never indexed
   half-way, and the failure is reported by the command that noticed it.
2. **Local only, no new network.** The index file lives under `~/.zen/`
   next to the projects directory. Nothing about its content leaves the
   machine. Search works offline.
3. **One index, two front-ends.** CLI and Desktop share the same Rust code
   and the same index file, so a search in either sees the same results.
4. **Fail soft, like DS-Bot.** If the index cannot be opened, search falls
   back to a slow scan of the journals (bounded, see 4.8) and says so;
   it never fails the whole command for an index problem.
5. **No new runtime dependency unless it earns its place.** The only
   proposed new dependency is `rusqlite` with the `bundled` feature
   (section 5 proves it provides FTS5). No server, no background daemon,
   no separate search engine process.

### 4.2 Schema

One SQLite file, `~/.zen/chat-index.sqlite`, WAL mode, `PRAGMA user_version`
as the schema version (start at 1; bump on any incompatible change, rebuild
from journals). Two tables plus the FTS5 table:

```sql
-- Source of truth for reads that are NOT full-text search (latest, around):
-- a plain table is indexable, unlike an FTS5 content table (see 5.4).
CREATE TABLE IF NOT EXISTS messages (
  id          INTEGER PRIMARY KEY,
  session_id  TEXT NOT NULL,          -- <uuid>, matches <id>.jsonl stem
  project     TEXT NOT NULL,          -- encoded cwd dir name
  seq         INTEGER NOT NULL,       -- 0-based record number in the journal
  time_ms     INTEGER NOT NULL,       -- record timestamp, epoch ms (0 if absent)
  role        TEXT NOT NULL,          -- user | assistant | tool | summary | system
  text        TEXT NOT NULL,          -- rendered searchable text (see 4.3)
  title       TEXT                    -- session title, denormalised for display
);
CREATE INDEX IF NOT EXISTS idx_messages_session_seq
  ON messages (session_id, seq);
CREATE INDEX IF NOT EXISTS idx_messages_time
  ON messages (time_ms);

-- Full-text index: external-content FTS5 over messages.text, so the text is
-- stored once (in messages) and the FTS5 table holds only the trigram index.
-- DS-Bot keeps text inside the FTS5 table; this design differs deliberately
-- (see 5.4: DS-Bot's latest/around reads full-scan without an index).
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  text, content = 'messages', content_rowid = 'id',
  tokenize = 'trigram');

-- Incremental-index watermark, borrowed from DS-Bot's marks table.
CREATE TABLE IF NOT EXISTS index_state (
  session_id  TEXT PRIMARY KEY,
  through_seq INTEGER NOT NULL,       -- last journal seq indexed (-1 = none)
  file_mtime_ms INTEGER NOT NULL,     -- journal mtime at last index
  file_len    INTEGER NOT NULL,       -- journal byte length at last index
  sealed      INTEGER NOT NULL DEFAULT 0  -- 1 when the session is archived/deleted
);
```

Why `content = 'messages'` instead of DS-Bot's content-storing FTS5 table:
the measurement in section 5.4 shows DS-Bot's shape makes "latest messages
of a session" a full table scan (23 ms at 100k rows and growing linearly),
because FTS5 content columns declared UNINDEXED cannot be indexed. Keeping
the text once in `messages` (with a real `(session_id, seq)` index) and the
trigrams in an external-content FTS5 table gives both fast search and fast
latest/around reads, at the cost of keeping the two tables in sync inside one
transaction (insert into `messages`, then `INSERT INTO messages_fts
(messages_fts) VALUES ('rebuild')` is NOT used - instead insert the rowid
into the FTS5 table explicitly, or use the `rebuild` command only for a full
rebuild).

### 4.3 What gets indexed

One row per journal record that renders to non-empty text:

| Journal record | Indexed as | `role` |
| --- | --- | --- |
| `message` with user/assistant content | rendered plain text (string content, or text blocks joined with `\n`) | `user` / `assistant` |
| `tool_result` | tool name + rendered result text, truncated to 4,000 chars (tool output is verbose; the cap keeps the index small and matches DS-Bot's 400-char display cap philosophy at a higher bound) | `tool` |
| `summary` (compaction summary) | summary text | `summary` |
| `system`, `request`, `response`, `usage`, `response_error` | **not indexed** (metadata, not conversation; same exclusion as DS-Bot's `historyEntry` returning `undefined` for setup/plugin events) | - |

Rules: empty text is not inserted (same as DS-Bot). Secrets are NOT
redacted in the index - the index is a local file under `~/.zen/` with the
same trust as the journals themselves, which already contain the raw text.
The index file must be listed alongside the journals wherever private files
are enumerated (cf. DS-Bot `privacy.js:70-75` listing `chat-index.sqlite*`),
and `zen sessions delete` must delete the session's index rows in the same
operation. Search output follows the same redaction rules as `zen export
--redact` when the user passes `--redact`; by default it prints what the
journal holds, exactly as `zen sessions list` already exposes titles.

### 4.4 Indexing flow (incremental catch-up, borrowed from DS-Bot)

```
open index (create if missing; WAL; busy_timeout 5000 ms)
for each session journal under ~/.zen/projects/**/<id>.jsonl (and archive/):
    state = index_state[session_id]
    if state exists and state.file_mtime_ms == journal mtime
       and state.file_len == journal length: skip (up to date)
    else:
        read journal records after state.through_seq (or all, if no state)
        in ONE transaction: insert new messages rows + FTS5 rows,
            update index_state (through_seq, mtime, len)
        on any error: ROLLBACK, keep the old watermark, warn, continue
if index_state has a session_id whose journal no longer exists:
    delete its messages + FTS5 rows + state row (session was deleted)
```

When indexing runs: **lazily, at the start of every search/latest command**
(DS-Bot's "a part catches up when it is read"), plus after a session ends
(`--resume` exit path may trigger a catch-up for that session only, so the
next search is fast). There is deliberately **no background daemon and no
file watcher** in DS-4: a watcher is a second process to supervise and a
cross-platform support burden; lazy catch-up bounds the cost to "only
sessions that changed since the last search". If a search ever feels slow
because many journals changed, the remedy is the same catch-up - it is
already the work being measured in section 5 (100k rows indexed in 1.26 s).

Sealing: a session under `archive/` is indexed once and then marked
`sealed = 1`; sealed sessions are skipped by catch-up (their journals do not
change). This mirrors DS-Bot's `SEAL_AFTER_MS` sealing of ended parts.

Corruption: if the file cannot be opened or `user_version` is not the
expected value, delete `chat-index.sqlite*` and rebuild from journals
(exactly DS-Bot's "damaged file is rebuilt from the logs once"). Rebuild is
also available explicitly as `zen sessions search --rebuild-index`.

Concurrency: CLI and Desktop may both open the index. SQLite WAL allows one
writer + many readers; use `busy_timeout = 5000` ms and a single-writer
discipline (each catch-up takes the write lock for one transaction; a second
writer waits, then re-checks the watermark and skips work already done).
If the lock cannot be taken, search reads whatever is indexed and warns -
it never blocks the user for more than the timeout.

### 4.5 Query construction (adapted from DS-Bot `termQuery`)

```
query: string, max 200 chars (DS-Bot caps at 80 for a model tool; a human
       CLI can afford more, but unbounded input is a foot-gun for FTS5 syntax)
terms = lowercase(query).split(/\s+/).filter(non-empty).dedup().slice(0, 6)
long  = terms with char count >= 3        -- FTS5 phrase terms
short = terms with char count < 3         -- LIKE terms
```

SQL (same shape as DS-Bot, with two deliberate fixes):

```sql
-- AND pass: every term must match.
SELECT m.session_id, m.project, m.seq, m.time_ms, m.role, m.text
FROM messages m
JOIN messages_fts f ON f.rowid = m.id
WHERE <fts-match> AND <like-filters> AND <session filter>
ORDER BY rank LIMIT <limit>;          -- BM25 when any long term exists
-- when there is no long term: ORDER BY m.time_ms DESC instead of rank.
```

- FTS5 match string: long terms as quoted phrases joined by spaces,
  `'"authentication" "token"'`, with `"` doubled inside a term
  (exactly DS-Bot). The string is passed as a **bound parameter** (`MATCH ?`),
  never interpolated - user input must never become SQL.
- LIKE filters: `m.text LIKE ? ESCAPE '\'` with `%` and `_` escaped in the
  term and wrapped in `%...%` (exactly DS-Bot's `likeOf`).
- Session filter: optional `--session <id>` (one session) or
  `--project <encoded-cwd>` (one project dir); default is all sessions.
  Implemented as `m.session_id = ?` / `m.project = ?` on the indexed
  `messages` table - not DS-Bot's `json_each` session list, because ZenCode
  does not have a small fixed session list: it has every session on disk.
- **Fix 1 - the CJK quirk:** DS-Bot counts characters, so 2-character CJK
  words fall to LIKE (section 2.3.6). ZenCode instead defines "long" by
  **trigram count**: a term is long if it yields at least one trigram, i.e.
  it has >= 3 characters **in the tokenizer's terms** - which for CJK means
  >= 3 code points still. The honest fix is different: treat a term as long
  when `len(term) >= 3` **or** the term contains any CJK code point
  (U+2E80 and above, same threshold as DS-Bot's own `estimateTokens` in
  `text.js:41`), because trigram tokenization matches CJK substrings of any
  length >= 1 once tokenized - a 2-char CJK term produces trigrams from its
  neighbours in the indexed text, but a bare 2-char FTS5 phrase query
  `"会议"` is itself a valid trigram query and matches. This must be verified
  by a test before implementation (a 2-char CJK query against the section 5
  harness); the design records the intended rule and the verification owed.
  Until that test passes, the DS-Bot behaviour (LIKE fallback) is the safe
  default.
- **Fix 2 - the empty-result fallback:** same as DS-Bot (AND, then per-term
  OR with top-50-per-term and match-count ranking), but the per-term pass
  reuses the same SQL with one term, and the result carries a flag
  (`partial: true`) so the CLI can print DS-Bot's honest heading: "no message
  has all of ...; messages with the most of these words".

### 4.6 Results, pagination, and the CLI surface

DS-Bot returns at most 20 lines with no next page (section 2.5). A human
CLI needs pagination, so DS-4 adds a cursor:

```
zen sessions search <query> [--session <id>] [--project <dir>]
    [--limit <n, default 20, max 100>] [--cursor <opaque>]
    [--around <session#seq>] [--json] [--rebuild-index]
```

- Each hit: `session_id` (short 8-char id, cf. `session_mgmt.rs:short_id`),
  project, `seq`, timestamp, role, and an **excerpt**: the message text cut
  to 400 chars centred on the first matching term (DS-Bot's `excerpt`,
  120 chars of leading context, `…` markers at cut points). `--json`
  returns the same rows plus `total`, `partial`, and `next_cursor`.
- `--around <session#seq>`: the DS-Bot anchor read - that message in full
  (cap 8,000 chars) plus 6 messages before and after it, from the `messages`
  table via the `(session_id, seq)` index (fast by construction, unlike
  DS-Bot's FTS5-only scan - section 5.4).
- Default sort is **oldest first** for reading (DS-Bot's display order), but
  the BM25 rank decides *which* messages are in the page; the cursor encodes
  (last rank, last id) so the next page continues the ranking. `--latest`
  flips to newest-first (latest-messages read, no query needed).
- Output contract follows the repo's existing conventions: human-readable by
  default, `--json` machine-readable, exit code 0 with zero hits (empty
  result, not an error - a search that finds nothing is a successful search),
  exit code 2 for a malformed query (empty query, over-long query).

### 4.7 Sharing with Desktop

- The search engine is a new Rust module in `zen-core` (e.g.
  `crates/zen-core/src/chat_search.rs`, exact module name decided at
  implementation): pure functions over a SQLite path - `open`, `catch_up`,
  `search`, `latest`, `around`, `delete_session`, `rebuild`. No CLI types,
  no I/O outside the index file and the journals.
- The CLI (`zen-cli`) is a thin wrapper: argument parsing + output formatting.
- Desktop uses the same crate through its existing Rust backend (Tauri):
  the same index file, the same queries, the same excerpts. Desktop-specific
  UI (search box, result list, click-to-open-session) is **out of scope for
  DS-4's design** and needs a follow-up design with the Desktop line; DS-4
  only guarantees the shared engine is UI-agnostic (no terminal formatting
  inside `zen-core`, results as data).
- The index file location is derived from the Zen home directory in both
  front-ends (`~/.zen/chat-index.sqlite`), so CLI and Desktop never disagree
  about where it is. If Desktop ever runs with a different home, it gets a
  different index - that is correct behaviour, not a bug: the index is a
  cache of that home's journals.

### 4.8 Fallback when SQLite is unavailable

If the index file cannot be opened (corrupt beyond rebuild, locked, or the
`rusqlite` dependency is disabled at build time): search degrades to a
bounded journal scan - read each `<id>.jsonl` newest-first, substring-match
the query terms, stop after 200 files or 50,000 records, and print a warning
that results may be incomplete (DS-Bot's `scanOwnChat` behaviour, with an
explicit bound DS-Bot does not state). The fallback is a degraded mode, not
a second implementation: it shares the excerpt/anchor formatting.

## 5. Verification: FTS5 in Rust, and performance at 100k messages

All numbers below are **measured on this VM on 05/11/2026**, not estimated.
The harness is a throwaway Cargo project (not part of the repo):
`/tmp/fts5-check` (`rusqlite 0.37.0`, `libsqlite3-sys 0.35.0`,
`--features bundled`, `rustc 1.99.1`, `cargo 1.99.1`).

### 5.1 FTS5 is available in `rusqlite` + `bundled` - CONFIRMED

Command: `cargo run --release` in `/tmp/fts5-check` (exit code 0).
The program opens a SQLite database, runs
`CREATE VIRTUAL TABLE lines USING fts5(text, session UNINDEXED,
seq UNINDEXED, time UNINDEXED, who UNINDEXED, tokenize = 'trigram')`
- the exact DS-Bot schema shape - and it succeeds. The bundled SQLite
version reported by `rusqlite::version()` is **3.50.2**. No extra feature
flag beyond `bundled` was needed: FTS5 (and the trigram tokenizer) are part
of the SQLite amalgamation that `libsqlite3-sys` compiles. Conclusion:
adding `rusqlite = { version = "0.37", features = ["bundled"] }` to the
workspace is sufficient for DS-4; no system SQLite dependency, no
`libsqlite3-sys` feature tuning. (Pin the exact version at implementation
time from the then-current crates.io; 0.37.0 is what resolved on 05/11/2026.
A newer `rusqlite` must re-run this check - FTS5 availability is a property
of the bundled amalgamation version, and this document only verifies 3.50.2.)

### 5.2 Corpus and method

- Synthetic corpus: **100,000 messages across 200 sessions** (500 messages
  per session), generated in the harness from 10 topic templates (English,
  Vietnamese with diacritics, CJK `你好世界`, code `fn main`, plus a rare
  term `quokka` in 1,080 rows). Mean message length ~70 chars.
- Insert in a single transaction: **1.26 s** for 100,000 rows.
- Sizes after `PRAGMA wal_checkpoint(TRUNCATE)` (from `dbstat`):

| Table | Bytes | Share |
| --- | --- | --- |
| `lines_data` (trigram index) | 19,984,384 | 56% |
| `lines_content` (stored text) | 11,051,008 | 31% |
| `lines_docsize` | 1,392,640 | 4% |
| `lines_idx` | 57,344 | <1% |
| other (`marks`, schema, config) | 12,288 | <1% |
| **Total DB file** | **35,397,632 (33.76 MiB)** | 100% |

- Counts verified: `MATCH '"authentication" "token"'` -> 10,000 rows;
  `MATCH '"quokka"'` -> 1,080 rows; `MATCH '"你好世界"'` -> 10,000 rows;
  `LIKE '%ox%'` -> 10,000 rows. BM25 sanity: the top-ranked hit for
  `'quokka'` contains the term (`true`).

### 5.3 Query timings (best of 5 runs, warm cache, LIMIT 20)

| Query | Time |
| --- | --- |
| 2-term AND, common terms, `ORDER BY rank` | **34.4 ms** |
| 3-term AND (`cargo test failure`), `ORDER BY rank` | **29.1 ms** |
| CJK trigram (`你好世界`), `ORDER BY rank` | **13.3 ms** |
| Rare term (`quokka`), `ORDER BY rank` | **1.8 ms** |
| 2-char LIKE fallback (`%ox%`), `LIMIT 20` | **0.07 ms** (see caveat) |

Caveat (honest reading): the LIKE timing is **not** a full-scan cost. With
`LIMIT 20` and no `ORDER BY`, SQLite stops after the first 20 matches, so a
common 2-char term looks free. The expensive LIKE case is a *rare* short
term, which scans the whole table before returning few rows - that case was
not timed in this harness and must be timed at implementation. The FTS5
timings above are real ranking costs (BM25 over the matched set), and all of
them are far below a 100 ms interactive budget at 100k messages.

### 5.4 Finding that changed the schema: latest/around reads need a real table

Measured in the same harness, on the DS-Bot schema shape:

| Query | Time |
| --- | --- |
| Latest read: `WHERE session = 'session-000' ORDER BY seq DESC LIMIT 12` | **23.5 ms** |

23 ms is not slow in absolute terms, but it is a **full scan of all 100,000
rows** (the FTS5 table's `session` column is UNINDEXED by design, so there
is nothing to seek on) and it grows linearly with the corpus. At 1M messages
it would be ~230 ms for every "show latest" call. That is why section 4.2
adds a plain `messages` table with a real `(session_id, seq)` index and an
external-content FTS5 table, instead of copying DS-Bot's content-storing
FTS5 table verbatim. The extra cost is one more write per indexed message
inside the same transaction; the read paths that users hit constantly
(latest, around, per-session listing) become index seeks instead of scans.

### 5.5 Extrapolation and budgets (labelled as extrapolation, not measurement)

- Size scales ~linearly with message text: 100k short messages -> 33.76 MiB,
  so **~340 MiB per 1M messages** of this shape is the planning figure
  (extrapolated, not measured). Real ZenCode messages are longer than the
  ~70-char synthetic ones (tool outputs especially), so the honest planning
  assumption is **larger**, and the design caps indexed tool output at 4,000
  chars (section 4.3) precisely to bound this.
- Budgets for implementation acceptance (at 100k messages, warm cache, this
  VM class): search p50 < 100 ms, latest/around < 5 ms, catch-up of one
  changed session < 1 s. Re-measure on the real implementation; these
  budgets are targets, not results.
- Not measured here: 1M-message behaviour, cold-cache timings, concurrent
  CLI + Desktop writers, real-journal text length distribution, and the
  2-char CJK FTS5 rule (section 4.5, Fix 1). Each is listed as an open
  verification item in section 8.

## 6. Risks and open questions

| # | Risk / question | Mitigation / owner |
| --- | --- | --- |
| R1 | Dossier sections 7/8 cited by the assignment do not exist in the dossier on file (section 0). | Leader to confirm the intended reference before implementation starts. Design does not depend on it. |
| R2 | Trigram index is ~56% of the DB; long tool outputs bloat it. | 4,000-char cap on indexed tool text (4.3); `VACUUM` on rebuild only, never automatic. |
| R3 | FTS5 query syntax errors from user input (`"`, `*`, `^`, unbalanced quotes). | Bound `MATCH ?` parameter + term quoting (double `"`), same as DS-Bot; malformed query returns exit code 2 with the offending term named. |
| R4 | LIKE fallback on a rare short term is a full scan (5.3 caveat). | Time it at implementation; if it exceeds budget, restrict short-term search to the current session or require a long term alongside. |
| R5 | Two writers (CLI + Desktop) on one WAL file. | `busy_timeout` 5 s + single-writer discipline + watermark re-check (4.4); search never blocks past the timeout. |
| R6 | Index staleness: a session edited while ZenCode is running (external edit, crash mid-write). | Watermark is (mtime, length, through_seq); a journal that shrank or changed mid-read is re-indexed from scratch for that session. |
| R7 | Privacy: the index duplicates conversation text in a second file. | Same directory, same permissions as journals; listed with private files; deleted with the session (4.3). No redaction inside the index - redaction happens at output time (`--redact`). |
| R8 | `rusqlite` version drift: FTS5 verified only for bundled SQLite 3.50.2 (5.1). | Re-run the `/tmp/fts5-check` harness when the version is pinned at implementation; fail the build if the trigram table cannot be created (a test, not a hope). |
| R9 | Scope: is Desktop search in DS-4 or a follow-up? | This design guarantees the shared engine only (4.7). Desktop UI needs its own design + Leader decision. |
| R10 | Archived sessions: searched by default or only with a flag? | Proposed: included by default, shown with an `archived` marker; Leader to confirm. |

## 7. Acceptance criteria (for the future implementation, not this document)

1. `zen sessions search` works offline on a fresh checkout with no prior
   index (builds it from journals on first run).
2. Search finds a message by words from a session in another project dir.
3. `ORDER BY rank` results put the most relevant message first on a fixed
   fixture (test with a hand-built journal, not synthetic data).
4. `--around` opens the hit in full with 6 neighbours each side.
5. Deleting a session removes its index rows; deleting the index file and
   re-searching rebuilds identical results.
6. Budgets in 5.5 hold at 100k messages on the implementation (re-measured).
7. No new network use; `cargo deny` clean for the new dependency; Windows
   path handling follows `session.rs` (`strip_verbatim_prefix`, `encode_cwd`).
8. The 2-char CJK rule (4.5 Fix 1) is decided by a passing test, not by
   assertion in this document.

## 8. Verification owed (not yet done)

- [ ] Leader confirms the dossier section 7/8 reference (section 0).
- [ ] 1M-message measurement (size + query times) on real-shape text.
- [ ] Cold-cache timings and CLI+Desktop concurrent-writer test.
- [ ] 2-char CJK term against FTS5 trigram (decides 4.5 Fix 1).
- [ ] Rare-short-term LIKE full-scan timing (5.3 caveat).
- [ ] Real-journal length distribution (to set the tool-output cap from data,
      not from the 4,000-char guess).

## 9. References

- [S] Fei2411/DS-Bot @ `71d344b`, `ds-bot/src/host/chat-index.js` (whole
  file), `ds-bot/src/host/groups.js:1029-1090`, `ds-bot/src/host/text.js:26-31`,
  `ds-bot/src/host/constants.js:83-91`, `ds-bot/src/host/privacy.js:70-75`,
  `ds-bot/test/history.test.js:64-77`. License: Apache-2.0. Ideas only; no
  code copied.
- [S] Dossier `~/workspace/zencode/study-ds-bot-fei2411-2026-11-05.md`
  (SHA256 `c0f94baa…fec30`, 5 sections; see section 0 for the mismatch).
  Note the dossier itself says its upstream claims are `[INFERRED]` from
  docs, not from source (section 4, critic 1) - this design replaces those
  with the primary-source reading above.
- [S] ZenCode repo @ `647fcc0`: `crates/zen-cli/src/session.rs`
  (`encode_cwd`, `session_path`, `find_session_path`,
  `list_sessions_for_cwd`), `crates/zen-cli/src/session_mgmt.rs`
  (`SessionInfo`, `list_all_sessions`, `short_id`), `crates/zen-cli/src/cli.rs`
  (`sessions` subcommands), `docs/session-format.md` (export v1, frozen),
  `docs/design/stats-page.md` (design-doc naming convention followed here).
- [S] FTS5 verification harness `/tmp/fts5-check` (throwaway, not in repo):
  `rusqlite 0.37.0` + `libsqlite3-sys 0.35.0` bundled SQLite **3.50.2**,
  `rustc 1.99.1`; `cargo run --release` exit code 0; all numbers in section 5
  are its output.
- [M] SQLite FTS5 documentation (trigram tokenizer, external-content tables,
  BM25 `rank`): background for section 4.2/4.5; the design's claims about
  ZenCode rest on the measured harness, not on the docs alone.
- [I] Size extrapolation to 1M messages (5.5) and the 4,000-char tool-output
  cap (4.3) are judgement calls, labelled as such, pending the measurements
  in section 8.
