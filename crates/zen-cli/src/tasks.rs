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

/// One of the seven statuses of the v3.0 state machine. BLOCKED and
/// ORPHAN are auxiliary states: a card carries them through its
/// `blocker` field and its `orphaned` flag, not through this enum.
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
pub struct ClaimOutcome {
    pub card: TaskCard,
    pub previous_holder: Option<String>,
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
    Ok(Some(ClaimOutcome {
        card,
        previous_holder,
    }))
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

pub fn render_list_text(cards: &[TaskCard], now: DateTime<Utc>) -> String {
    if cards.is_empty() {
        return "No tasks in the registry.\n".to_string();
    }
    let mut out = format!(
        "{:<14} {:<10} {:<18} {:<8} {}\n",
        "ID", "STATUS", "OWNER", "LEASE", "TITLE"
    );
    for card in cards {
        let lease = card
            .lease
            .as_ref()
            .map(|lease| lease_status(lease, now).as_str())
            .unwrap_or("-");
        out.push_str(&format!(
            "{:<14} {:<10} {:<18} {:<8} {}\n",
            card.id,
            card.status.as_str(),
            card.owner.as_deref().unwrap_or("-"),
            lease,
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
    if let Some(blocker) = &card.blocker {
        out.push_str(&format!(
            "blocker: what={} tried={} who={} deadline_impact={}\n",
            blocker.what, blocker.tried, blocker.who, blocker.deadline_impact
        ));
    }
    if card.orphaned {
        out.push_str("orphaned: true\n");
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
    let mut out = format!("Recorded claim: {} is now held by {}.\n", card.id, holder);
    if let Some(lease) = &card.lease {
        out.push_str(&format!(
            "lease: version={} expires_at={}\n",
            lease.version, lease.expires_at
        ));
    }
    if let Some(prev) = &outcome.previous_holder {
        if prev != holder {
            out.push_str(&format!(
                "Note: previously held by {}; the overlap is flagged, not blocked.\n",
                prev
            ));
        }
    }
    out.push_str("Observe-only: the claim is recorded; no seat was blocked.\n");
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
        TaskSubcommand::Claim { id, by } => {
            let Some(outcome) = claim_task(&path, id, by, Utc::now())? else {
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
                            "claim_recorded": true,
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
}
