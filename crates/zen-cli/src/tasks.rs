//! P0-3: the multi-seat task registry (observe-only).
//!
//! The registry is a single append-only JSONL file at
//! `<home>/.zen/tasks.jsonl` — the same `.zen` tree that already holds
//! the session transcripts (`.zen/projects/<cwd>/<id>.jsonl`, the CLI's
//! run history) and the cost ledgers (`.zen/cost/<id>.json`). Each line
//! is one full task-card snapshot in the schema of the P0-3 design
//! scout (2026-11-06, section 4.5a); reading folds the lines in order
//! and the latest snapshot for an id wins, so the file doubles as the
//! claim history.
//!
//! Step 1 is observe-only by design: `zen task claim` records the
//! holder and a lease, but it never blocks another seat from claiming
//! the same task — an overlap is reported, not rejected — and no
//! existing command changes behavior.
//!
//! Step 2 adds `zen task heartbeat`: a heartbeat refreshes the
//! lease's `last_heartbeat`/`expires_at` and bumps its `version`,
//! still observe-only — a heartbeat from a seat other than the holder
//! is recorded and flagged, never rejected, and it never transfers
//! the lease. `list`/`show` display the derived lease status
//! (`active`/`expired`); an expired lease changes nothing here —
//! returning the task to the queue is the sweep of a later step.
//!
//! Step 3 adds `zen task sweep`, the handover sweep of section 4.1:
//! a DOING task whose lease has expired is returned to the queue as
//! ORPHAN (the holder stays on the card as history and the count of
//! reassignments grows), and an orphan nobody claims again is
//! counted once per sweep until it is parked as DEAD-LETTER for a
//! Leader decision - never deleted, never closed. The sweep is a
//! manual command run at a handover; nothing runs in the background.
//!
//! Step 4 adds claim compare-and-swap plus an idempotency key
//! (design sections 4.2 and 4.3): `zen task claim --expected-version
//! <n>` only succeeds when the lease version on the card equals `n`;
//! otherwise it is refused with exit code 7 (B2 CONFLICT) and writes
//! nothing. A successful guarded claim records the key
//! `claim:<task_id>:<actor>:<expected_version>` on the card; a retry
//! with the same key is a no-op that reports success without writing
//! a duplicate snapshot. A claim without `--expected-version` keeps
//! the observe-only behaviour of step 1.
//!
//! Step 5 guards DEAD-LETTER cards (Leader decision, tin 7328,
//! 07/11/2026): `zen task claim` and `zen task heartbeat` on a card
//! the sweep has parked as dead-letter are refused with exit code 7
//! (B2 CONFLICT) and write nothing - the card is terminal and only a
//! Leader decision can revive it. There is no revive command in this
//! step; inventing one is out of scope.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::{OutputFormat, TaskArgs, TaskSubcommand};
use crate::exit_code;

/// Lease length proposed by the design (section 4.2): one 8-hour
/// shift. A lease is a liveness signal, not a deadline for the task.
pub const LEASE_HOURS: i64 = 8;

/// One of the seven statuses of the v3.0 state machine. BLOCKED,
/// ORPHAN and DEAD-LETTER are auxiliary states: a card carries them
/// through its `blocker` field, its `orphaned` flag and its
/// `dead_lettered` flag, not through this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Queued,
    Ready,
    Assigned,
    Accepted,
    Doing,
    Verifying,
    Done,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Ready => "ready",
            TaskStatus::Assigned => "assigned",
            TaskStatus::Accepted => "accepted",
            TaskStatus::Doing => "doing",
            TaskStatus::Verifying => "verifying",
            TaskStatus::Done => "done",
        }
    }
}

/// Who holds a task and since when (design section 4.5a). `version`
/// grows on every claim/renew so a later step can compare-and-swap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    // Every field defaults so a snapshot written before a field
    // existed (or written by hand) still loads.
    #[serde(default)]
    pub holder: String,
    #[serde(default)]
    pub acquired_at: String,
    #[serde(default)]
    pub last_heartbeat: String,
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub version: u64,
}

/// The mandatory four fields of the auxiliary BLOCKED state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocker {
    pub what: String,
    pub tried: String,
    pub who: String,
    pub deadline_impact: String,
}

/// One verification receipt on a card (design section 4.5e).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    #[serde(default)]
    pub verifier: Option<String>,
    pub command: String,
    pub result: String,
    pub at: String,
}

/// A task card, design section 4.5a. Optional fields stay absent from
/// the JSONL line until they carry a value, so cards written by hand
/// or by later steps remain readable by this one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TaskCard {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub verifier: Option<String>,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub lease: Option<Lease>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub blocker: Option<Blocker>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default)]
    pub anti_jobs: Vec<String>,
    #[serde(default)]
    pub orphaned: bool,
    /// How many times the sweep has returned this task to the queue
    /// (step 3). History: a later claim never resets it.
    #[serde(default)]
    pub reassign_count: u32,
    /// Set by the sweep once a task has been returned to the queue
    /// `DEAD_LETTER_AFTER_REASSIGNS` times without a new claim: the
    /// task is parked for a Leader decision, never deleted or closed.
    #[serde(default)]
    pub dead_lettered: bool,
    /// Why the sweep orphaned this task (the lease expiry it saw).
    #[serde(default)]
    pub orphan_reason: Option<String>,
    /// The idempotency key of the latest guarded claim
    /// (`claim:<task_id>:<actor>:<expected_version>`, step 4). A
    /// retry with the same key is a no-op; a claim without
    /// `--expected-version` leaves the recorded key untouched.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub version: u64,
}

/// The registry file for a home directory: `<home>/.zen/tasks.jsonl`.
pub fn registry_path(home: &Path) -> PathBuf {
    home.join(".zen").join("tasks.jsonl")
}

/// Load the registry, folding snapshots in file order so the latest
/// line for an id wins. A missing or empty file is an empty registry,
/// not an error; a malformed line is an error that names the line.
pub fn load_registry(path: &Path) -> Result<Vec<TaskCard>> {
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading task registry {}", path.display()))
        }
    };
    let mut cards: Vec<TaskCard> = Vec::new();
    for (i, raw) in data.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let card: TaskCard = serde_json::from_str(line).with_context(|| {
            format!(
                "task registry {} line {} is not a valid task card",
                path.display(),
                i + 1
            )
        })?;
        if let Some(slot) = cards.iter_mut().find(|c| c.id == card.id) {
            *slot = card;
        } else {
            cards.push(card);
        }
    }
    Ok(cards)
}

/// Append one snapshot line for `card`, creating the `.zen` tree and
/// the registry file when they do not exist yet.
pub fn append_snapshot(path: &Path, card: &TaskCard) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening task registry {}", path.display()))?;
    let mut line = serde_json::to_string(card)?;
    line.push('\n');
    file.write_all(line.as_bytes())?;
    Ok(())
}

pub fn find_task<'a>(cards: &'a [TaskCard], id: &str) -> Option<&'a TaskCard> {
    cards.iter().find(|c| c.id == id)
}

/// The result of a recorded claim: the updated card plus whoever held
/// the task before (if anyone), so the caller can flag the overlap.
/// `replayed` is true when the claim was already applied under the
/// same idempotency key and no new snapshot was written (step 4).
#[derive(Debug)]
pub struct ClaimOutcome {
    pub card: TaskCard,
    pub previous_holder: Option<String>,
    pub replayed: bool,
    pub idempotency_key: Option<String>,
}

/// The outcome of a guarded claim (`zen task claim
/// --expected-version`, step 4): the claim was recorded, it was
/// already applied under the same key, the lease version the
/// caller expected no longer matches the card, or the card has been
/// parked as DEAD-LETTER (step 5) and the claim is refused.
pub enum CasClaim {
    Recorded(ClaimOutcome),
    Replayed(ClaimOutcome),
    Conflict {
        expected: u64,
        actual: u64,
    },
    /// The card has been parked as DEAD-LETTER by the sweep (step 5):
    /// the claim is refused and nothing is written.
    DeadLetter,
}

/// Build the idempotency key of a guarded claim (Leader decision,
/// tin 7242, 07/11/2026): `claim:<task_id>:<actor>:<expected_version>`.
pub fn claim_idempotency_key(id: &str, by: &str, expected_version: u64) -> String {
    format!("claim:{id}:{by}:{expected_version}")
}

/// Message for a refused operation on a DEAD-LETTER card: the card is
/// terminal and only a Leader decision can revive it, through the
/// revive command added in step 6 (Leader tin 7351).
fn dead_letter_message(id: &str) -> String {
    format!(
        "task '{id}' is dead-lettered: only a Leader decision can revive it (zen task revive {id} --by <seat>)"
    )
}

/// Record `by` as the holder of task `id` and append the new snapshot.
///
/// Observe-only: a task that already has a holder is still claimed —
/// the previous holder is reported in the outcome, never treated as a
/// conflict. Returns `None` when the registry has no task with `id`;
/// in that case nothing is written.
pub fn claim_task(
    path: &Path,
    id: &str,
    by: &str,
    now: DateTime<Utc>,
) -> Result<Option<ClaimOutcome>> {
    let cards = load_registry(path)?;
    let Some(existing) = find_task(&cards, id) else {
        return Ok(None);
    };
    if existing.dead_lettered {
        return Err(crate::coded_error(
            exit_code::CONFLICT,
            dead_letter_message(id),
        ));
    }
    let mut card = existing.clone();
    let previous_holder = card
        .lease
        .as_ref()
        .map(|l| l.holder.clone())
        .or_else(|| card.owner.clone());
    let lease_version = card.lease.as_ref().map_or(1, |l| l.version + 1);
    card.owner = Some(by.to_string());
    card.lease = Some(Lease {
        holder: by.to_string(),
        acquired_at: now.to_rfc3339(),
        last_heartbeat: now.to_rfc3339(),
        expires_at: (now + Duration::hours(LEASE_HOURS)).to_rfc3339(),
        version: lease_version,
    });
    card.version += 1;
    card.updated_at = now.to_rfc3339();
    append_snapshot(path, &card)?;
    let idempotency_key = card.idempotency_key.clone();
    Ok(Some(ClaimOutcome {
        card,
        previous_holder,
        replayed: false,
        idempotency_key,
    }))
}

/// Record a guarded claim for task `id` from seat `by`.
///
/// When `expected_version` is `None` this is exactly the step-1
/// observe-only claim (recorded, never blocked). When it is `Some`,
/// the claim is a compare-and-swap on the lease `version` (design
/// sections 4.2 and 4.3):
///
/// * the lease version on the card must equal `expected_version`,
///   otherwise the claim is refused as `CasClaim::Conflict` and
///   nothing is written (a task with no lease yet reads as version 0);
/// * a successful guarded claim records the idempotency key
///   `claim:<task_id>:<actor>:<expected_version>` on the card;
/// * a retry whose key is already on the card *and* whose lease is
///   exactly the post-state of that claim (same holder, version
///   `expected_version + 1`) is a no-op `CasClaim::Replayed`: it
///   succeeds without writing a duplicate snapshot.
///
/// Returns `None` when the registry has no task with `id`; in that
/// case nothing is written.
pub fn claim_task_cas(
    path: &Path,
    id: &str,
    by: &str,
    expected_version: Option<u64>,
    now: DateTime<Utc>,
) -> Result<Option<CasClaim>> {
    let Some(expected) = expected_version else {
        return Ok(claim_task(path, id, by, now)?.map(CasClaim::Recorded));
    };
    let cards = load_registry(path)?;
    let Some(existing) = find_task(&cards, id) else {
        return Ok(None);
    };
    if existing.dead_lettered {
        return Ok(Some(CasClaim::DeadLetter));
    }
    let key = claim_idempotency_key(id, by, expected);
    let current_version = existing.lease.as_ref().map_or(0, |l| l.version);
    if existing.idempotency_key.as_deref() == Some(key.as_str())
        && existing
            .lease
            .as_ref()
            .is_some_and(|l| l.holder == by && l.version == expected.saturating_add(1))
    {
        return Ok(Some(CasClaim::Replayed(ClaimOutcome {
            card: existing.clone(),
            previous_holder: existing
                .lease
                .as_ref()
                .map(|l| l.holder.clone())
                .or_else(|| existing.owner.clone()),
            replayed: true,
            idempotency_key: Some(key),
        })));
    }
    if current_version != expected {
        return Ok(Some(CasClaim::Conflict {
            expected,
            actual: current_version,
        }));
    }
    let mut card = existing.clone();
    let previous_holder = card
        .lease
        .as_ref()
        .map(|l| l.holder.clone())
        .or_else(|| card.owner.clone());
    card.owner = Some(by.to_string());
    card.lease = Some(Lease {
        holder: by.to_string(),
        acquired_at: now.to_rfc3339(),
        last_heartbeat: now.to_rfc3339(),
        expires_at: (now + Duration::hours(LEASE_HOURS)).to_rfc3339(),
        version: expected.saturating_add(1),
    });
    card.idempotency_key = Some(key.clone());
    card.version += 1;
    card.updated_at = now.to_rfc3339();
    append_snapshot(path, &card)?;
    Ok(Some(CasClaim::Recorded(ClaimOutcome {
        card,
        previous_holder,
        replayed: false,
        idempotency_key: Some(key),
    })))
}

/// Whether a lease is still live at a given moment, derived only from
/// its `expires_at` (design section 4.2). This is a display value:
/// step 2 never mutates a card because its lease expired - handing
/// the task back to the queue is the sweep's job (step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    Active,
    Expired,
}

impl LeaseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            LeaseStatus::Active => "active",
            LeaseStatus::Expired => "expired",
        }
    }
}

/// The lease status of `lease` at `now`. An `expires_at` that cannot
/// be parsed cannot prove the lease is live, so it reads as expired.
pub fn lease_status(lease: &Lease, now: DateTime<Utc>) -> LeaseStatus {
    match DateTime::parse_from_rfc3339(&lease.expires_at) {
        Ok(expires) if expires.with_timezone(&Utc) > now => LeaseStatus::Active,
        _ => LeaseStatus::Expired,
    }
}

/// The result of a recorded heartbeat: the updated card plus whether
/// the heartbeat came from a seat other than the lease holder.
#[derive(Debug)]
pub struct HeartbeatOutcome {
    pub card: TaskCard,
    pub flagged: bool,
}

/// Record a heartbeat for task `id` from seat `by` and append the new
/// snapshot.
///
/// The heartbeat refreshes `last_heartbeat` to `now`, extends
/// `expires_at` to `now + LEASE_HOURS`, and bumps the lease `version`.
/// A heartbeat never transfers the lease: the holder stays whoever the
/// lease names. A heartbeat from a different seat is still recorded -
/// the outcome flags it, exactly like an overlapping claim - and a
/// task with no lease yet gains one held by `by`. Returns `None` when
/// the registry has no task with `id`; in that case nothing is
/// written.
pub fn heartbeat_task(
    path: &Path,
    id: &str,
    by: &str,
    now: DateTime<Utc>,
) -> Result<Option<HeartbeatOutcome>> {
    let cards = load_registry(path)?;
    let Some(existing) = find_task(&cards, id) else {
        return Ok(None);
    };
    if existing.dead_lettered {
        return Err(crate::coded_error(
            exit_code::CONFLICT,
            dead_letter_message(id),
        ));
    }
    let mut card = existing.clone();
    let flagged = if let Some(lease) = card.lease.as_mut() {
        let flagged = lease.holder != by;
        lease.last_heartbeat = now.to_rfc3339();
        lease.expires_at = (now + Duration::hours(LEASE_HOURS)).to_rfc3339();
        lease.version += 1;
        flagged
    } else {
        let flagged = card.owner.as_deref().is_some_and(|owner| owner != by);
        if card.owner.is_none() {
            card.owner = Some(by.to_string());
        }
        card.lease = Some(Lease {
            holder: by.to_string(),
            acquired_at: now.to_rfc3339(),
            last_heartbeat: now.to_rfc3339(),
            expires_at: (now + Duration::hours(LEASE_HOURS)).to_rfc3339(),
            version: 1,
        });
        flagged
    };
    card.version += 1;
    card.updated_at = now.to_rfc3339();
    append_snapshot(path, &card)?;
    Ok(Some(HeartbeatOutcome { card, flagged }))
}

/// The result of a revived task: the updated card plus the seat that
/// recorded the revive, so the caller can show who revived it.
#[derive(Debug)]
pub struct ReviveOutcome {
    pub card: TaskCard,
    pub revived_by: String,
}

/// Revive a DEAD-LETTER task (step 6, Leader tin 7351) and append the
/// new snapshot.
///
/// The card returns to the queue (`Queued`) with the dead-letter park
/// cleared, so it can be claimed again; the lease, the reassign count
/// and the orphan reason stay on the card as history, and every older
/// snapshot stays in the append-only registry. A revive is a Leader
/// decision by process rule: the CLI records the seat that ran it
/// (`--by`) but performs no permission check of its own.
///
/// Returns `None` when the registry has no task with `id`; in that
/// case nothing is written. A task that is not dead-lettered is
/// refused with CONFLICT (exit 7, B2 state mismatch, same family as
/// the step-5 dead-letter guard) and nothing is written.
pub fn revive_task(
    path: &Path,
    id: &str,
    by: &str,
    now: DateTime<Utc>,
) -> Result<Option<ReviveOutcome>> {
    let cards = load_registry(path)?;
    let Some(existing) = find_task(&cards, id) else {
        return Ok(None);
    };
    if !existing.dead_lettered {
        return Err(crate::coded_error(
            exit_code::CONFLICT,
            format!(
                "task '{id}' is not dead-lettered (status {}): only a dead-lettered task can be revived",
                existing.status.as_str()
            ),
        ));
    }
    let mut card = existing.clone();
    card.status = TaskStatus::Queued;
    card.dead_lettered = false;
    card.orphaned = false;
    card.version += 1;
    card.updated_at = now.to_rfc3339();
    append_snapshot(path, &card)?;
    Ok(Some(ReviveOutcome {
        card,
        revived_by: by.to_string(),
    }))
}

/// How many times a task may be returned to the queue without a new
/// claim before the sweep parks it as DEAD-LETTER for a Leader
/// decision (design sections 4.1 and 4.2). Like the 8-hour lease,
/// the number is a proposal of the design, not yet measured on real
/// team data.
pub const DEAD_LETTER_AFTER_REASSIGNS: u32 = 3;

/// The auxiliary state a card is in at a given moment, derived from
/// its flags and its lease: `dead-letter` once the sweep has parked
/// the card for a Leader decision, `orphan` while the card sits back
/// in the queue with no live lease after an expiry. A card a seat
/// has claimed again carries a live lease, so it reads as neither;
/// the flags stay on the card as history.
pub fn aux_state(card: &TaskCard, now: DateTime<Utc>) -> Option<&'static str> {
    if card.dead_lettered {
        return Some("dead-letter");
    }
    if card.orphaned {
        let live = card
            .lease
            .as_ref()
            .is_some_and(|lease| lease_status(lease, now) == LeaseStatus::Active);
        if !live {
            return Some("orphan");
        }
    }
    None
}

/// One task the sweep moved (or, in a dry run, would move): the id,
/// the state it came from, the state it went to, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SweepEntry {
    pub id: String,
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// The result of one sweep pass: one entry per changed task, in
/// registry order.
#[derive(Debug)]
pub struct SweepOutcome {
    pub entries: Vec<SweepEntry>,
    pub dry_run: bool,
}

/// Run one handover sweep over the registry (design sections 4.1,
/// 4.2, and step 3 of section 6). The sweep is a manual command run
/// at a shift handover; it starts nothing in the background.
///
/// Each task changes at most once per pass:
/// - a DOING task whose lease has expired is returned to the queue
///   as ORPHAN: the holder stays on the card as history, the reason
///   names the expiry, and `reassign_count` grows by one - being
///   returned to the queue is one more offering of the task. A
///   DOING task without a lease has no expiry to prove and is left
///   alone;
/// - an ORPHAN task sitting in the queue still without a new claim
///   is offered again, so its `reassign_count` grows by one per
///   sweep as well; once the count has reached
///   `DEAD_LETTER_AFTER_REASSIGNS`, the next sweep parks it as
///   DEAD-LETTER - never deleted, never closed - to wait for a
///   Leader decision.
///
/// A claim after an orphan gives the card a live lease again, which
/// is how the sweep tells "claimed again" from "still waiting"; the
/// count is history and is never reset here. With `dry_run` the
/// same entries are computed but no snapshot is written. An empty
/// registry, or one with nothing expired, yields zero entries and
/// writes nothing.
pub fn sweep_tasks(path: &Path, now: DateTime<Utc>, dry_run: bool) -> Result<SweepOutcome> {
    let cards = load_registry(path)?;
    let mut outcome = SweepOutcome {
        entries: Vec::new(),
        dry_run,
    };
    for card in &cards {
        if card.dead_lettered {
            continue;
        }
        let lease_live = card
            .lease
            .as_ref()
            .is_some_and(|lease| lease_status(lease, now) == LeaseStatus::Active);
        if lease_live {
            continue;
        }
        if card.status == TaskStatus::Doing {
            let Some(lease) = &card.lease else {
                continue;
            };
            let reason = format!(
                "lease expired at {} (holder {}, last heartbeat {})",
                lease.expires_at, lease.holder, lease.last_heartbeat
            );
            let mut next = card.clone();
            next.status = TaskStatus::Queued;
            next.orphaned = true;
            next.reassign_count += 1;
            next.orphan_reason = Some(reason.clone());
            next.version += 1;
            next.updated_at = now.to_rfc3339();
            if !dry_run {
                append_snapshot(path, &next)?;
            }
            outcome.entries.push(SweepEntry {
                id: card.id.clone(),
                from: TaskStatus::Doing.as_str().to_string(),
                to: "orphan".to_string(),
                reason,
            });
        } else if card.orphaned && card.status == TaskStatus::Queued {
            if card.reassign_count >= DEAD_LETTER_AFTER_REASSIGNS {
                let mut next = card.clone();
                next.dead_lettered = true;
                next.version += 1;
                next.updated_at = now.to_rfc3339();
                if !dry_run {
                    append_snapshot(path, &next)?;
                }
                outcome.entries.push(SweepEntry {
                    id: card.id.clone(),
                    from: "orphan".to_string(),
                    to: "dead-letter".to_string(),
                    reason: format!(
                        "returned to the queue {} times without a new claim; parked for a Leader decision (never deleted, never closed)",
                        card.reassign_count
                    ),
                });
            } else {
                let mut next = card.clone();
                next.reassign_count += 1;
                next.version += 1;
                next.updated_at = now.to_rfc3339();
                if !dry_run {
                    append_snapshot(path, &next)?;
                }
                outcome.entries.push(SweepEntry {
                    id: card.id.clone(),
                    from: "orphan".to_string(),
                    to: "orphan".to_string(),
                    reason: format!(
                        "still unclaimed at this sweep; reassign_count {} -> {}",
                        card.reassign_count, next.reassign_count
                    ),
                });
            }
        }
    }
    Ok(outcome)
}

pub fn render_sweep_text(outcome: &SweepOutcome) -> String {
    let mut out = if outcome.dry_run {
        format!(
            "Sweep (dry-run): {} task(s) would change.\n",
            outcome.entries.len()
        )
    } else {
        format!("Sweep: {} task(s) changed.\n", outcome.entries.len())
    };
    for entry in &outcome.entries {
        out.push_str(&format!(
            "{}: {} -> {} ({})\n",
            entry.id, entry.from, entry.to, entry.reason
        ));
    }
    out
}

pub fn render_heartbeat_text(outcome: &HeartbeatOutcome) -> String {
    let card = &outcome.card;
    let mut out = format!("Recorded heartbeat: {}.\n", card.id);
    if let Some(lease) = &card.lease {
        out.push_str(&format!(
            "lease: holder={} version={} last_heartbeat={} expires_at={}\n",
            lease.holder, lease.version, lease.last_heartbeat, lease.expires_at
        ));
    }
    if outcome.flagged {
        out.push_str(
            "Note: heartbeat from a seat other than the holder; the overlap is flagged, not blocked.\n",
        );
    }
    out.push_str("Observe-only: the heartbeat is recorded; no seat was blocked.\n");
    out
}

pub fn render_revive_text(outcome: &ReviveOutcome) -> String {
    let card = &outcome.card;
    format!(
        "Revived task {} (by {}): back in the queue as queued, ready to be claimed again.\n",
        card.id, outcome.revived_by
    )
}

pub fn render_list_text(cards: &[TaskCard], now: DateTime<Utc>) -> String {
    if cards.is_empty() {
        return "No tasks in the registry.\n".to_string();
    }
    let mut out = format!(
        "{:<14} {:<10} {:<18} {:<8} {:<11} {}\n",
        "ID", "STATUS", "OWNER", "LEASE", "STATE", "TITLE"
    );
    for card in cards {
        let lease = card
            .lease
            .as_ref()
            .map(|lease| lease_status(lease, now).as_str())
            .unwrap_or("-");
        out.push_str(&format!(
            "{:<14} {:<10} {:<18} {:<8} {:<11} {}\n",
            card.id,
            card.status.as_str(),
            card.owner.as_deref().unwrap_or("-"),
            lease,
            aux_state(card, now).unwrap_or("-"),
            card.title
        ));
    }
    out
}

pub fn render_show_text(card: &TaskCard, now: DateTime<Utc>) -> String {
    let mut out = String::new();
    out.push_str(&format!("id: {}\n", card.id));
    out.push_str(&format!("title: {}\n", card.title));
    if !card.goal.is_empty() {
        out.push_str(&format!("goal: {}\n", card.goal));
    }
    if !card.output.is_empty() {
        out.push_str(&format!("output: {}\n", card.output));
    }
    out.push_str(&format!("status: {}\n", card.status.as_str()));
    if let Some(owner) = &card.owner {
        out.push_str(&format!("owner: {}\n", owner));
    }
    if let Some(verifier) = &card.verifier {
        out.push_str(&format!("verifier: {}\n", verifier));
    }
    if let Some(priority) = &card.priority {
        out.push_str(&format!("priority: {}\n", priority));
    }
    if let Some(due) = &card.due {
        out.push_str(&format!("due: {}\n", due));
    }
    if let Some(lease) = &card.lease {
        out.push_str(&format!(
            "lease: holder={} acquired_at={} last_heartbeat={} expires_at={} version={} status={}\n",
            lease.holder,
            lease.acquired_at,
            lease.last_heartbeat,
            lease.expires_at,
            lease.version,
            lease_status(lease, now).as_str()
        ));
    }
    if let Some(key) = &card.idempotency_key {
        out.push_str(&format!("idempotency_key: {}\n", key));
    }
    if let Some(blocker) = &card.blocker {
        out.push_str(&format!(
            "blocker: what={} tried={} who={} deadline_impact={}\n",
            blocker.what, blocker.tried, blocker.who, blocker.deadline_impact
        ));
    }
    if card.orphaned {
        out.push_str("orphaned: true\n");
    }
    if let Some(state) = aux_state(card, now) {
        out.push_str(&format!("state: {}\n", state));
    }
    if card.reassign_count > 0 {
        out.push_str(&format!("reassign_count: {}\n", card.reassign_count));
    }
    if let Some(reason) = &card.orphan_reason {
        out.push_str(&format!("orphan_reason: {}\n", reason));
    }
    if !card.evidence.is_empty() {
        out.push_str(&format!("evidence: {} entries\n", card.evidence.len()));
    }
    if !card.depends_on.is_empty() {
        out.push_str(&format!("depends_on: {}\n", card.depends_on.join(", ")));
    }
    if let Some(lane) = &card.lane {
        out.push_str(&format!("lane: {}\n", lane));
    }
    if !card.anti_jobs.is_empty() {
        out.push_str(&format!("anti_jobs: {}\n", card.anti_jobs.join(", ")));
    }
    if !card.updated_at.is_empty() {
        out.push_str(&format!("updated_at: {}\n", card.updated_at));
    }
    out.push_str(&format!("version: {}\n", card.version));
    out
}

pub fn render_claim_text(outcome: &ClaimOutcome) -> String {
    let card = &outcome.card;
    let holder = card.owner.as_deref().unwrap_or("-");
    let mut out = if outcome.replayed {
        format!(
            "Already applied: {} is still held by {} (idempotency key {}); nothing was written.\n",
            card.id,
            holder,
            outcome.idempotency_key.as_deref().unwrap_or("-")
        )
    } else {
        format!("Recorded claim: {} is now held by {}.\n", card.id, holder)
    };
    if let Some(lease) = &card.lease {
        out.push_str(&format!(
            "lease: version={} expires_at={}\n",
            lease.version, lease.expires_at
        ));
    }
    if let Some(key) = &outcome.idempotency_key {
        out.push_str(&format!("idempotency_key: {}\n", key));
    }
    if let Some(prev) = &outcome.previous_holder {
        if prev != holder {
            out.push_str(&format!(
                "Note: previously held by {}; the overlap is flagged, not blocked.\n",
                prev
            ));
        }
    }
    if outcome.replayed {
        out.push_str("Observe-only: a repeated claim is a no-op; no seat was blocked.\n");
    } else {
        out.push_str("Observe-only: the claim is recorded; no seat was blocked.\n");
    }
    out
}

fn print_json(value: &serde_json::Value, raw: bool) -> Result<()> {
    if raw {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}

/// Run one `zen task` subcommand. `--quiet` silences stdout only: the
/// claim is still recorded, errors still surface, and the exit code
/// never changes (the B9/quiet contract the cost pilot established).
pub fn run_task(
    args: &TaskArgs,
    format: &OutputFormat,
    quiet: bool,
    raw: bool,
    home: Option<&Path>,
) -> Result<i32> {
    let Some(home) = home else {
        anyhow::bail!("cannot determine home directory");
    };
    let path = registry_path(home);
    match &args.command {
        TaskSubcommand::List => {
            let cards = load_registry(&path)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::json!({ "tasks": cards }), raw)?;
                    }
                    OutputFormat::Text => print!("{}", render_list_text(&cards, Utc::now())),
                }
            }
            Ok(0)
        }
        TaskSubcommand::Show { id } => {
            let cards = load_registry(&path)?;
            let Some(card) = find_task(&cards, id) else {
                return Err(crate::coded_error(
                    exit_code::NOT_FOUND,
                    format!("task '{}' not found in the task registry", id),
                ));
            };
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(card)?, raw)?;
                    }
                    OutputFormat::Text => print!("{}", render_show_text(card, Utc::now())),
                }
            }
            Ok(0)
        }
        TaskSubcommand::Claim {
            id,
            by,
            expected_version,
        } => {
            let Some(cas) = claim_task_cas(&path, id, by, *expected_version, Utc::now())? else {
                return Err(crate::coded_error(
                    exit_code::NOT_FOUND,
                    format!("task '{}' not found in the task registry", id),
                ));
            };
            let outcome = match cas {
                CasClaim::Recorded(outcome) | CasClaim::Replayed(outcome) => outcome,
                CasClaim::Conflict { expected, actual } => {
                    return Err(crate::coded_error(
                        exit_code::CONFLICT,
                        format!(
                            "claim for task '{id}' refused: expected lease version {expected}, actual lease version {actual}; re-read the card and retry with the current version"
                        ),
                    ));
                }
                CasClaim::DeadLetter => {
                    return Err(crate::coded_error(
                        exit_code::CONFLICT,
                        dead_letter_message(id),
                    ));
                }
            };
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        let value = serde_json::json!({
                            "task": outcome.card,
                            "claim_recorded": true,
                            "replayed": outcome.replayed,
                            "idempotency_key": outcome.idempotency_key,
                            "previous_holder": outcome.previous_holder,
                            "observe_only": true,
                        });
                        print_json(&value, raw)?;
                    }
                    OutputFormat::Text => print!("{}", render_claim_text(&outcome)),
                }
            }
            Ok(0)
        }
        TaskSubcommand::Heartbeat { id, by } => {
            let Some(outcome) = heartbeat_task(&path, id, by, Utc::now())? else {
                return Err(crate::coded_error(
                    exit_code::NOT_FOUND,
                    format!("task '{}' not found in the task registry", id),
                ));
            };
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        let value = serde_json::json!({
                            "task": outcome.card,
                            "heartbeat_recorded": true,
                            "flagged": outcome.flagged,
                            "observe_only": true,
                        });
                        print_json(&value, raw)?;
                    }
                    OutputFormat::Text => print!("{}", render_heartbeat_text(&outcome)),
                }
            }
            Ok(0)
        }
        TaskSubcommand::Revive { id, by } => {
            let Some(outcome) = revive_task(&path, id, by, Utc::now())? else {
                return Err(crate::coded_error(
                    exit_code::NOT_FOUND,
                    format!("task '{}' not found in the task registry", id),
                ));
            };
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        let value = serde_json::json!({
                            "task": outcome.card,
                            "revived": true,
                            "revived_by": outcome.revived_by,
                        });
                        print_json(&value, raw)?;
                    }
                    OutputFormat::Text => print!("{}", render_revive_text(&outcome)),
                }
            }
            Ok(0)
        }
        TaskSubcommand::Sweep { dry_run } => {
            let outcome = sweep_tasks(&path, Utc::now(), *dry_run)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        let count = outcome.entries.len();
                        let value = serde_json::json!({
                            "changes": outcome.entries,
                            "count": count,
                            "dry_run": outcome.dry_run,
                        });
                        print_json(&value, raw)?;
                    }
                    OutputFormat::Text => print!("{}", render_sweep_text(&outcome)),
                }
            }
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_registry(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zen_tasks_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tasks.jsonl")
    }

    fn card(id: &str, title: &str) -> TaskCard {
        TaskCard {
            id: id.to_string(),
            title: title.to_string(),
            updated_at: "2026-11-07T00:00:00+00:00".to_string(),
            ..Default::default()
        }
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-07T01:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn registry_path_lives_under_dot_zen() {
        let path = registry_path(Path::new("/home/tester"));
        assert_eq!(path, Path::new("/home/tester/.zen/tasks.jsonl"));
    }

    #[test]
    fn missing_file_loads_as_empty_registry() {
        let path = temp_registry("missing").with_file_name("nope.jsonl");
        assert!(load_registry(&path).unwrap().is_empty());
    }

    #[test]
    fn empty_file_loads_as_empty_registry() {
        let path = temp_registry("empty");
        std::fs::write(&path, "").unwrap();
        assert!(load_registry(&path).unwrap().is_empty());
    }

    #[test]
    fn blank_lines_are_skipped() {
        let path = temp_registry("blank");
        append_snapshot(&path, &card("B-1", "first")).unwrap();
        let mut data = std::fs::read_to_string(&path).unwrap();
        data.push_str("\n   \n");
        std::fs::write(&path, data).unwrap();
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, "B-1");
    }

    #[test]
    fn append_then_load_roundtrips_cards() {
        let path = temp_registry("roundtrip");
        append_snapshot(&path, &card("B-1", "first")).unwrap();
        let mut second = card("B-2", "second");
        second.status = TaskStatus::Doing;
        second.owner = Some("zc-developer".to_string());
        append_snapshot(&path, &second).unwrap();
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].id, "B-1");
        assert_eq!(cards[1].status, TaskStatus::Doing);
        assert_eq!(cards[1].owner.as_deref(), Some("zc-developer"));
    }

    #[test]
    fn latest_snapshot_for_an_id_wins() {
        let path = temp_registry("fold");
        append_snapshot(&path, &card("B-1", "old title")).unwrap();
        let mut newer = card("B-1", "new title");
        newer.version = 2;
        append_snapshot(&path, &newer).unwrap();
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "new title");
        assert_eq!(cards[0].version, 2);
    }

    #[test]
    fn malformed_line_is_an_error() {
        let path = temp_registry("malformed");
        std::fs::write(&path, "{\"id\": 42}\n").unwrap();
        assert!(load_registry(&path).is_err());
    }

    #[test]
    fn claim_records_holder_lease_and_bumps_versions() {
        let path = temp_registry("claim");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        let outcome = claim_task(&path, "B-4", "zc-developer", fixed_now())
            .unwrap()
            .expect("task exists");
        assert_eq!(outcome.previous_holder, None);
        assert_eq!(outcome.card.owner.as_deref(), Some("zc-developer"));
        let lease = outcome.card.lease.as_ref().unwrap();
        assert_eq!(lease.holder, "zc-developer");
        assert_eq!(lease.acquired_at, "2026-11-07T01:00:00+00:00");
        assert_eq!(lease.last_heartbeat, "2026-11-07T01:00:00+00:00");
        assert_eq!(lease.expires_at, "2026-11-07T09:00:00+00:00");
        assert_eq!(lease.version, 1);
        assert_eq!(outcome.card.version, 1);
        // The claim is one appended snapshot, not a rewrite.
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 2);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].owner.as_deref(), Some("zc-developer"));
    }

    #[test]
    fn claim_unknown_id_records_nothing() {
        let path = temp_registry("claim_unknown");
        let outcome = claim_task(&path, "B-99", "zc-developer", fixed_now()).unwrap();
        assert!(outcome.is_none());
        assert!(!path.exists());
    }

    #[test]
    fn second_claim_is_recorded_not_blocked() {
        let path = temp_registry("claim_overlap");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        claim_task(&path, "B-4", "zc-developer", fixed_now())
            .unwrap()
            .unwrap();
        let outcome = claim_task(&path, "B-4", "sd-developer", fixed_now())
            .unwrap()
            .expect("observe-only claim still records");
        assert_eq!(outcome.previous_holder.as_deref(), Some("zc-developer"));
        assert_eq!(outcome.card.owner.as_deref(), Some("sd-developer"));
        assert_eq!(outcome.card.lease.as_ref().unwrap().version, 2);
        let text = render_claim_text(&outcome);
        assert!(text.contains("previously held by zc-developer"));
        assert!(text.contains("flagged, not blocked"));
    }

    #[test]
    fn cas_claim_with_matching_version_records_and_keys_it() {
        let path = temp_registry("cas_match");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        let cas = claim_task_cas(&path, "B-4", "zc-developer", Some(0), fixed_now())
            .unwrap()
            .expect("task exists");
        let outcome = match cas {
            CasClaim::Recorded(outcome) => outcome,
            CasClaim::Replayed(_) => panic!("expected Recorded, got Replayed"),
            CasClaim::Conflict { .. } => panic!("expected Recorded, got Conflict"),
            CasClaim::DeadLetter => panic!("expected Recorded, got DeadLetter"),
        };
        assert!(!outcome.replayed);
        assert_eq!(
            outcome.idempotency_key.as_deref(),
            Some("claim:B-4:zc-developer:0")
        );
        assert_eq!(outcome.card.lease.as_ref().unwrap().version, 1);
        assert_eq!(outcome.card.version, 1);
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 2);
        let last = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_string();
        assert!(last.contains(r#""idempotency_key":"claim:B-4:zc-developer:0""#));
    }

    #[test]
    fn cas_claim_with_stale_version_is_refused_without_writing() {
        let path = temp_registry("cas_stale");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        claim_task_cas(&path, "B-4", "zc-developer", Some(0), fixed_now())
            .unwrap()
            .unwrap();
        let cas = claim_task_cas(&path, "B-4", "sd-developer", Some(0), fixed_now())
            .unwrap()
            .expect("task exists");
        match cas {
            CasClaim::Conflict { expected, actual } => {
                assert_eq!(expected, 0);
                assert_eq!(actual, 1);
            }
            CasClaim::Recorded(_) => panic!("expected Conflict, got Recorded"),
            CasClaim::Replayed(_) => panic!("expected Conflict, got Replayed"),
            CasClaim::DeadLetter => panic!("expected Conflict, got DeadLetter"),
        }
        // A refused claim writes no snapshot: still seed + first claim.
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 2);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].owner.as_deref(), Some("zc-developer"));
        assert_eq!(cards[0].lease.as_ref().unwrap().version, 1);
    }

    #[test]
    fn cas_claim_replay_with_same_key_is_a_noop() {
        let path = temp_registry("cas_replay");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        claim_task_cas(&path, "B-4", "zc-developer", Some(0), fixed_now())
            .unwrap()
            .unwrap();
        let cas = claim_task_cas(&path, "B-4", "zc-developer", Some(0), fixed_now())
            .unwrap()
            .expect("task exists");
        let outcome = match cas {
            CasClaim::Replayed(outcome) => outcome,
            CasClaim::Recorded(_) => panic!("expected Replayed, got Recorded"),
            CasClaim::Conflict { .. } => panic!("expected Replayed, got Conflict"),
            CasClaim::DeadLetter => panic!("expected Replayed, got DeadLetter"),
        };
        assert!(outcome.replayed);
        assert_eq!(outcome.card.lease.as_ref().unwrap().version, 1);
        assert_eq!(outcome.card.version, 1);
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 2);
        let text = render_claim_text(&outcome);
        assert!(text.contains("Already applied"));
        assert!(text.contains("nothing was written"));
    }

    #[test]
    fn cas_replay_after_an_intervening_claim_is_a_conflict() {
        let path = temp_registry("cas_intervening");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        claim_task_cas(&path, "B-4", "zc-developer", Some(0), fixed_now())
            .unwrap()
            .unwrap();
        // A legacy (unguarded) claim moves the lease on; the old key
        // is still on the card but no longer describes the lease.
        claim_task(&path, "B-4", "sd-developer", fixed_now())
            .unwrap()
            .unwrap();
        let cas = claim_task_cas(&path, "B-4", "zc-developer", Some(0), fixed_now())
            .unwrap()
            .expect("task exists");
        match cas {
            CasClaim::Conflict { expected, actual } => {
                assert_eq!(expected, 0);
                assert_eq!(actual, 2);
            }
            CasClaim::Recorded(_) => panic!("expected Conflict, got Recorded"),
            CasClaim::Replayed(_) => panic!("expected Conflict, got Replayed"),
            CasClaim::DeadLetter => panic!("expected Conflict, got DeadLetter"),
        }
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 3);
    }

    #[test]
    fn claim_without_expected_version_keeps_legacy_behavior() {
        let path = temp_registry("cas_legacy");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        let cas = claim_task_cas(&path, "B-4", "zc-developer", None, fixed_now())
            .unwrap()
            .expect("task exists");
        let outcome = match cas {
            CasClaim::Recorded(outcome) => outcome,
            CasClaim::Replayed(_) => panic!("expected Recorded, got Replayed"),
            CasClaim::Conflict { .. } => panic!("expected Recorded, got Conflict"),
            CasClaim::DeadLetter => panic!("expected Recorded, got DeadLetter"),
        };
        assert!(!outcome.replayed);
        assert_eq!(outcome.idempotency_key, None);
        assert_eq!(outcome.card.lease.as_ref().unwrap().version, 1);
        let outcome = claim_task(&path, "B-4", "sd-developer", fixed_now())
            .unwrap()
            .expect("observe-only claim still records");
        assert_eq!(outcome.previous_holder.as_deref(), Some("zc-developer"));
        assert_eq!(outcome.card.lease.as_ref().unwrap().version, 2);
        assert_eq!(outcome.idempotency_key, None);
    }

    #[test]
    fn cas_claim_on_a_leased_card_needs_the_current_version() {
        let path = temp_registry("cas_leased");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        claim_task(&path, "B-4", "zc-developer", fixed_now())
            .unwrap()
            .unwrap();
        let cas = claim_task_cas(&path, "B-4", "sd-developer", Some(1), fixed_now())
            .unwrap()
            .expect("task exists");
        let outcome = match cas {
            CasClaim::Recorded(outcome) => outcome,
            CasClaim::Replayed(_) => panic!("expected Recorded, got Replayed"),
            CasClaim::Conflict { .. } => panic!("expected Recorded, got Conflict"),
            CasClaim::DeadLetter => panic!("expected Recorded, got DeadLetter"),
        };
        assert_eq!(
            outcome.idempotency_key.as_deref(),
            Some("claim:B-4:sd-developer:1")
        );
        assert_eq!(outcome.card.lease.as_ref().unwrap().version, 2);
        let text = render_show_text(&outcome.card, fixed_now());
        assert!(text.contains("idempotency_key: claim:B-4:sd-developer:1"));
        assert!(text.contains("version=2"));
    }

    #[test]
    fn cas_claim_unknown_id_records_nothing() {
        let path = temp_registry("cas_unknown");
        let cas = claim_task_cas(&path, "B-99", "zc-developer", Some(0), fixed_now()).unwrap();
        assert!(cas.is_none());
        assert!(!path.exists());
    }

    #[test]
    fn card_json_uses_the_design_schema_fields() {
        let mut full = card("B-4", "fix install");
        full.goal = "runs in Constrained Language".to_string();
        full.output = "PR with patched install.ps1".to_string();
        full.owner = Some("zc-developer".to_string());
        full.verifier = Some("zc-leader".to_string());
        full.status = TaskStatus::Doing;
        full.priority = Some("P1".to_string());
        full.due = Some("2026-11-08".to_string());
        full.depends_on = vec!["B-2".to_string()];
        full.lane = Some("cli".to_string());
        full.anti_jobs = vec!["do not merge".to_string()];
        full.blocker = Some(Blocker {
            what: "no Windows box".to_string(),
            tried: "pwsh on Linux".to_string(),
            who: "zc-leader".to_string(),
            deadline_impact: "none".to_string(),
        });
        full.evidence = vec![Evidence {
            verifier: Some("zc-leader".to_string()),
            command: "cargo test".to_string(),
            result: "1250 passed".to_string(),
            at: "2026-11-07T01:00:00+00:00".to_string(),
        }];
        full.lease = Some(Lease {
            holder: "zc-developer".to_string(),
            acquired_at: "2026-11-07T01:00:00+00:00".to_string(),
            last_heartbeat: "2026-11-07T01:00:00+00:00".to_string(),
            expires_at: "2026-11-07T09:00:00+00:00".to_string(),
            version: 1,
        });
        let value = serde_json::to_value(&full).unwrap();
        for key in [
            "id",
            "title",
            "goal",
            "output",
            "owner",
            "verifier",
            "status",
            "lease",
            "evidence",
            "blocker",
            "priority",
            "due",
            "depends_on",
            "lane",
            "anti_jobs",
            "orphaned",
            "updated_at",
            "version",
        ] {
            assert!(value.get(key).is_some(), "missing field {}", key);
        }
        assert_eq!(value["status"], "doing");
        assert_eq!(value["lease"]["holder"], "zc-developer");
        assert_eq!(value["blocker"]["who"], "zc-leader");
        assert_eq!(value["evidence"][0]["command"], "cargo test");
        // And the same JSON parses back into an equal card.
        let back: TaskCard = serde_json::from_value(value).unwrap();
        assert_eq!(back, full);
    }

    #[test]
    fn sparse_card_json_still_parses() {
        // A hand-written minimal line must load: every field beyond
        // id/title carries a default.
        let parsed: TaskCard = serde_json::from_str("{\"id\":\"B-1\",\"title\":\"x\"}").unwrap();
        assert_eq!(parsed.status, TaskStatus::Queued);
        assert!(parsed.lease.is_none());
        assert!(parsed.evidence.is_empty());
    }

    #[test]
    fn list_text_renders_rows_and_empty_state() {
        assert_eq!(
            render_list_text(&[], fixed_now()),
            "No tasks in the registry.\n"
        );
        let mut doing = card("B-4", "fix install");
        doing.status = TaskStatus::Doing;
        doing.owner = Some("zc-developer".to_string());
        let text = render_list_text(&[doing], fixed_now());
        assert!(text.contains("B-4"));
        assert!(text.contains("doing"));
        assert!(text.contains("zc-developer"));
        assert!(text.contains("fix install"));
    }

    #[test]
    fn show_text_renders_card_fields() {
        let mut doing = card("B-4", "fix install");
        doing.status = TaskStatus::Verifying;
        let text = render_show_text(&doing, fixed_now());
        assert!(text.contains("id: B-4"));
        assert!(text.contains("status: verifying"));
        assert!(text.contains("title: fix install"));
    }

    fn fixed_now_plus_hours(hours: i64) -> DateTime<Utc> {
        fixed_now() + Duration::hours(hours)
    }

    #[test]
    fn heartbeat_renews_lease_and_bumps_versions() {
        let path = temp_registry("heartbeat");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        claim_task(&path, "B-4", "zc-developer", fixed_now())
            .unwrap()
            .unwrap();
        let outcome = heartbeat_task(&path, "B-4", "zc-developer", fixed_now_plus_hours(2))
            .unwrap()
            .expect("task exists");
        assert!(!outcome.flagged);
        let lease = outcome.card.lease.as_ref().unwrap();
        assert_eq!(lease.holder, "zc-developer");
        assert_eq!(lease.acquired_at, "2026-11-07T01:00:00+00:00");
        assert_eq!(lease.last_heartbeat, "2026-11-07T03:00:00+00:00");
        assert_eq!(lease.expires_at, "2026-11-07T11:00:00+00:00");
        assert_eq!(lease.version, 2);
        assert_eq!(outcome.card.version, 2);
        // One seed + one claim + one heartbeat snapshot in the file.
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 3);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].lease.as_ref().unwrap().version, 2);
    }

    #[test]
    fn heartbeat_from_another_seat_is_flagged_not_blocked() {
        let path = temp_registry("heartbeat_overlap");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        claim_task(&path, "B-4", "zc-developer", fixed_now())
            .unwrap()
            .unwrap();
        let outcome = heartbeat_task(&path, "B-4", "sd-developer", fixed_now_plus_hours(1))
            .unwrap()
            .expect("observe-only heartbeat still records");
        assert!(outcome.flagged);
        // The lease is renewed, but a heartbeat never transfers it.
        let lease = outcome.card.lease.as_ref().unwrap();
        assert_eq!(lease.holder, "zc-developer");
        assert_eq!(lease.version, 2);
        assert_eq!(lease.last_heartbeat, "2026-11-07T02:00:00+00:00");
        assert_eq!(lease.expires_at, "2026-11-07T10:00:00+00:00");
        let text = render_heartbeat_text(&outcome);
        assert!(text.contains("flagged, not blocked"));
    }

    #[test]
    fn heartbeat_unknown_id_records_nothing() {
        let path = temp_registry("heartbeat_unknown");
        let outcome = heartbeat_task(&path, "B-99", "zc-developer", fixed_now()).unwrap();
        assert!(outcome.is_none());
        assert!(!path.exists());
    }

    #[test]
    fn heartbeat_without_a_lease_starts_one() {
        let path = temp_registry("heartbeat_fresh");
        append_snapshot(&path, &card("B-4", "fix install")).unwrap();
        let outcome = heartbeat_task(&path, "B-4", "zc-developer", fixed_now())
            .unwrap()
            .expect("task exists");
        assert!(!outcome.flagged);
        let lease = outcome.card.lease.as_ref().unwrap();
        assert_eq!(lease.holder, "zc-developer");
        assert_eq!(lease.version, 1);
        assert_eq!(lease.expires_at, "2026-11-07T09:00:00+00:00");
        assert_eq!(outcome.card.owner.as_deref(), Some("zc-developer"));
    }

    #[test]
    fn lease_status_flips_at_expiry() {
        let mut lease = Lease {
            holder: "zc-developer".to_string(),
            acquired_at: "2026-11-07T01:00:00+00:00".to_string(),
            last_heartbeat: "2026-11-07T01:00:00+00:00".to_string(),
            expires_at: "2026-11-07T09:00:00+00:00".to_string(),
            version: 1,
        };
        assert_eq!(lease_status(&lease, fixed_now()), LeaseStatus::Active);
        assert_eq!(
            lease_status(&lease, fixed_now_plus_hours(8)),
            LeaseStatus::Expired
        );
        lease.expires_at = "not-a-timestamp".to_string();
        assert_eq!(lease_status(&lease, fixed_now()), LeaseStatus::Expired);
    }

    #[test]
    fn list_and_show_display_expired_lease() {
        let mut stale = card("B-4", "fix install");
        stale.lease = Some(Lease {
            holder: "zc-developer".to_string(),
            acquired_at: "2026-11-06T01:00:00+00:00".to_string(),
            last_heartbeat: "2026-11-06T01:00:00+00:00".to_string(),
            expires_at: "2026-11-06T09:00:00+00:00".to_string(),
            version: 3,
        });
        let list = render_list_text(std::slice::from_ref(&stale), fixed_now());
        assert!(list.contains("expired"), "list text: {list}");
        let shown = render_show_text(&stale, fixed_now());
        assert!(shown.contains("status=expired"), "show text: {shown}");
        let earlier = render_show_text(&stale, fixed_now() - Duration::days(2));
        assert!(earlier.contains("status=active"), "show text: {earlier}");
    }

    #[test]
    fn step1_registry_lines_still_load() {
        // Lines exactly as step 1 wrote them: a bare hand-written card,
        // a full claim snapshot, and a card whose lease is missing the
        // fields a later reader might expect (serde defaults fill in).
        let path = temp_registry("step1_compat");
        let data = concat!(
            "{\"id\":\"B-1\",\"title\":\"seed\"}\n",
            "{\"id\":\"B-4\",\"title\":\"fix install\",\"goal\":\"\",\"output\":\"\",\"owner\":\"zc-developer\",\"verifier\":null,\"status\":\"doing\",\"lease\":{\"holder\":\"zc-developer\",\"acquired_at\":\"2026-11-07T01:00:00+00:00\",\"last_heartbeat\":\"2026-11-07T01:00:00+00:00\",\"expires_at\":\"2026-11-07T09:00:00+00:00\",\"version\":1},\"evidence\":[],\"blocker\":null,\"priority\":null,\"due\":null,\"depends_on\":[],\"lane\":null,\"anti_jobs\":[],\"orphaned\":false,\"updated_at\":\"2026-11-07T01:00:00+00:00\",\"version\":1}\n",
            "{\"id\":\"B-7\",\"title\":\"partial lease\",\"lease\":{\"holder\":\"sd-developer\"}}\n"
        );
        std::fs::write(&path, data).unwrap();
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards.len(), 3);
        let claimed = find_task(&cards, "B-4").unwrap();
        assert_eq!(claimed.lease.as_ref().unwrap().version, 1);
        assert_eq!(
            lease_status(claimed.lease.as_ref().unwrap(), fixed_now()),
            LeaseStatus::Active
        );
        let partial = find_task(&cards, "B-7").unwrap();
        let lease = partial.lease.as_ref().unwrap();
        assert_eq!(lease.holder, "sd-developer");
        assert_eq!(lease.version, 0);
        assert_eq!(lease.expires_at, "");
    }

    fn doing_with_lease(id: &str, expires_at: &str) -> TaskCard {
        let mut c = card(id, "work");
        c.status = TaskStatus::Doing;
        c.owner = Some("zc-developer".to_string());
        c.lease = Some(Lease {
            holder: "zc-developer".to_string(),
            acquired_at: "2026-11-06T01:00:00+00:00".to_string(),
            last_heartbeat: "2026-11-06T01:00:00+00:00".to_string(),
            expires_at: expires_at.to_string(),
            version: 1,
        });
        c
    }

    #[test]
    fn sweep_orphans_expired_doing_task() {
        let path = temp_registry("sweep_orphan");
        append_snapshot(&path, &doing_with_lease("B-4", "2026-11-06T09:00:00+00:00")).unwrap();
        let outcome = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert_eq!(outcome.entries.len(), 1);
        assert_eq!(outcome.entries[0].id, "B-4");
        assert_eq!(outcome.entries[0].from, "doing");
        assert_eq!(outcome.entries[0].to, "orphan");
        assert!(outcome.entries[0].reason.contains("lease expired"));
        let cards = load_registry(&path).unwrap();
        let swept = &cards[0];
        assert_eq!(swept.status, TaskStatus::Queued);
        assert!(swept.orphaned);
        assert_eq!(swept.reassign_count, 1);
        // The old holder stays on the card as history.
        assert_eq!(swept.owner.as_deref(), Some("zc-developer"));
        assert!(swept
            .orphan_reason
            .as_ref()
            .unwrap()
            .contains("2026-11-06T09:00:00+00:00"));
        assert!(!swept.dead_lettered);
        // Seed + one sweep snapshot.
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 2);
    }

    #[test]
    fn sweep_leaves_live_doing_task_alone() {
        let path = temp_registry("sweep_live");
        append_snapshot(&path, &doing_with_lease("B-4", "2026-11-07T09:00:00+00:00")).unwrap();
        let outcome = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert!(outcome.entries.is_empty());
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 1);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].status, TaskStatus::Doing);
        assert!(!cards[0].orphaned);
    }

    #[test]
    fn sweep_counts_unclaimed_orphan_until_dead_letter() {
        let path = temp_registry("sweep_dead_letter_cycle");
        append_snapshot(&path, &doing_with_lease("B-4", "2026-11-06T09:00:00+00:00")).unwrap();
        // Pass 1: doing -> orphan, count 1.
        let first = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert_eq!(first.entries[0].to, "orphan");
        // Passes 2 and 3: still unclaimed, the count grows.
        sweep_tasks(&path, fixed_now(), false).unwrap();
        let third = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert_eq!(third.entries[0].to, "orphan");
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].reassign_count, 3);
        assert!(!cards[0].dead_lettered);
        // Pass 4: at the threshold, the task is parked as dead-letter.
        let fourth = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert_eq!(fourth.entries[0].from, "orphan");
        assert_eq!(fourth.entries[0].to, "dead-letter");
        let cards = load_registry(&path).unwrap();
        assert!(cards[0].dead_lettered);
        // A dead-lettered task is never swept again.
        let fifth = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert!(fifth.entries.is_empty());
    }

    #[test]
    fn sweep_dead_letters_orphan_already_at_threshold() {
        let path = temp_registry("sweep_dead_letter_ready");
        let mut parked = doing_with_lease("B-9", "2026-11-06T09:00:00+00:00");
        parked.status = TaskStatus::Queued;
        parked.orphaned = true;
        parked.reassign_count = 3;
        append_snapshot(&path, &parked).unwrap();
        let outcome = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert_eq!(outcome.entries.len(), 1);
        assert_eq!(outcome.entries[0].to, "dead-letter");
        let cards = load_registry(&path).unwrap();
        assert!(cards[0].dead_lettered);
        assert_eq!(cards[0].status, TaskStatus::Queued);
    }

    #[test]
    fn sweep_dry_run_writes_nothing() {
        let path = temp_registry("sweep_dry_run");
        append_snapshot(&path, &doing_with_lease("B-4", "2026-11-06T09:00:00+00:00")).unwrap();
        let before = std::fs::read(&path).unwrap();
        let outcome = sweep_tasks(&path, fixed_now(), true).unwrap();
        assert!(outcome.dry_run);
        assert_eq!(outcome.entries.len(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let text = render_sweep_text(&outcome);
        assert!(text.contains("dry-run"), "sweep text: {text}");
        assert!(text.contains("B-4: doing -> orphan"), "sweep text: {text}");
    }

    #[test]
    fn sweep_on_empty_registry_reports_zero() {
        let path = temp_registry("sweep_empty").with_file_name("nope.jsonl");
        let outcome = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert!(outcome.entries.is_empty());
        assert!(!path.exists());
        let text = render_sweep_text(&outcome);
        assert!(text.contains("0 task(s)"), "sweep text: {text}");
    }

    #[test]
    fn orphan_can_be_claimed_and_heartbeated_again() {
        let path = temp_registry("sweep_reclaim");
        append_snapshot(&path, &doing_with_lease("B-4", "2026-11-06T09:00:00+00:00")).unwrap();
        sweep_tasks(&path, fixed_now(), false).unwrap();
        let claim = claim_task(&path, "B-4", "sd-developer", fixed_now())
            .unwrap()
            .expect("an orphan can be claimed");
        assert_eq!(claim.previous_holder.as_deref(), Some("zc-developer"));
        let hb = heartbeat_task(&path, "B-4", "sd-developer", fixed_now())
            .unwrap()
            .expect("heartbeat works after the claim");
        assert!(!hb.flagged);
        let cards = load_registry(&path).unwrap();
        // The count is history: a new claim does not reset it.
        assert_eq!(cards[0].reassign_count, 1);
        // With a live lease again the card no longer reads as orphan.
        assert_eq!(aux_state(&cards[0], fixed_now()), None);
        // And the sweep leaves it alone now.
        let outcome = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert!(outcome.entries.is_empty());
    }

    fn dead_lettered_card() -> TaskCard {
        let mut c = doing_with_lease("B-4", "2026-11-06T09:00:00+00:00");
        c.status = TaskStatus::Queued;
        c.orphaned = true;
        c.reassign_count = 3;
        c.dead_lettered = true;
        c
    }

    fn registry_lines(path: &Path) -> usize {
        std::fs::read_to_string(path).unwrap().lines().count()
    }

    fn assert_conflict_7(err: &anyhow::Error) {
        assert_eq!(crate::exit_code_for_error(err), crate::exit_code::CONFLICT);
    }

    #[test]
    fn claim_on_dead_lettered_card_is_refused_and_writes_nothing() {
        let path = temp_registry("dead_claim");
        let seeded = dead_lettered_card();
        append_snapshot(&path, &seeded).unwrap();
        let err = claim_task(&path, "B-4", "sd-developer", fixed_now()).unwrap_err();
        assert_conflict_7(&err);
        assert!(format!("{err}").contains("dead-lettered"));
        assert_eq!(registry_lines(&path), 1);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0], seeded);
    }

    #[test]
    fn cas_claim_on_dead_lettered_card_is_refused_even_with_matching_version() {
        let path = temp_registry("dead_cas");
        append_snapshot(&path, &dead_lettered_card()).unwrap();
        // The lease version on the card is 1, so this version matches -
        // the refusal comes from the dead-letter state, not the CAS.
        let cas = claim_task_cas(&path, "B-4", "sd-developer", Some(1), fixed_now())
            .unwrap()
            .expect("task exists");
        assert!(matches!(cas, CasClaim::DeadLetter));
        assert_eq!(registry_lines(&path), 1);
    }

    #[test]
    fn cas_replay_on_dead_lettered_card_is_refused_not_replayed() {
        let path = temp_registry("dead_replay");
        let mut parked = dead_lettered_card();
        parked.idempotency_key = Some(claim_idempotency_key("B-4", "sd-developer", 1));
        parked.lease.as_mut().unwrap().version = 2;
        parked.lease.as_mut().unwrap().holder = "sd-developer".to_string();
        append_snapshot(&path, &parked).unwrap();
        // Without the dead-letter guard this would be a replay no-op:
        // the key matches and the lease is exactly the post-claim state.
        let cas = claim_task_cas(&path, "B-4", "sd-developer", Some(1), fixed_now())
            .unwrap()
            .expect("task exists");
        assert!(matches!(cas, CasClaim::DeadLetter));
        assert_eq!(registry_lines(&path), 1);
    }

    #[test]
    fn heartbeat_on_dead_lettered_card_is_refused_and_keeps_lease() {
        let path = temp_registry("dead_heartbeat");
        let seeded = dead_lettered_card();
        append_snapshot(&path, &seeded).unwrap();
        let err = heartbeat_task(&path, "B-4", "sd-developer", fixed_now()).unwrap_err();
        assert_conflict_7(&err);
        assert!(format!("{err}").contains("dead-lettered"));
        assert_eq!(registry_lines(&path), 1);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0], seeded);
    }

    #[test]
    fn sweep_to_dead_letter_then_claim_and_heartbeat_both_fail() {
        let path = temp_registry("dead_cycle");
        append_snapshot(&path, &doing_with_lease("B-4", "2026-11-06T09:00:00+00:00")).unwrap();
        for _ in 0..4 {
            sweep_tasks(&path, fixed_now(), false).unwrap();
        }
        let cards = load_registry(&path).unwrap();
        assert!(cards[0].dead_lettered);
        let lines_before = registry_lines(&path);
        assert!(claim_task(&path, "B-4", "sd-developer", fixed_now()).is_err());
        assert!(heartbeat_task(&path, "B-4", "sd-developer", fixed_now()).is_err());
        // A fifth sweep leaves the parked card alone, too.
        assert!(sweep_tasks(&path, fixed_now(), false)
            .unwrap()
            .entries
            .is_empty());
        assert_eq!(registry_lines(&path), lines_before);
    }

    #[test]
    fn step1_lines_load_with_sweep_field_defaults() {
        // A step 1/2 snapshot has none of the step-3 fields; serde
        // defaults fill them in and the sweep can act on the card.
        let path = temp_registry("step3_compat");
        let data = "{\"id\":\"B-4\",\"title\":\"fix install\",\"status\":\"doing\",\"owner\":\"zc-developer\",\"lease\":{\"holder\":\"zc-developer\",\"acquired_at\":\"2026-11-06T01:00:00+00:00\",\"last_heartbeat\":\"2026-11-06T01:00:00+00:00\",\"expires_at\":\"2026-11-06T09:00:00+00:00\",\"version\":1}}\n";
        std::fs::write(&path, data).unwrap();
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].reassign_count, 0);
        assert!(!cards[0].dead_lettered);
        assert_eq!(cards[0].orphan_reason, None);
        let outcome = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert_eq!(outcome.entries.len(), 1);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].reassign_count, 1);
        assert!(cards[0].orphaned);
    }

    #[test]
    fn list_and_show_display_orphan_and_dead_letter() {
        let mut orphan = doing_with_lease("B-4", "2026-11-06T09:00:00+00:00");
        orphan.status = TaskStatus::Queued;
        orphan.orphaned = true;
        orphan.reassign_count = 1;
        orphan.orphan_reason = Some(
            "lease expired at 2026-11-06T09:00:00+00:00 (holder zc-developer, last heartbeat 2026-11-06T01:00:00+00:00)"
                .to_string(),
        );
        let list = render_list_text(std::slice::from_ref(&orphan), fixed_now());
        assert!(list.contains("orphan"), "list text: {list}");
        let shown = render_show_text(&orphan, fixed_now());
        assert!(shown.contains("state: orphan"), "show text: {shown}");
        assert!(shown.contains("reassign_count: 1"), "show text: {shown}");
        let mut dead = orphan.clone();
        dead.dead_lettered = true;
        dead.reassign_count = 3;
        let list = render_list_text(std::slice::from_ref(&dead), fixed_now());
        assert!(list.contains("dead-letter"), "list text: {list}");
        let shown = render_show_text(&dead, fixed_now());
        assert!(shown.contains("state: dead-letter"), "show text: {shown}");
    }

    #[test]
    fn revive_dead_lettered_card_returns_it_to_the_queue() {
        let path = temp_registry("revive_ok");
        let seeded = dead_lettered_card();
        append_snapshot(&path, &seeded).unwrap();
        let outcome = revive_task(&path, "B-4", "zc-leader", fixed_now())
            .unwrap()
            .expect("dead-lettered task exists");
        assert_eq!(outcome.revived_by, "zc-leader");
        assert_eq!(outcome.card.status, TaskStatus::Queued);
        assert!(!outcome.card.dead_lettered);
        assert!(!outcome.card.orphaned);
        // History is kept: the reassign count and the lease survive.
        assert_eq!(outcome.card.reassign_count, 3);
        assert_eq!(outcome.card.lease, seeded.lease);
        assert_eq!(registry_lines(&path), 2);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].status, TaskStatus::Queued);
        assert_eq!(aux_state(&cards[0], fixed_now()), None);
    }

    #[test]
    fn revive_queued_card_is_refused_and_writes_nothing() {
        let path = temp_registry("revive_queued");
        let seeded = card("B-4", "work");
        append_snapshot(&path, &seeded).unwrap();
        let err = revive_task(&path, "B-4", "zc-leader", fixed_now()).unwrap_err();
        assert_conflict_7(&err);
        assert!(format!("{err}").contains("not dead-lettered"));
        assert_eq!(registry_lines(&path), 1);
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0], seeded);
    }

    #[test]
    fn revive_orphan_card_is_refused_and_writes_nothing() {
        let path = temp_registry("revive_orphan");
        append_snapshot(&path, &doing_with_lease("B-4", "2026-11-06T09:00:00+00:00")).unwrap();
        sweep_tasks(&path, fixed_now(), false).unwrap();
        let lines_before = registry_lines(&path);
        let err = revive_task(&path, "B-4", "zc-leader", fixed_now()).unwrap_err();
        assert_conflict_7(&err);
        assert_eq!(registry_lines(&path), lines_before);
        let cards = load_registry(&path).unwrap();
        assert!(cards[0].orphaned);
        assert!(!cards[0].dead_lettered);
    }

    #[test]
    fn revive_unknown_id_records_nothing() {
        let path = temp_registry("revive_unknown");
        append_snapshot(&path, &card("B-4", "work")).unwrap();
        assert!(revive_task(&path, "B-9", "zc-leader", fixed_now())
            .unwrap()
            .is_none());
        assert_eq!(registry_lines(&path), 1);
    }

    #[test]
    fn revived_card_can_be_claimed_again_and_sweep_leaves_it_alone() {
        let path = temp_registry("revive_cycle");
        append_snapshot(&path, &doing_with_lease("B-4", "2026-11-06T09:00:00+00:00")).unwrap();
        for _ in 0..4 {
            sweep_tasks(&path, fixed_now(), false).unwrap();
        }
        assert!(load_registry(&path).unwrap()[0].dead_lettered);
        revive_task(&path, "B-4", "zc-leader", fixed_now())
            .unwrap()
            .expect("dead-lettered task exists");
        let claim = claim_task(&path, "B-4", "sd-developer", fixed_now())
            .unwrap()
            .expect("a revived card can be claimed");
        assert_eq!(claim.card.lease.as_ref().unwrap().holder, "sd-developer");
        // With a live lease again the sweep leaves it alone, and the
        // reassign count stays history (the revive did not reset it).
        let outcome = sweep_tasks(&path, fixed_now(), false).unwrap();
        assert!(outcome.entries.is_empty());
        let cards = load_registry(&path).unwrap();
        assert_eq!(cards[0].reassign_count, 3);
        assert!(!cards[0].dead_lettered);
    }
}
