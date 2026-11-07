# P0-3 Coordination Decisions

Status: decision record for the P0-3 task coordination work (steps 1-7
merged, follow-up in progress). Revision checked: `origin/main`
`7e2301fce63a0d8071d80a89503f454cf5317de0` (merge of PR #56).
Follow-up PR deadline: 19:30 18/11/2026. Items marked [decided] are
already implemented in merged code; [open] means no final decision
yet. Line references are to `crates/zen-cli/src/tasks.rs` at that
revision unless stated otherwise.

## Q1 [decided] Registry: one append-only JSONL file

The registry is a single append-only JSONL file at
`<home>/.zen/tasks.jsonl`; each line is a full snapshot of a card
following the design schema (section 4.5a). On read, the newest
snapshot of an id wins, and the file doubles as the claim history.
Sources: tasks.rs lines 3-10 and 193-195.

## Q2 [decided, step 1] Observe-only claim

`zen task claim` records the holder and lease but does not block
another seat from claiming the same task; a repeated holder is
reported, not refused. Sources: tasks.rs lines 12-15, `claim_task`
line 302.

## Q3 [decided, step 2] Lease

`expires_at = last_heartbeat + 8 hours` (`LEASE_HOURS = 8`, line 63).
A heartbeat refreshes `last_heartbeat` and `expires_at`, bumps the
lease version by 1, and never moves the holder; a heartbeat from
another seat is only flagged, not blocked. Note: 8 hours is a design
proposal (section 4.2, one work shift), not yet measured on real team
data. Sources: tasks.rs lines 17-23 and 61-63, `heartbeat_task`
line 480.

## Q4 [decided, step 3] Manual sweep at shift handover

A DOING card whose lease has expired is returned to the queue as
ORPHAN (`reassign_count` +1; the counter is not reset when the card
is claimed again). An ORPHAN card nobody re-claims counts +1 on each
sweep; at `DEAD_LETTER_AFTER_REASSIGNS = 3` (line 582) the sweep parks
it as DEAD-LETTER for a Leader decision - never deleted, never
closed. Note: 3 is also a design proposal, not yet measured; the doc
comment at lines 577-581 says so. Sources: tasks.rs lines 25-31 and
577-582, `sweep_tasks` line 648.

## Q5 [decided, step 4] CAS and the idempotency key

`zen task claim --expected-version <n>` succeeds only when the lease
version on the card equals n; otherwise it is refused as CONFLICT
(exit 7) and writes nothing. The key is
`claim:<task_id>:<actor>:<expected_version>` (Leader decision,
tin 7242). A successful guarded claim records the key on the card; a
retry with the same key is a no-op success when the lease is the
post-state of that claim (same holder, version n + 1). A claim
without `--expected-version` keeps the step-1 behaviour. Amended by
Q10: the replay condition is now version `>= n + 1`, and an ORPHAN
card never replays. Sources: tasks.rs lines 33-41 and 281-285,
`claim_task_cas` line 364.

## Q6 [decided, step 5] Refusal on DEAD-LETTER cards

Claim (plain and CAS) and heartbeat on a dead-lettered card are both
refused as CONFLICT (exit 7), write no snapshot, and change no
state; in `claim_task_cas` the dead-letter check runs before the
replay check. Sources: Leader tin 7328; tasks.rs lines 43-48 and
378-380, `dead_letter_message` line 290.

## Q7 [decided, step 6] Revive

`zen task revive <id> --by <seat>` applies only to a DEAD-LETTER
card: it returns the card to Queued, clears `dead_lettered` and
`orphaned`, and bumps the card version by 1; the lease,
`reassign_count` and `orphan_reason` stay on the card as history.
The CLI records the acting seat via --by and does not check
permissions itself; revive is a Leader decision under the process.
Revive on a card that is not dead-lettered is refused as CONFLICT
(exit 7); an unknown id returns NOT_FOUND (exit 3). Sources: Leader
tin 7351; `revive_task` lines 545-575.

## Q8 [decided, step 7] Floor guard CI

The branch or PR body must name a work code in the prescribed
format; two open PRs with the same work code are flagged for the
Leader to decide. Warn-only until the end of 14/11/2026, then
merge-blocking; the `floor-change-approved` label is the only
override; dependabot branches are exempt; the `min_rust_tests`
threshold rose from 1252 to 1300. Sources: PR #66, merged (main
478fd15 before the PR #56 merge); this part lives outside tasks.rs,
so no line numbers are quoted at this revision.

## Q9 [decided] CONFLICT exit code stays 7

The CONFLICT exit code stays 7, not 4. Final Leader decision, tin
7800 and tin 7803, 08/11/2026, on the step-4 decision (tin 7264):
table B2 is unchanged (4 = validation, 7 = conflict); do not ask
again. Sources: `crates/zen-cli/src/lib.rs` line 80
(`pub const CONFLICT: i32 = 7`); `docs/exit-codes.md` line 18 (7 is
conflict) and line 15 (4 is validation).

## Q10 [decided in design, code in this PR] Widened replay across heartbeats

Leader decision tin 7803, 08/11/2026. The new replay condition in
`claim_task_cas` is: the idempotency key on the card matches the
retry key AND `lease.holder == by` AND `lease.version >=
expected + 1` AND the card is not effectively ORPHAN.

1. A card the sweep has parked as ORPHAN does NOT replay: the
   `!orphaned` arm (effective state derived by `aux_state`, line
   590) is added; the retry falls through to the version check and
   returns CONFLICT (the lease version is still `expected + 1`,
   which differs from `expected`), so the holder re-reads the state
   and claims fresh. A fresh claim after ORPHAN already works: a new
   claim issues a fresh lease, and once the lease is live the card
   no longer reads as orphan under `aux_state` (revive also clears
   the flag, line 567). Like DEAD-LETTER, which never replayed,
   ORPHAN never replays.
2. A heartbeat from another seat still replays for the holder: the
   version is only a monotone counter; the real evidence is the
   idempotency key and `lease.holder == by`. A heartbeat never moves
   the holder and never changes the key.
3. The CONFLICT exit code stays 7 (see Q9).

Six new tests cover the change (tasks.rs test module): replay after
one heartbeat, after two heartbeats, after another seat's heartbeat,
conflict after another seat's CAS claim, refusal after heartbeat
then dead-letter, and conflict (not replay) after an orphan sweep.
The pre-existing intervening-claim conflict test is kept unchanged,
and a full-suite regression run covers the rest. PR deadline
19:30 18/11/2026.

## Q11 [open] Precision pilot after 14/11/2026

Measure coordination precision on real shifts after the floor guard
turns blocking; no measurements exist at the time of writing.

## Known trade-off (not yet decided)

A plain (non-CAS) claim by the same holder between the guarded
claim and the retry creates a fresh lease (new `acquired_at`), bumps
the version by 1, and keeps the old key, so under the minimal rule
it looks like a heartbeat and the old retry is treated as a replay.
The final coordination result is unchanged (the same holder still
holds the task, only with a newer lease). The strict variant -
walking the snapshot history of the id in the append-only registry
and checking that only heartbeat snapshots sit between the
key-writing snapshot and the current one - needs a dedicated history
reader, because `load_registry` folds snapshots and drops history;
it is larger and riskier and is not recommended for this follow-up.
The Leader has not decided this point; the trade-off is recorded
here as-is.
