//! DS-7: the secret request mechanism.
//!
//! A seat that needs a secret to do its work files a request by name,
//! an approver records a decision, and an issuer records the handover.
//! The value itself never appears here: no function in this module takes
//! a secret value, and no file it writes can hold one. Names travel
//! freely; values travel once, from the Secure Vault to the seat's
//! runtime, through a human issuer (design section 4.5).
//!
//! Design: `ds-7-secret-request-draft-2026-11-08.md`, approved by the
//! Leader in tin 8071 with eight decisions (D1-D8), which this module
//! implements as written:
//!
//! - D1 approver split: the Leader (`zc-leader`) approves seat secrets;
//!   Zen (`zen`) approves user/Zen broker grants and paid-use secrets.
//! - D2 lifetimes: 7 days for a task-scoped seat secret, 24 hours for a
//!   one-shot issuance and for a user/Zen broker grant, 90-day rotation
//!   for seat secrets, 30 days for paid-use secrets, 24-hour rotation
//!   grace. A request may ask for less, never for more than the
//!   registry default for that secret - and a one-shot issuance or a
//!   broker grant never lives past 24 hours even when the registry
//!   default is longer; `request_secret` and `rotate_secret` enforce
//!   that cap (review 5817478591, F1).
//! - D3 Vault read path: the Leader or Z Ops reads the Vault and places
//!   the value in the seat's runtime environment on its box. `issue`
//!   records that act (issuer, time, Vault entry); it never carries
//!   the value.
//! - D4 Vault entry shape: one entry per (secret, seat) pair, so one
//!   seat's access can be revoked without cutting off another seat.
//!   The entry id recorded on issuance is `<NAME>:<seat>`.
//! - D5 command and field names are kept as designed: request ids look
//!   like `sr-YYYYMMDD-NNN`, the states are `requested`, `approved`,
//!   `denied`, `issued`, `active`, `expired`, `revoked`, `rotated`,
//!   plus `withdrawn` (follow-up F1, Leader tin 8174), and the CLI
//!   surface is `zen secret` with the subcommands `request`, `list`,
//!   `approve`, `deny`, `issue`, `revoke`, `rotate`, `status`, plus
//!   `register` and `deregister` (Leader only, review 5817478591, Q1)
//!   and `withdraw` (follow-up F1).
//! - D6 expiry needs a restart/reload: the check alone refuses after
//!   the `until`, and the runtime must drop the value at its next
//!   start. This module records the expiry and refuses the check; it
//!   cannot reach into a running process to clear its environment,
//!   and it says so instead of pretending otherwise.
//! - D7 the DS-Bot source was read before implementation (commit
//!   `6559c9e7c4ef71c292e6ac50f690031133536fe0`, release 0.3.1):
//!   `ds-bot/src/host/secrets.js` (586 lines). What the source showed,
//!   against design section 2:
//!   - The dossier's "no request mechanism" claim was overstated. The
//!     source does have a request flow: the `request_secret` tool
//!     files a pending card request (uuid id, name, purpose, kind
//!     `fill`/`allow`, status `pending`), and the user answers it
//!     with `secret-set` (save a value), `secret-allow` (extend
//!     scope), or `secret-cancel`. What it does not have is an
//!     approval step by anyone but the user, an expiry, a rotation,
//!     a revocation state, a registry, or an audit trail - so the
//!     DS-7 lifecycle in section 4 is still designed, not copied,
//!     and no upstream code was reused.
//!   - Redaction is stronger than the dossier's verbatim-only claim:
//!     the source redactor also covers the JSON-escaped and
//!     URL-encoded forms of a value, longest value first, in one
//!     pass, with short values (under 8 characters) replaced only
//!     where they stand alone. The redactor here follows that shape
//!     (written from scratch for ZenCode); the encoded/split/reversed
//!     leak the dossier names is still not covered, and the tests
//!     say so.
//!   - Secrets live in `secrets.json` (mode 600) with the value in
//!     plaintext JSON, scoped `all` or to a list of bots, and reach
//!     a bot's shell as `$DSH_SECRET_<NAME>`.
//! - D8 the first real issuance on one secret, with the Leader
//!   watching, is future work outside this module: it is the proof
//!   of the Vault handover, not a code step.
//!
//! Follow-ups after PR #76 merged (Leader tin 8174):
//!
//! - F1 withdraw: `withdraw_secret` lets the requesting seat take back
//!   its own request while it is still `requested`; the request moves
//!   to the terminal state `withdrawn` and is never decided
//!   afterwards. The approver's `deny` is unchanged: deny is the
//!   approver's act, withdraw is the requester's own act.
//! - F2 Zen revokes a user-class broker grant: `revoke_secret` accepts
//!   Zen (`zen`) as the revoker for a user-class issuance, alongside
//!   the issuer and the Leader. Zen owns the credential the grant
//!   touches, so he can cut the grant off directly. A seat-class
//!   issuance is unchanged: Zen cannot revoke it.
//!
//! Files, all under `<home>/.zen/` (the same tree that holds the task
//! registry and the session transcripts):
//!
//! - `secrets-registry.jsonl`: one line per registration. Names
//!   secrets; holds no values.
//! - `secret-requests.jsonl`: one line per state change of a request.
//!   Reading folds the lines in order and the latest snapshot for a
//!   request id wins, so the file doubles as the request history.
//! - `secret-audit.jsonl`: one append-only line per lifecycle step,
//!   by name only.
//!
//! The DS-5 capability check that gates issuance (design section 4.5)
//! has no code implementation yet: the DS-5 grant table does not exist
//! on disk. Until it does, the approval recorded here is the ALLOW
//! evidence - `issue` requires the request to be `approved`, and the
//! approver recorded on it must satisfy the D1 authority rule. When
//! the DS-5 grant store lands, `check_issue` is the seam where its
//! `check(seat, secret-request, scope, secret:NAME)` plugs in.
//!
//! This module is UI-agnostic: no terminal formatting, results as
//! data. The CLI (`zen-cli`) is a thin wrapper over it, and Desktop
//! can use the same crate through its existing Rust backend.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// File names under `<home>/.zen/`.
pub const REGISTRY_FILE: &str = "secrets-registry.jsonl";
pub const REQUESTS_FILE: &str = "secret-requests.jsonl";
pub const AUDIT_FILE: &str = "secret-audit.jsonl";

/// D1: the Leader approves seat secrets.
pub const LEADER_APPROVER: &str = "zc-leader";
/// D1: Zen approves user/Zen broker grants and paid-use secrets.
pub const ZEN_APPROVER: &str = "zen";
/// D3: the operator who may place a value alongside the Leader.
pub const OPERATOR_ISSUER: &str = "z-ops";

/// D2: default lifetime of a task-scoped seat secret, in days.
pub const DEFAULT_TASK_LIFETIME_DAYS: i64 = 7;
/// D2: lifetime of a one-shot issuance, in hours.
pub const ONE_SHOT_LIFETIME_HOURS: i64 = 24;
/// D2: lifetime of a user/Zen broker grant, in hours.
pub const BROKER_GRANT_LIFETIME_HOURS: i64 = 24;
/// D2: rotation interval for seat secrets, in days.
pub const ROTATION_DAYS_SEAT: i64 = 90;
/// D2: rotation interval for secrets that unlock paid use, in days.
pub const ROTATION_DAYS_PAID: i64 = 30;
/// D2: rotation grace period, in hours. During grace both values work;
/// after it, only the new one does.
pub const ROTATION_GRACE_HOURS: i64 = 24;

/// A secret name is upper snake case, starting with a letter, at most
/// 64 characters (the same shape as DS-Bot's `SECRET_NAME`).
pub const MAX_SECRET_NAME_LEN: usize = 64;

/// Exit codes, following the zen-pstack mapping (docs/exit-codes.md):
/// 0 ok, 1 general, 2 auth, 3 not found, 4 validation, 5 forbidden,
/// 6 rate limit, 7 conflict.
pub mod exit_code {
    pub const OK: i32 = 0;
    pub const GENERAL: i32 = 1;
    pub const NOT_FOUND: i32 = 3;
    pub const VALIDATION: i32 = 4;
    pub const FORBIDDEN: i32 = 5;
    pub const CONFLICT: i32 = 7;
}

/// An error that carries its own process exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretError {
    code: i32,
    message: String,
}

impl SecretError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(exit_code::VALIDATION, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(exit_code::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(exit_code::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(exit_code::CONFLICT, message)
    }

    pub fn code(&self) -> i32 {
        self.code
    }
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SecretError {}

pub type SecretResult<T> = Result<T, SecretError>;

/// The two secret classes (design section 4.2). The class is decided
/// by ownership, not by sensitivity, and is set at registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretClass {
    /// Owned by the team, for a seat's work. May be issued as a value
    /// to the seat's runtime.
    Seat,
    /// Owned by Zen, personally. Never issued as a value to a seat;
    /// work that needs it goes through a broker.
    User,
}

impl SecretClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecretClass::Seat => "seat",
            SecretClass::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "seat" => Some(SecretClass::Seat),
            "user" => Some(SecretClass::User),
            _ => None,
        }
    }
}

/// The request states (design section 4.4, plus `withdrawn` from
/// follow-up F1, Leader tin 8174). State moves forward only, except
/// `denied`, `withdrawn`, and `revoked`, which are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestState {
    Requested,
    Approved,
    Denied,
    Issued,
    Active,
    Expired,
    Revoked,
    Rotated,
    Withdrawn,
}

impl RequestState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RequestState::Requested => "requested",
            RequestState::Approved => "approved",
            RequestState::Denied => "denied",
            RequestState::Issued => "issued",
            RequestState::Active => "active",
            RequestState::Expired => "expired",
            RequestState::Revoked => "revoked",
            RequestState::Rotated => "rotated",
            RequestState::Withdrawn => "withdrawn",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "requested" => RequestState::Requested,
            "approved" => RequestState::Approved,
            "denied" => RequestState::Denied,
            "issued" => RequestState::Issued,
            "active" => RequestState::Active,
            "expired" => RequestState::Expired,
            "revoked" => RequestState::Revoked,
            "rotated" => RequestState::Rotated,
            "withdrawn" => RequestState::Withdrawn,
            _ => return None,
        })
    }
}

/// One line of the secret registry (design section 4.3). The registry
/// names secrets; it holds no values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub v: u32,
    pub name: String,
    pub class: SecretClass,
    pub service: String,
    pub owner: String,
    pub registered_by: String,
    pub registered_at: String,
    pub default_until_days: i64,
    pub rotation_days: i64,
    /// True when the secret unlocks paid use: only Zen approves it,
    /// and it rotates on the paid-use interval (D1, D2).
    #[serde(default)]
    pub paid: bool,
    pub reason: String,
    /// A deregistered name refuses all new requests, and live
    /// issuances under it are revoked.
    #[serde(default)]
    pub deregistered: bool,
}

/// One snapshot of a secret request (design section 4.4). The record
/// holds names only - never a secret value, never a hash of one, never
/// the first or last characters of one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRequest {
    pub v: u32,
    pub request_id: String,
    pub name: String,
    pub class: SecretClass,
    pub seat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub purpose: String,
    pub scope: String,
    /// RFC 3339 timestamp. The lifetime of the issuance, not of the
    /// approval: the issuance ends at this time.
    pub until: String,
    pub state: RequestState,
    pub requested_at: String,
    pub requested_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// The reason the request ended: the denial, withdrawal,
    /// revocation, or one-shot consumption reason. The audit line
    /// carries the same reason; this field keeps the record itself
    /// explainable without reading the audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_reason: Option<String>,
    /// A one-shot issuance is consumed by its first real use and then
    /// revoked. A probe (the check) never consumes it.
    #[serde(default)]
    pub one_shot: bool,
    /// For a user-class secret: the broker action the work needs
    /// ("call endpoint X with Zen's key and return only the result").
    /// A user-class request without one is refused at the writer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<String>,
    /// D4: one Vault entry per (secret, seat) pair, recorded as
    /// `<NAME>:<seat>`. Absent for a user-class broker grant, where no
    /// value is ever issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_entry: Option<String>,
    /// The request this one rotates, if this request was filed by a
    /// rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotates: Option<String>,
    /// D2: during the rotation grace period both values work. After
    /// this time the old issuance is dead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<String>,
}

/// One append-only audit line (design section 4.6). Names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub v: u32,
    pub at: String,
    pub request_id: String,
    pub name: String,
    pub seat: String,
    pub state: RequestState,
    pub actor: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// The outcome of the check that gates a use of an issued secret.
/// The check re-reads the files every time: no cache, no delay, so a
/// revocation is immediate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub allowed: bool,
    /// The reason, always named: the expiry, the revocation, the
    /// deregistration, the wrong seat, or the state that is not live.
    pub reason: String,
}

/// The outcome of recording a real use of an issued secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseOutcome {
    /// True when this use consumed the issuance (one-shot only).
    pub consumed: bool,
    pub state: RequestState,
}

/// One live issuance, as `status` reports it. Names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveIssuance {
    pub request_id: String,
    pub seat: String,
    pub until: String,
    pub state: RequestState,
    pub one_shot: bool,
}

/// One secret's status (design section 4.9). Names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretStatus {
    pub name: String,
    pub registered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<SecretClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default)]
    pub deregistered: bool,
    #[serde(default)]
    pub live: Vec<LiveIssuance>,
    /// Next rotation due, from registration plus the rotation
    /// interval. Absent for an unregistered secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_rotation_due: Option<String>,
}

/// The three files a store reads and writes. `dir` is the `<home>/.zen`
/// directory; the caller resolves it (the CLI honors `HOME` first, as
/// the rest of the CLI does, so tests can point it at a temp dir).
#[derive(Debug, Clone)]
pub struct SecretStore {
    pub dir: PathBuf,
}

impl SecretStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn registry_path(&self) -> PathBuf {
        self.dir.join(REGISTRY_FILE)
    }

    pub fn requests_path(&self) -> PathBuf {
        self.dir.join(REQUESTS_FILE)
    }

    pub fn audit_path(&self) -> PathBuf {
        self.dir.join(AUDIT_FILE)
    }
}

fn rfc3339(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn parse_time(value: &str) -> SecretResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| {
            SecretError::validation(format!("invalid timestamp '{value}': expected RFC 3339"))
        })
}

/// A secret name is upper snake case, starting with a letter, at most
/// 64 characters.
pub fn valid_secret_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_SECRET_NAME_LEN {
        return false;
    }
    let mut chars = name.chars();
    if !chars
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false)
    {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// The writer refusal (DS-5, reused by DS-7): a field carrying a
/// token-shaped value is refused, with the pattern named, not the
/// value. Returns the pattern name when `text` carries one.
pub fn secret_shaped_pattern(text: &str) -> Option<&'static str> {
    let bytes = text.as_bytes();
    // A 40-character hex string.
    let mut run = 0usize;
    for &b in bytes {
        if b.is_ascii_hexdigit() {
            run += 1;
            if run >= 40 {
                return Some("40-hex");
            }
        } else {
            run = 0;
        }
    }
    // A `sk-...` string: `sk-` followed by at least 8 token characters.
    let mut i = 0usize;
    while i + 3 < bytes.len() {
        if &bytes[i..i + 3] == b"sk-" {
            let mut n = 0usize;
            let mut j = i + 3;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
            {
                n += 1;
                j += 1;
            }
            if n >= 8 {
                return Some("sk-prefix");
            }
            i = j;
        } else {
            i += 1;
        }
    }
    // A `*_KEY=...` assignment: an upper snake identifier ending in
    // `_KEY`, followed by optional spaces and `=`.
    let mut k = 0usize;
    while k + 4 <= bytes.len() {
        if &bytes[k..k + 4] == b"_KEY" {
            let mut j = k + 4;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                // The identifier before `_KEY` must be upper snake.
                let mut start = k;
                while start > 0
                    && (bytes[start - 1].is_ascii_uppercase()
                        || bytes[start - 1].is_ascii_digit()
                        || bytes[start - 1] == b'_')
                {
                    start -= 1;
                }
                if k > start {
                    return Some("KEY-assignment");
                }
            }
            k = j;
        } else {
            k += 1;
        }
    }
    None
}

fn refuse_shaped(field: &str, value: &str) -> SecretResult<()> {
    if let Some(pattern) = secret_shaped_pattern(value) {
        return Err(SecretError::validation(format!(
            "refused: field '{field}' carries a secret-shaped value (pattern {pattern}); values never go in a request, approval, or audit record"
        )));
    }
    Ok(())
}

fn refuse_shaped_opt(field: &str, value: Option<&str>) -> SecretResult<()> {
    if let Some(v) = value {
        refuse_shaped(field, v)?;
    }
    Ok(())
}

fn append_line(path: &Path, line: &str) -> SecretResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            SecretError::new(
                exit_code::GENERAL,
                format!("cannot create {}: {e}", parent.display()),
            )
        })?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| {
            SecretError::new(
                exit_code::GENERAL,
                format!("cannot open {}: {e}", path.display()),
            )
        })?;
    writeln!(file, "{line}").map_err(|e| {
        SecretError::new(
            exit_code::GENERAL,
            format!("cannot write {}: {e}", path.display()),
        )
    })?;
    Ok(())
}

fn load_jsonl<T>(path: &Path, what: &str) -> SecretResult<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(SecretError::new(
                exit_code::GENERAL,
                format!("cannot read {}: {e}", path.display()),
            ));
        }
    };
    let mut out = Vec::new();
    for (i, raw) in data.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let value: T = serde_json::from_str(line).map_err(|_| {
            SecretError::new(
                exit_code::GENERAL,
                format!(
                    "{} {} line {} is not valid JSON; the file is append-only, fix or move it",
                    what,
                    path.display(),
                    i + 1
                ),
            )
        })?;
        out.push(value);
    }
    Ok(out)
}

/// Load the registry, folding lines in order: the latest line for a
/// name wins, so a deregistration line supersedes the registration.
pub fn load_registry(store: &SecretStore) -> SecretResult<Vec<RegistryEntry>> {
    let lines: Vec<RegistryEntry> = load_jsonl(&store.registry_path(), "registry")?;
    let mut entries: Vec<RegistryEntry> = Vec::new();
    for entry in lines {
        if let Some(slot) = entries.iter_mut().find(|e| e.name == entry.name) {
            *slot = entry;
        } else {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Load the requests, folding lines in order: the latest snapshot for
/// a request id wins.
pub fn load_requests(store: &SecretStore) -> SecretResult<Vec<SecretRequest>> {
    let lines: Vec<SecretRequest> = load_jsonl(&store.requests_path(), "request")?;
    let mut requests: Vec<SecretRequest> = Vec::new();
    for req in lines {
        if let Some(slot) = requests.iter_mut().find(|r| r.request_id == req.request_id) {
            *slot = req;
        } else {
            requests.push(req);
        }
    }
    Ok(requests)
}

pub fn load_audit(store: &SecretStore) -> SecretResult<Vec<AuditEntry>> {
    load_jsonl(&store.audit_path(), "audit")
}

fn append_request(store: &SecretStore, req: &SecretRequest) -> SecretResult<()> {
    let line = serde_json::to_string(req).map_err(|e| {
        SecretError::new(exit_code::GENERAL, format!("cannot serialize request: {e}"))
    })?;
    append_line(&store.requests_path(), &line)
}

fn append_audit(store: &SecretStore, entry: &AuditEntry) -> SecretResult<()> {
    let line = serde_json::to_string(entry).map_err(|e| {
        SecretError::new(
            exit_code::GENERAL,
            format!("cannot serialize audit entry: {e}"),
        )
    })?;
    append_line(&store.audit_path(), &line)
}

fn audit(
    store: &SecretStore,
    req: &SecretRequest,
    actor: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> SecretResult<()> {
    append_audit(
        store,
        &AuditEntry {
            v: 1,
            at: rfc3339(now),
            request_id: req.request_id.clone(),
            name: req.name.clone(),
            seat: req.seat.clone(),
            state: req.state,
            actor: actor.to_string(),
            reason: reason.to_string(),
            evidence: req.evidence.clone(),
        },
    )
}

/// The state a request is effectively in at `now`: an `issued` or
/// `active` issuance past its `until` is `expired`; a live issuance
/// under a deregistered name, or past its rotation grace, is
/// `revoked`. A request that only waits (`requested`, `approved`) past
/// its `until` has lapsed and is `expired` too.
pub fn effective_state(
    req: &SecretRequest,
    now: DateTime<Utc>,
    deregistered: bool,
) -> RequestState {
    let until = parse_time(&req.until).unwrap_or(now);
    match req.state {
        RequestState::Requested | RequestState::Approved if now > until => RequestState::Expired,
        RequestState::Issued | RequestState::Active => {
            if deregistered {
                RequestState::Revoked
            } else if now > until {
                RequestState::Expired
            } else if let Some(grace) = req.grace_until.as_deref() {
                // The grace only exists because a rotation started: once
                // it passes, the old value is dead because a new value
                // replaced it, so the state is `rotated`, not `revoked`.
                match parse_time(grace) {
                    Ok(g) if now > g => RequestState::Rotated,
                    _ => req.state,
                }
            } else {
                req.state
            }
        }
        other => other,
    }
}

/// Input for registering a secret (design section 4.3). Registration
/// is a Leader act, recorded with evidence.
pub struct RegisterInput {
    pub name: String,
    pub class: SecretClass,
    pub service: String,
    pub owner: String,
    pub registered_by: String,
    pub default_until_days: i64,
    pub rotation_days: i64,
    pub paid: bool,
    pub reason: String,
}

/// Register a secret name. A request naming an unregistered name is
/// refused; a typo'd name fails closed instead of matching something
/// near it, because names match exactly and nothing else.
pub fn register_secret(
    store: &SecretStore,
    input: RegisterInput,
    now: DateTime<Utc>,
) -> SecretResult<RegistryEntry> {
    if !valid_secret_name(&input.name) {
        return Err(SecretError::validation(format!(
            "invalid secret name '{}': use upper snake case, starting with a letter, at most {MAX_SECRET_NAME_LEN} characters",
            input.name
        )));
    }
    // Registration is a Leader act (design section 4.3): a seat may
    // propose a registration, only the Leader lands it.
    if input.registered_by != LEADER_APPROVER {
        return Err(SecretError::forbidden(format!(
            "only '{LEADER_APPROVER}' registers a secret: '{}' cannot register '{}'",
            input.registered_by, input.name
        )));
    }
    refuse_shaped("service", &input.service)?;
    refuse_shaped("owner", &input.owner)?;
    refuse_shaped("reason", &input.reason)?;
    if input.default_until_days < 1 {
        return Err(SecretError::validation(
            "default_until_days must be at least 1: every issuance carries an until",
        ));
    }
    if input.rotation_days < 1 {
        return Err(SecretError::validation("rotation_days must be at least 1"));
    }
    let entry = RegistryEntry {
        v: 1,
        name: input.name.clone(),
        class: input.class,
        service: input.service,
        owner: input.owner,
        registered_by: input.registered_by,
        registered_at: rfc3339(now),
        default_until_days: input.default_until_days,
        rotation_days: input.rotation_days,
        paid: input.paid,
        reason: input.reason,
        deregistered: false,
    };
    let line = serde_json::to_string(&entry).map_err(|e| {
        SecretError::new(
            exit_code::GENERAL,
            format!("cannot serialize registry entry: {e}"),
        )
    })?;
    append_line(&store.registry_path(), &line)?;
    Ok(entry)
}

/// Deregister a secret name: a new registry line marks it deregistered,
/// and every live issuance under it is revoked at once (criterion 10).
/// The request records stay, so a revoked secret's history is
/// explainable.
pub fn deregister_secret(
    store: &SecretStore,
    name: &str,
    by: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> SecretResult<RegistryEntry> {
    let registry = load_registry(store)?;
    let Some(entry) = registry.iter().find(|e| e.name == name).cloned() else {
        return Err(SecretError::not_found(format!(
            "secret '{name}' is not in the registry; register it before deregistering it"
        )));
    };
    if entry.deregistered {
        return Err(SecretError::conflict(format!(
            "secret '{name}' is already deregistered"
        )));
    }
    // Deregistration is a registration line, so the same Leader-only
    // rule applies (design section 4.3).
    if by != LEADER_APPROVER {
        return Err(SecretError::forbidden(format!(
            "only '{LEADER_APPROVER}' deregisters a secret: '{by}' cannot deregister '{name}'"
        )));
    }
    refuse_shaped("reason", reason)?;
    let mut updated = entry.clone();
    updated.deregistered = true;
    updated.registered_by = by.to_string();
    updated.registered_at = rfc3339(now);
    updated.reason = reason.to_string();
    let line = serde_json::to_string(&updated).map_err(|e| {
        SecretError::new(
            exit_code::GENERAL,
            format!("cannot serialize registry entry: {e}"),
        )
    })?;
    append_line(&store.registry_path(), &line)?;
    let requests = load_requests(store)?;
    for req in requests
        .iter()
        .filter(|r| r.name == name)
        .filter(|r| matches!(r.state, RequestState::Issued | RequestState::Active))
    {
        let mut revoked = req.clone();
        revoked.state = RequestState::Revoked;
        revoked.deny_reason = Some(format!("secret '{name}' deregistered"));
        append_request(store, &revoked)?;
        audit(
            store,
            &revoked,
            by,
            "secret deregistered; live issuance revoked",
            now,
        )?;
    }
    Ok(updated)
}

/// Input for filing a secret request (design section 4.4).
pub struct RequestInput {
    pub name: String,
    pub seat: String,
    pub task_id: Option<String>,
    pub purpose: String,
    pub scope: String,
    /// RFC 3339 timestamp. Mandatory: there is no request "for general
    /// use" and no request without an end date.
    pub until: String,
    pub one_shot: bool,
    /// Required for a user-class secret: the broker action the work
    /// needs. A user secret is never requested by name alone as if it
    /// were a seat secret.
    pub broker_action: Option<String>,
}

fn next_request_id(requests: &[SecretRequest], now: DateTime<Utc>) -> String {
    let day = now.format("%Y%m%d").to_string();
    let prefix = format!("sr-{day}-");
    let count = requests
        .iter()
        .filter(|r| r.request_id.starts_with(&prefix))
        .count();
    format!("{prefix}{:03}", count + 1)
}

/// D2: the longest `until` a new request for this entry may carry.
/// A one-shot issuance and a user-class broker grant live at most 24
/// hours, even when the registry default for the secret is longer;
/// anything else lives at most the registry default. The registry
/// default is at least 1 day by construction, so the 24-hour cap is
/// always the binding one when it applies.
fn max_until(entry: &RegistryEntry, one_shot: bool, now: DateTime<Utc>) -> DateTime<Utc> {
    let default_until = now + Duration::days(entry.default_until_days);
    let short_cap = if one_shot {
        now + Duration::hours(ONE_SHOT_LIFETIME_HOURS)
    } else if entry.class == SecretClass::User {
        now + Duration::hours(BROKER_GRANT_LIFETIME_HOURS)
    } else {
        return default_until;
    };
    default_until.min(short_cap)
}

/// File a request (state: `requested`). Refused without purpose, scope,
/// or until; refused for an unregistered name; refused for a user/Zen
/// secret asked as a value; refused when a field carries a
/// secret-shaped value. Prints nothing; the caller reports the id.
pub fn request_secret(
    store: &SecretStore,
    input: RequestInput,
    now: DateTime<Utc>,
) -> SecretResult<SecretRequest> {
    if !valid_secret_name(&input.name) {
        return Err(SecretError::validation(format!(
            "invalid secret name '{}': use upper snake case, starting with a letter",
            input.name
        )));
    }
    let registry = load_registry(store)?;
    let Some(entry) = registry.iter().find(|e| e.name == input.name) else {
        return Err(SecretError::validation(format!(
            "unknown-secret: '{}' is not in the registry; the registry is the place to fix it",
            input.name
        )));
    };
    if entry.deregistered {
        return Err(SecretError::validation(format!(
            "secret '{}' is deregistered; it refuses all new requests",
            input.name
        )));
    }
    if input.purpose.trim().is_empty() {
        return Err(SecretError::validation(
            "a request without a purpose is refused: the missing field is purpose",
        ));
    }
    if input.scope.trim().is_empty() {
        return Err(SecretError::validation(
            "a request without a scope is refused: the missing field is scope",
        ));
    }
    if input.until.trim().is_empty() {
        return Err(SecretError::validation(
            "a request without an until is refused: the missing field is until",
        ));
    }
    let until = parse_time(&input.until)?;
    if until <= now {
        return Err(SecretError::validation(format!(
            "a request with a past until is refused: until {} has already passed",
            input.until
        )));
    }
    // A request may ask for less, never for more than the registry
    // default for that secret without a Leader decision (D2). A
    // one-shot issuance or a broker grant is refused past 24 hours
    // even when the registry default is longer (F1): the constants
    // above are the cap, not a comment.
    if input.one_shot || entry.class == SecretClass::User {
        let kind = if input.one_shot {
            "one-shot"
        } else {
            "broker grant"
        };
        let cap = max_until(entry, input.one_shot, now);
        if until > cap {
            return Err(SecretError::validation(format!(
                "lifetime-exceeds-24h: until {} is past the 24-hour limit for a {kind} of '{}'; ask for less, or get a Leader decision",
                input.until, input.name
            )));
        }
    }
    let default_until = now + Duration::days(entry.default_until_days);
    if until > default_until {
        return Err(SecretError::validation(format!(
            "lifetime-exceeds-default: until {} is past the registry default of {} days for '{}'; ask for less, or get a Leader decision",
            input.until, entry.default_until_days, input.name
        )));
    }
    // A user/Zen secret asked as a value is refused with the class
    // named. An unknown class is treated as user class until
    // registered, and the registry is the only source of the class,
    // so reaching here with class user means the name is known.
    if entry.class == SecretClass::User
        && input
            .broker_action
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
    {
        return Err(SecretError::validation(
            "refused: a user-class secret is never issued as a value; name the broker action the work needs",
        ));
    }
    refuse_shaped("purpose", &input.purpose)?;
    refuse_shaped("scope", &input.scope)?;
    refuse_shaped_opt("broker_action", input.broker_action.as_deref())?;
    refuse_shaped_opt("task_id", input.task_id.as_deref())?;
    let requests = load_requests(store)?;
    let req = SecretRequest {
        v: 1,
        request_id: next_request_id(&requests, now),
        name: input.name,
        class: entry.class,
        seat: input.seat.clone(),
        task_id: input.task_id,
        purpose: input.purpose,
        scope: input.scope,
        until: rfc3339(until),
        state: RequestState::Requested,
        requested_at: rfc3339(now),
        requested_by: input.seat.clone(),
        decided_by: None,
        decided_at: None,
        evidence: None,
        deny_reason: None,
        one_shot: input.one_shot,
        broker_action: input.broker_action,
        issued_by: None,
        issued_at: None,
        vault_entry: None,
        rotates: None,
        grace_until: None,
        consumed_at: None,
    };
    append_request(store, &req)?;
    audit(store, &req, &req.requested_by, "request filed", now)?;
    Ok(req)
}

fn load_request(store: &SecretStore, request_id: &str) -> SecretResult<SecretRequest> {
    let requests = load_requests(store)?;
    requests
        .into_iter()
        .find(|r| r.request_id == request_id)
        .ok_or_else(|| SecretError::not_found(format!("request '{request_id}' not found")))
}

/// The approver the D1 rule requires for a request: the Leader for a
/// seat secret, Zen for a user-class secret or a paid-use secret.
fn required_approver(req: &SecretRequest, registry: &[RegistryEntry]) -> &'static str {
    let paid = registry
        .iter()
        .find(|e| e.name == req.name)
        .map(|e| e.paid)
        .unwrap_or(false);
    if req.class == SecretClass::User || paid {
        ZEN_APPROVER
    } else {
        LEADER_APPROVER
    }
}

/// Move a request to `approved` (D1). Refused when the approver is the
/// requesting seat (exit 5), or when the approver lacks the authority
/// for the secret class (exit 5).
pub fn approve_secret(
    store: &SecretStore,
    request_id: &str,
    by: &str,
    evidence: &str,
    now: DateTime<Utc>,
) -> SecretResult<SecretRequest> {
    let mut req = load_request(store, request_id)?;
    if req.state != RequestState::Requested {
        return Err(SecretError::conflict(format!(
            "request '{request_id}' is {} and cannot be approved; only a requested request can be approved",
            req.state.as_str()
        )));
    }
    if by == req.seat {
        return Err(SecretError::forbidden(format!(
            "a seat never approves its own request: '{by}' filed request '{request_id}'"
        )));
    }
    let registry = load_registry(store)?;
    let required = required_approver(&req, &registry);
    if by != required {
        return Err(SecretError::forbidden(format!(
            "approver '{by}' lacks the authority for this request: a {} secret is approved by '{required}'",
            req.class.as_str()
        )));
    }
    refuse_shaped("evidence", evidence)?;
    let until = parse_time(&req.until)?;
    if now > until {
        return Err(SecretError::conflict(format!(
            "request '{request_id}' has expired: its until {} has passed",
            req.until
        )));
    }
    req.state = RequestState::Approved;
    req.decided_by = Some(by.to_string());
    req.decided_at = Some(rfc3339(now));
    req.evidence = Some(evidence.to_string());
    append_request(store, &req)?;
    audit(store, &req, by, "request approved", now)?;
    Ok(req)
}

/// Move a request to `denied`, with the reason named. Only the
/// approver for the secret class denies (design section 4.4: the
/// `denied` actor is "the approver", the same D1 authority as
/// `approve`). A denied request is never re-approved in place: the
/// seat files a new request.
pub fn deny_secret(
    store: &SecretStore,
    request_id: &str,
    by: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> SecretResult<SecretRequest> {
    let mut req = load_request(store, request_id)?;
    if req.state != RequestState::Requested {
        return Err(SecretError::conflict(format!(
            "request '{request_id}' is {} and cannot be denied; only a requested request can be denied",
            req.state.as_str()
        )));
    }
    let registry = load_registry(store)?;
    let required = required_approver(&req, &registry);
    if by != required {
        return Err(SecretError::forbidden(format!(
            "denier '{by}' lacks the authority for this request: a {} secret is denied by '{required}'",
            req.class.as_str()
        )));
    }
    refuse_shaped("reason", reason)?;
    req.state = RequestState::Denied;
    req.decided_by = Some(by.to_string());
    req.decided_at = Some(rfc3339(now));
    req.deny_reason = Some(reason.to_string());
    append_request(store, &req)?;
    audit(store, &req, by, reason, now)?;
    Ok(req)
}

/// Withdraw a request (follow-up F1, Leader tin 8174): the requesting
/// seat takes back its own request while it is still undecided. Only
/// from `requested`, and only by the seat that filed it (exit 5
/// otherwise): the Leader denies instead of withdrawing, and any
/// other seat has no act of its own on the request. A withdrawn
/// request is terminal: it is never approved, denied, issued, or
/// revoked afterwards, and the seat files a new request if it still
/// needs the secret. An approved request cannot be withdrawn - it
/// lapses at its `until` - and a live issuance is ended by `revoke`,
/// not by withdraw. The reason is recorded in `deny_reason`, the
/// terminal-reason field, and in the audit line.
pub fn withdraw_secret(
    store: &SecretStore,
    request_id: &str,
    by: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> SecretResult<SecretRequest> {
    let mut req = load_request(store, request_id)?;
    if req.state != RequestState::Requested {
        return Err(SecretError::conflict(format!(
            "request '{request_id}' is {} and cannot be withdrawn; only a requested request can be withdrawn",
            req.state.as_str()
        )));
    }
    if by != req.seat {
        return Err(SecretError::forbidden(format!(
            "only the requesting seat '{}' withdraws request '{request_id}': '{by}' did not file it",
            req.seat
        )));
    }
    refuse_shaped("reason", reason)?;
    req.state = RequestState::Withdrawn;
    req.deny_reason = Some(reason.to_string());
    append_request(store, &req)?;
    audit(store, &req, by, reason, now)?;
    Ok(req)
}

/// The check that gates issuance (design section 4.5, step 1): the
/// request must be `approved`, the name registered and not
/// deregistered, and the `until` still in the future. Until the DS-5
/// grant store exists, the approval recorded here is the ALLOW
/// evidence; see the module docs.
pub fn check_issue(
    store: &SecretStore,
    req: &SecretRequest,
    now: DateTime<Utc>,
) -> SecretResult<CheckOutcome> {
    if req.state != RequestState::Approved {
        return Ok(CheckOutcome {
            allowed: false,
            reason: format!(
                "not approved: request '{}' is {}",
                req.request_id,
                req.state.as_str()
            ),
        });
    }
    let registry = load_registry(store)?;
    match registry.iter().find(|e| e.name == req.name) {
        None => Ok(CheckOutcome {
            allowed: false,
            reason: format!("unknown-secret: '{}' is not in the registry", req.name),
        }),
        Some(entry) if entry.deregistered => Ok(CheckOutcome {
            allowed: false,
            reason: format!("deregistered: secret '{}' is deregistered", req.name),
        }),
        Some(_) => {
            let until = parse_time(&req.until)?;
            if now > until {
                Ok(CheckOutcome {
                    allowed: false,
                    reason: format!("expired: until {} has passed", req.until),
                })
            } else {
                Ok(CheckOutcome {
                    allowed: true,
                    reason: format!("approved by {}", req.decided_by.as_deref().unwrap_or("?")),
                })
            }
        }
    }
}

/// Record the issuance (state: `issued`). The issuer is the human who
/// placed the value; the command records the act, it does not carry
/// the value (D3). Refused when the issuance check does not ALLOW, and
/// when the issuer is the requesting seat. For a user-class secret the
/// issuance is a broker grant: no value is placed, so no Vault entry
/// is recorded.
pub fn issue_secret(
    store: &SecretStore,
    request_id: &str,
    by: &str,
    now: DateTime<Utc>,
) -> SecretResult<SecretRequest> {
    let mut req = load_request(store, request_id)?;
    if by != LEADER_APPROVER && by != OPERATOR_ISSUER {
        return Err(SecretError::forbidden(format!(
            "issuer '{by}' cannot place a secret value: only '{LEADER_APPROVER}' or '{OPERATOR_ISSUER}' issues"
        )));
    }
    if by == req.seat {
        return Err(SecretError::forbidden(format!(
            "a seat never issues its own secret: '{by}' filed request '{request_id}'"
        )));
    }
    let check = check_issue(store, &req, now)?;
    if !check.allowed {
        return Err(SecretError::conflict(format!(
            "issuance refused for request '{request_id}': {}",
            check.reason
        )));
    }
    req.state = RequestState::Issued;
    req.issued_by = Some(by.to_string());
    req.issued_at = Some(rfc3339(now));
    // D4: one Vault entry per (secret, seat) pair. A user-class broker
    // grant issues no value, so it records no entry.
    if req.class == SecretClass::Seat {
        req.vault_entry = Some(format!("{}:{}", req.name, req.seat));
    }
    append_request(store, &req)?;
    let reason = if req.class == SecretClass::User {
        "broker grant issued; no value placed"
    } else {
        "value placed in the seat runtime; see Vault entry"
    };
    audit(store, &req, by, reason, now)?;
    Ok(req)
}

/// Move a live issuance to `revoked`. Immediate: the next check
/// refuses. Only the issuer who placed the value, or the Leader,
/// revokes (design sections 4.4 and 4.7: "the issuer or the Leader") -
/// and, for a user-class broker grant, Zen too (follow-up F2, Leader
/// tin 8174): Zen owns the credential the grant touches, so he can
/// cut the grant off directly instead of asking the issuer or the
/// Leader. A seat-class issuance is unchanged: Zen cannot revoke it.
/// Revocation never deletes the request record.
pub fn revoke_secret(
    store: &SecretStore,
    request_id: &str,
    by: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> SecretResult<SecretRequest> {
    let mut req = load_request(store, request_id)?;
    if !matches!(req.state, RequestState::Issued | RequestState::Active) {
        return Err(SecretError::conflict(format!(
            "request '{request_id}' is {} and has no live issuance to revoke",
            req.state.as_str()
        )));
    }
    let issuer = req.issued_by.as_deref().unwrap_or("");
    // F2: Zen revokes a user-class broker grant directly. The class
    // check keeps this closed for seat secrets, where Zen is still
    // refused (the issuer or the Leader revokes those).
    let zen_may_revoke = req.class == SecretClass::User && by == ZEN_APPROVER;
    if by != issuer && by != LEADER_APPROVER && !zen_may_revoke {
        let allowed = if req.class == SecretClass::User {
            format!(
                "only the issuer '{issuer}', '{LEADER_APPROVER}', or '{ZEN_APPROVER}' (for a user-class broker grant) revokes"
            )
        } else {
            format!("only the issuer '{issuer}' or '{LEADER_APPROVER}' revokes")
        };
        return Err(SecretError::forbidden(format!(
            "revoker '{by}' cannot revoke request '{request_id}': {allowed}"
        )));
    }
    refuse_shaped("reason", reason)?;
    req.state = RequestState::Revoked;
    req.deny_reason = Some(reason.to_string());
    append_request(store, &req)?;
    audit(store, &req, by, reason, now)?;
    Ok(req)
}

/// Start a rotation (design section 4.7): a new request linked to the
/// old one by `rotates`, and the old issuance stays live only until
/// the end of the rotation grace period (D2, 24 hours), after which
/// the check refuses it. The new value is issued under the new request
/// once it is approved and issued; this function files the request,
/// it does not place a value.
pub fn rotate_secret(
    store: &SecretStore,
    name: &str,
    by: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> SecretResult<SecretRequest> {
    let registry = load_registry(store)?;
    let Some(entry) = registry.iter().find(|e| e.name == name).cloned() else {
        return Err(SecretError::not_found(format!(
            "secret '{name}' is not in the registry"
        )));
    };
    if entry.deregistered {
        return Err(SecretError::validation(format!(
            "secret '{name}' is deregistered; it cannot be rotated"
        )));
    }
    if by != entry.owner && by != LEADER_APPROVER {
        return Err(SecretError::forbidden(format!(
            "only the owner '{}' or '{LEADER_APPROVER}' rotates secret '{name}'",
            entry.owner
        )));
    }
    refuse_shaped("reason", reason)?;
    let requests = load_requests(store)?;
    let Some(old) = requests
        .iter()
        .filter(|r| r.name == name)
        .filter(|r| {
            matches!(
                effective_state(r, now, false),
                RequestState::Issued | RequestState::Active
            )
        })
        .max_by_key(|r| r.requested_at.clone())
        .cloned()
    else {
        return Err(SecretError::conflict(format!(
            "secret '{name}' has no live issuance to rotate"
        )));
    };
    let mut old = old;
    old.grace_until = Some(rfc3339(now + Duration::hours(ROTATION_GRACE_HOURS)));
    append_request(store, &old)?;
    audit(
        store,
        &old,
        by,
        &format!(
            "rotation started; grace until {}",
            old.grace_until.as_deref().unwrap_or("?")
        ),
        now,
    )?;
    // The new request carries the same cap as a fresh request: a
    // broker grant rotated here lives at most 24 hours (D2, F1).
    let until = max_until(&entry, false, now);
    let new_req = SecretRequest {
        v: 1,
        request_id: next_request_id(&requests, now),
        name: name.to_string(),
        class: entry.class,
        seat: old.seat.clone(),
        task_id: old.task_id.clone(),
        purpose: format!("rotation of {}", old.request_id),
        scope: old.scope.clone(),
        until: rfc3339(until),
        state: RequestState::Requested,
        requested_at: rfc3339(now),
        requested_by: by.to_string(),
        decided_by: None,
        decided_at: None,
        evidence: None,
        deny_reason: None,
        one_shot: false,
        broker_action: old.broker_action.clone(),
        issued_by: None,
        issued_at: None,
        vault_entry: None,
        rotates: Some(old.request_id.clone()),
        grace_until: None,
        consumed_at: None,
    };
    append_request(store, &new_req)?;
    audit(store, &new_req, by, reason, now)?;
    Ok(new_req)
}

/// The check that gates a use of an issued secret (design section 5.1).
/// A probe: it never consumes a one-shot issuance and never writes.
/// It re-reads the files every time, so a revocation is immediate.
/// The caller names the seat presenting the value: a secret issued to
/// one seat is not usable by another.
pub fn check_use(
    store: &SecretStore,
    request_id: &str,
    seat: &str,
    now: DateTime<Utc>,
) -> SecretResult<CheckOutcome> {
    let req = load_request(store, request_id)?;
    let registry = load_registry(store)?;
    let deregistered = registry
        .iter()
        .find(|e| e.name == req.name)
        .map(|e| e.deregistered)
        .unwrap_or(true);
    if deregistered {
        return Ok(CheckOutcome {
            allowed: false,
            reason: format!("deregistered: secret '{}' is deregistered", req.name),
        });
    }
    if req.seat != seat {
        return Ok(CheckOutcome {
            allowed: false,
            reason: format!(
                "wrong seat: request '{}' was issued to '{}', not '{seat}'",
                req.request_id, req.seat
            ),
        });
    }
    match effective_state(&req, now, false) {
        RequestState::Issued | RequestState::Active => {
            if req.one_shot && req.consumed_at.is_some() {
                Ok(CheckOutcome {
                    allowed: false,
                    reason: format!(
                        "one-shot consumed: request '{}' was consumed at {}",
                        req.request_id,
                        req.consumed_at.as_deref().unwrap_or("?")
                    ),
                })
            } else {
                Ok(CheckOutcome {
                    allowed: true,
                    reason: format!("live issuance until {}", req.until),
                })
            }
        }
        RequestState::Expired => Ok(CheckOutcome {
            allowed: false,
            reason: format!("expired: until {} has passed", req.until),
        }),
        RequestState::Revoked => Ok(CheckOutcome {
            allowed: false,
            reason: format!(
                "revoked: {}",
                req.deny_reason.as_deref().unwrap_or("revoked")
            ),
        }),
        RequestState::Rotated => Ok(CheckOutcome {
            allowed: false,
            reason: format!(
                "rotated: a new value replaced this one; rotation grace ended at {}",
                req.grace_until.as_deref().unwrap_or("?")
            ),
        }),
        other => Ok(CheckOutcome {
            allowed: false,
            reason: format!(
                "not issued: request '{}' is {}",
                req.request_id,
                other.as_str()
            ),
        }),
    }
}

/// Record a real use of an issued secret. A probe (`check_use`) never
/// calls this. The first use moves `issued` to `active`; a one-shot
/// issuance is consumed by this use and then revoked, so a second use
/// is a `DENY` (criterion 7).
pub fn record_use(
    store: &SecretStore,
    request_id: &str,
    seat: &str,
    now: DateTime<Utc>,
) -> SecretResult<UseOutcome> {
    let check = check_use(store, request_id, seat, now)?;
    if !check.allowed {
        return Err(SecretError::forbidden(format!(
            "use refused for request '{request_id}': {}",
            check.reason
        )));
    }
    let mut req = load_request(store, request_id)?;
    if req.one_shot {
        req.state = RequestState::Revoked;
        req.consumed_at = Some(rfc3339(now));
        req.deny_reason = Some("one-shot consumed by first use".to_string());
        append_request(store, &req)?;
        audit(store, &req, seat, "one-shot consumed by first use", now)?;
        Ok(UseOutcome {
            consumed: true,
            state: RequestState::Revoked,
        })
    } else {
        if req.state == RequestState::Issued {
            req.state = RequestState::Active;
            append_request(store, &req)?;
            audit(store, &req, seat, "first use; issuance active", now)?;
        }
        Ok(UseOutcome {
            consumed: false,
            state: req.state,
        })
    }
}

/// List requests by name, seat, state, and until (effective states at
/// `now`). Values never appear in the output.
pub fn list_requests(
    store: &SecretStore,
    seat: Option<&str>,
    state: Option<RequestState>,
    now: DateTime<Utc>,
) -> SecretResult<Vec<SecretRequest>> {
    let registry = load_registry(store)?;
    let mut requests = load_requests(store)?;
    for req in requests.iter_mut() {
        let deregistered = registry
            .iter()
            .find(|e| e.name == req.name)
            .map(|e| e.deregistered)
            .unwrap_or(false);
        req.state = effective_state(req, now, deregistered);
    }
    requests.retain(|r| seat.map(|s| r.seat == s).unwrap_or(true));
    requests.retain(|r| state.map(|s| r.state == s).unwrap_or(true));
    requests.sort_by(|a, b| a.requested_at.cmp(&b.requested_at));
    Ok(requests)
}

/// Per-secret status: registered or not, class, live issuances with
/// their seats and untils, next rotation due. Names only.
pub fn secret_status(
    store: &SecretStore,
    name: Option<&str>,
    now: DateTime<Utc>,
) -> SecretResult<Vec<SecretStatus>> {
    let registry = load_registry(store)?;
    let requests = load_requests(store)?;
    let names: Vec<String> = match name {
        Some(n) => vec![n.to_string()],
        None => registry.iter().map(|e| e.name.clone()).collect(),
    };
    let mut out = Vec::new();
    for n in names {
        let entry = registry.iter().find(|e| e.name == n);
        let deregistered = entry.map(|e| e.deregistered).unwrap_or(false);
        let live: Vec<LiveIssuance> = requests
            .iter()
            .filter(|r| r.name == n)
            .filter(|r| {
                matches!(
                    effective_state(r, now, deregistered),
                    RequestState::Issued | RequestState::Active
                )
            })
            .map(|r| LiveIssuance {
                request_id: r.request_id.clone(),
                seat: r.seat.clone(),
                until: r.until.clone(),
                state: effective_state(r, now, deregistered),
                one_shot: r.one_shot,
            })
            .collect();
        let next_rotation_due = entry.map(|e| {
            let registered_at = parse_time(&e.registered_at).unwrap_or(now);
            rfc3339(registered_at + Duration::days(e.rotation_days))
        });
        out.push(SecretStatus {
            name: n,
            registered: entry.is_some(),
            class: entry.map(|e| e.class),
            service: entry.map(|e| e.service.clone()),
            owner: entry.map(|e| e.owner.clone()),
            deregistered,
            live,
            next_rotation_due,
        });
    }
    Ok(out)
}

/// Percent-encode a value the way a URL would carry it: every byte
/// outside the unreserved set becomes `%XX`, including the characters
/// `encodeURIComponent` leaves alone (`'`, `(`, `)`, `*`), so the sent
/// URL holds the same form the redactor looks for.
fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for &b in value.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The JSON-escaped form of a value: the string as it appears inside a
/// JSON document, without the surrounding quotes.
fn json_escaped(value: &str) -> String {
    let quoted = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    quoted
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(&quoted)
        .to_string()
}

/// Replace every known secret value in `text` with `[secret:NAME]`.
///
/// The replacement covers the value verbatim, its JSON-escaped form,
/// and its URL-encoded form, longest value first, in one pass, so a
/// label already written is never matched again. A short value (under
/// 8 characters) is replaced only where it stands alone, so an
/// ordinary word that happens to contain it is left untouched.
///
/// The honest limit, stated in the design (section 4.5) and in the
/// tests: this catches the value verbatim and in its two encoded
/// forms only. A value that is split, reversed, or otherwise
/// transformed is NOT covered. The defences that actually hold are
/// structural - the value never enters the model's prompt where a
/// broker exists, the value never appears in a file the model can
/// read, and the issuance expires. Redaction is the tripwire, not
/// the wall.
pub fn redact_text(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut forms: Vec<(String, String, bool)> = Vec::new();
    for (name, value) in pairs {
        if value.len() < 4 {
            continue;
        }
        let label = format!("[secret:{name}]");
        let short = value.len() < 8;
        for form in [value.to_string(), json_escaped(value), url_encode(value)] {
            if form.is_empty() || forms.iter().any(|(f, _, _)| *f == form) {
                continue;
            }
            forms.push((form, label.clone(), short));
        }
    }
    forms.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
    if forms.is_empty() || !forms.iter().any(|(f, _, _)| text.contains(f.as_str())) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < text.len() {
        let mut matched: Option<(&str, usize)> = None;
        for (form, label, short) in &forms {
            if text[i..].starts_with(form.as_str()) {
                if *short {
                    let before = text[..i].chars().last();
                    let after = text[i + form.len()..].chars().next();
                    let boundary =
                        |c: Option<char>| c.map(|c| !c.is_alphanumeric()).unwrap_or(true);
                    if !boundary(before) || !boundary(after) {
                        continue;
                    }
                }
                matched = Some((label.as_str(), form.len()));
                break;
            }
        }
        match matched {
            Some((label, len)) => {
                out.push_str(label);
                i += len;
            }
            None => {
                let ch_len = text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                out.push_str(&text[i..i + ch_len]);
                i += ch_len;
            }
        }
    }
    out
}

/// Redact `text` using the values currently in the process environment
/// for the named secrets (the issued values sit in the runtime
/// environment, never in a DS-7 file). Names with no value set are
/// skipped. The caller passes the live issuance names for its seat.
pub fn redact_with_env(text: &str, names: &[String]) -> String {
    let mut owned: Vec<(String, String)> = Vec::new();
    for name in names {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            owned.push((name.clone(), value));
        }
    }
    let refs: Vec<(&str, &str)> = owned
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    redact_text(text, &refs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn temp_store(name: &str) -> (PathBuf, SecretStore) {
        let dir = std::env::temp_dir().join(format!("zen_secret_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.clone(), SecretStore::new(dir.join(".zen")))
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-08T15:30:00+00:00")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn register_openrouter(store: &SecretStore, now: DateTime<Utc>) -> RegistryEntry {
        register_secret(
            store,
            RegisterInput {
                name: "OPENROUTER_KEY".to_string(),
                class: SecretClass::Seat,
                service: "openrouter".to_string(),
                owner: "zc-leader".to_string(),
                registered_by: "zc-leader".to_string(),
                default_until_days: 7,
                rotation_days: 90,
                paid: false,
                reason: "provider key for verification runs".to_string(),
            },
            now,
        )
        .unwrap()
    }

    fn register_user_secret(store: &SecretStore, now: DateTime<Utc>) {
        register_secret(
            store,
            RegisterInput {
                name: "ZEN_PERSONAL_LOGIN".to_string(),
                class: SecretClass::User,
                service: "personal account".to_string(),
                owner: "zen".to_string(),
                registered_by: "zc-leader".to_string(),
                default_until_days: 1,
                rotation_days: 30,
                paid: false,
                reason: "broker-only personal login".to_string(),
            },
            now,
        )
        .unwrap();
    }

    fn request_openrouter(
        store: &SecretStore,
        seat: &str,
        until: DateTime<Utc>,
        one_shot: bool,
    ) -> SecretRequest {
        request_secret(
            store,
            RequestInput {
                name: "OPENROUTER_KEY".to_string(),
                seat: seat.to_string(),
                task_id: Some("DS-7".to_string()),
                purpose: "call the provider for the DS-7 verification runs".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(until),
                one_shot,
                broker_action: None,
            },
            fixed_now(),
        )
        .unwrap()
    }

    fn issue_openrouter(store: &SecretStore, seat: &str, now: DateTime<Utc>) -> SecretRequest {
        let req = request_openrouter(store, seat, now + Duration::days(1), false);
        approve_secret(store, &req.request_id, LEADER_APPROVER, "tin 8071", now).unwrap();
        issue_secret(store, &req.request_id, OPERATOR_ISSUER, now).unwrap()
    }

    fn file_has_shaped_value(store: &SecretStore) -> bool {
        for path in [
            store.registry_path(),
            store.requests_path(),
            store.audit_path(),
        ] {
            if let Ok(data) = std::fs::read_to_string(path)
                && secret_shaped_pattern(&data).is_some()
            {
                return true;
            }
        }
        false
    }

    #[test]
    fn empty_registry_refuses_every_request() {
        let (_dir, store) = temp_store("empty");
        let err = request_secret(
            &store,
            RequestInput {
                name: "OPENROUTER_KEY".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "work".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(fixed_now() + Duration::days(1)),
                one_shot: false,
                broker_action: None,
            },
            fixed_now(),
        )
        .unwrap_err();
        assert_eq!(err.code(), exit_code::VALIDATION);
        assert!(err.to_string().contains("unknown-secret"));
        assert!(load_requests(&store).unwrap().is_empty());
        assert!(load_audit(&store).unwrap().is_empty());
    }

    #[test]
    fn unknown_and_typo_names_fail_closed() {
        let (_dir, store) = temp_store("unknown");
        register_openrouter(&store, fixed_now());
        for bad in [
            "OPENROUTER_KEYS",
            "OPENROUTER",
            "openrouter_key",
            "OPENROUTER-KEY",
        ] {
            let err = request_secret(
                &store,
                RequestInput {
                    name: bad.to_string(),
                    seat: "zd-developer".to_string(),
                    task_id: None,
                    purpose: "work".to_string(),
                    scope: "task:DS-7".to_string(),
                    until: rfc3339(fixed_now() + Duration::days(1)),
                    one_shot: false,
                    broker_action: None,
                },
                fixed_now(),
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("unknown-secret")
                    || err.to_string().contains("invalid secret name"),
                "{bad}: {err}"
            );
        }
        assert!(load_requests(&store).unwrap().is_empty());
    }

    #[test]
    fn secret_shaped_values_refused_in_every_field() {
        let (_dir, store) = temp_store("shaped");
        let now = fixed_now();
        register_openrouter(&store, now);
        // Deliberately fake values, obviously not real keys. Each one
        // matches one of the three writer patterns (40-hex, sk-prefix,
        // KEY-assignment) and must be refused wherever it is tried.
        let shaped = [
            "a".repeat(40),
            // Built at runtime, never as a literal: the value must match
            // the sk-prefix writer pattern, but no token-shaped literal
            // belongs in source (the repo's gitleaks job scans the full
            // history). It is obviously fake: "live-fake" is in it.
            format!("sk-{}-{}", "live-fake", "0123456789abcdef"),
            format!("{}={}", "OPENROUTER_KEY", "fake-value-here"),
        ];
        let mut attempts = 0;
        for value in &shaped {
            // Purpose, scope, and broker action on a request.
            for field in ["purpose", "scope"] {
                let (purpose, scope) = if field == "purpose" {
                    (format!("work with {value}"), "task:DS-7".to_string())
                } else {
                    ("work".to_string(), format!("task:DS-7 {value}"))
                };
                let err = request_secret(
                    &store,
                    RequestInput {
                        name: "OPENROUTER_KEY".to_string(),
                        seat: "zd-developer".to_string(),
                        task_id: None,
                        purpose,
                        scope,
                        until: rfc3339(now + Duration::days(1)),
                        one_shot: false,
                        broker_action: None,
                    },
                    now,
                )
                .unwrap_err();
                assert!(err.to_string().contains("secret-shaped"), "{field}: {err}");
                attempts += 1;
            }
            // Reason on a registration and on a denial; evidence on an
            // approval.
            let err = register_secret(
                &store,
                RegisterInput {
                    name: "OTHER_KEY".to_string(),
                    class: SecretClass::Seat,
                    service: "other".to_string(),
                    owner: "zc-leader".to_string(),
                    registered_by: "zc-leader".to_string(),
                    default_until_days: 7,
                    rotation_days: 90,
                    paid: false,
                    reason: format!("because {value}"),
                },
                now,
            )
            .unwrap_err();
            assert!(err.to_string().contains("secret-shaped"), "register: {err}");
            attempts += 1;
            let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
            let err =
                deny_secret(&store, &req.request_id, LEADER_APPROVER, value, now).unwrap_err();
            assert!(err.to_string().contains("secret-shaped"), "deny: {err}");
            attempts += 1;
            let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
            let err =
                approve_secret(&store, &req.request_id, LEADER_APPROVER, value, now).unwrap_err();
            assert!(err.to_string().contains("secret-shaped"), "approve: {err}");
            attempts += 1;
        }
        // 3 patterns x 5 fields = 15 attempts; pad the count with 5
        // more attempts through the revoke reason path.
        for value in &shaped {
            let req = issue_openrouter(&store, "zd-developer", now);
            let err =
                revoke_secret(&store, &req.request_id, LEADER_APPROVER, value, now).unwrap_err();
            assert!(err.to_string().contains("secret-shaped"), "revoke: {err}");
            attempts += 1;
        }
        for _ in 0..2 {
            let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
            let err = deny_secret(
                &store,
                &req.request_id,
                LEADER_APPROVER,
                &format!(
                    "not needed because sk-{}-{}",
                    "live-fake", "0123456789abcdef"
                ),
                now,
            )
            .unwrap_err();
            assert!(err.to_string().contains("secret-shaped"), "deny2: {err}");
            attempts += 1;
        }
        assert_eq!(attempts, 20);
        // The refused attempts wrote nothing shaped: the three files
        // hold no secret-shaped value at all.
        assert!(!file_has_shaped_value(&store));
        // And no request from a refused attempt exists: every request
        // in the file was filed cleanly.
        for req in load_requests(&store).unwrap() {
            assert!(secret_shaped_pattern(&req.purpose).is_none());
            assert!(secret_shaped_pattern(&req.scope).is_none());
        }
    }

    #[test]
    fn seat_never_approves_its_own_request() {
        let (_dir, store) = temp_store("selfapprove");
        let now = fixed_now();
        register_openrouter(&store, now);
        for i in 0..20 {
            let seat = format!("seat-{i:02}");
            let req = request_secret(
                &store,
                RequestInput {
                    name: "OPENROUTER_KEY".to_string(),
                    seat: seat.clone(),
                    task_id: None,
                    purpose: "work".to_string(),
                    scope: format!("task:T-{i}"),
                    until: rfc3339(now + Duration::days(1)),
                    one_shot: false,
                    broker_action: None,
                },
                now,
            )
            .unwrap();
            let err = approve_secret(&store, &req.request_id, &seat, "tin 1", now).unwrap_err();
            assert_eq!(err.code(), exit_code::FORBIDDEN);
            let after = load_request(&store, &req.request_id).unwrap();
            assert_eq!(after.state, RequestState::Requested);
        }
    }

    #[test]
    fn user_secret_needs_broker_action_and_zen_approval() {
        let (_dir, store) = temp_store("userclass");
        let now = fixed_now();
        register_user_secret(&store, now);
        // Asked as a value (no broker action): refused, class named.
        let err = request_secret(
            &store,
            RequestInput {
                name: "ZEN_PERSONAL_LOGIN".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "log in as Zen".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::hours(12)),
                one_shot: false,
                broker_action: None,
            },
            now,
        )
        .unwrap_err();
        assert!(err.to_string().contains("user-class"), "{err}");
        assert!(load_requests(&store).unwrap().is_empty());
        // With a broker action: the Leader may not approve it.
        let req = request_secret(
            &store,
            RequestInput {
                name: "ZEN_PERSONAL_LOGIN".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "call the endpoint and return only the result".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::hours(12)),
                one_shot: false,
                broker_action: Some(
                    "call endpoint X with Zen's key; return only the result".to_string(),
                ),
            },
            now,
        )
        .unwrap();
        let err =
            approve_secret(&store, &req.request_id, LEADER_APPROVER, "tin 1", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        // Zen approves; issuance is a broker grant, never a value:
        // no Vault entry is recorded.
        let approved = approve_secret(&store, &req.request_id, ZEN_APPROVER, "tin 2", now).unwrap();
        assert_eq!(approved.state, RequestState::Approved);
        let issued = issue_secret(&store, &req.request_id, OPERATOR_ISSUER, now).unwrap();
        assert_eq!(issued.state, RequestState::Issued);
        assert!(issued.vault_entry.is_none());
    }

    #[test]
    fn expiry_refuses_after_until() {
        let (_dir, store) = temp_store("expiry");
        let now = fixed_now();
        register_openrouter(&store, now);
        let mut ids = Vec::new();
        for i in 0..20 {
            let req = request_secret(
                &store,
                RequestInput {
                    name: "OPENROUTER_KEY".to_string(),
                    seat: "zd-developer".to_string(),
                    task_id: None,
                    purpose: "short work".to_string(),
                    scope: format!("task:E-{i}"),
                    until: rfc3339(Utc::now() + Duration::seconds(2)),
                    one_shot: false,
                    broker_action: None,
                },
                Utc::now(),
            )
            .unwrap();
            approve_secret(
                &store,
                &req.request_id,
                LEADER_APPROVER,
                "tin 1",
                Utc::now(),
            )
            .unwrap();
            issue_secret(&store, &req.request_id, OPERATOR_ISSUER, Utc::now()).unwrap();
            ids.push(req.request_id);
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        let later = Utc::now();
        for id in &ids {
            let check = check_use(&store, id, "zd-developer", later).unwrap();
            assert!(!check.allowed, "{id}: {check:?}");
            assert!(check.reason.contains("expired"), "{id}: {}", check.reason);
            let req = load_request(&store, id).unwrap();
            assert_eq!(effective_state(&req, later, false), RequestState::Expired);
        }
    }

    #[test]
    fn revocation_is_immediate() {
        let (_dir, store) = temp_store("revoke");
        let now = fixed_now();
        register_openrouter(&store, now);
        let issued = issue_openrouter(&store, "zd-developer", now);
        let check = check_use(&store, &issued.request_id, "zd-developer", now).unwrap();
        assert!(check.allowed);
        revoke_secret(
            &store,
            &issued.request_id,
            LEADER_APPROVER,
            "leak suspected",
            now,
        )
        .unwrap();
        let check = check_use(&store, &issued.request_id, "zd-developer", now).unwrap();
        assert!(!check.allowed);
        assert!(check.reason.contains("revoked"), "{}", check.reason);
        // A revoked issuance is never re-activated: re-issue is refused.
        let err = issue_secret(&store, &issued.request_id, OPERATOR_ISSUER, now).unwrap_err();
        assert_eq!(err.code(), exit_code::CONFLICT);
    }

    #[test]
    fn one_shot_consumed_by_first_real_use_only() {
        let (_dir, store) = temp_store("oneshot");
        let now = fixed_now();
        register_openrouter(&store, now);
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), true);
        approve_secret(&store, &req.request_id, LEADER_APPROVER, "tin 1", now).unwrap();
        issue_secret(&store, &req.request_id, OPERATOR_ISSUER, now).unwrap();
        // Probes return ALLOW and consume nothing, as often as run.
        for _ in 0..3 {
            let check = check_use(&store, &req.request_id, "zd-developer", now).unwrap();
            assert!(check.allowed);
        }
        let used = record_use(&store, &req.request_id, "zd-developer", now).unwrap();
        assert!(used.consumed);
        assert_eq!(used.state, RequestState::Revoked);
        // A second use is DENY, and a probe after consumption is DENY.
        let check = check_use(&store, &req.request_id, "zd-developer", now).unwrap();
        assert!(!check.allowed);
        let err = record_use(&store, &req.request_id, "zd-developer", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
    }

    #[test]
    fn redaction_covers_verbatim_and_encoded_forms() {
        // Obviously fake values, never real keys.
        let pairs = [
            ("OPENROUTER_KEY", "test-value-alpha-1234"),
            ("OTHER_KEY", "short1"),
        ];
        for i in 0..20 {
            let text = format!("run {i} used test-value-alpha-1234 and short1 and done");
            let redacted = redact_text(&text, &pairs);
            assert!(!redacted.contains("test-value-alpha-1234"), "{redacted}");
            assert!(redacted.contains("[secret:OPENROUTER_KEY]"), "{redacted}");
            // A short value is redacted where it stands alone.
            assert!(redacted.contains("[secret:OTHER_KEY]"), "{redacted}");
        }
        // The JSON-escaped and URL-encoded forms are covered too (the
        // DS-Bot source read for D7 showed its redactor covers them).
        let value = "test value/with?chars";
        let json = format!("{{\"key\": \"{}\"}}", value.replace('"', "\\\""));
        assert!(!redact_text(&json, &[("SOME_KEY", value)]).contains(value));
        let url = format!("https://example.invalid/?q={}", url_encode(value));
        assert!(!redact_text(&url, &[("SOME_KEY", value)]).contains(value));
        // The encoded/split/reversed leak is explicitly NOT covered:
        // a reversed value passes through, and the report must say so.
        let reversed: String = "test-value-alpha-1234".chars().rev().collect();
        assert_eq!(
            redact_text(&reversed, &pairs),
            reversed,
            "reversed values are not redacted by design; see the report"
        );
        // A short value inside an ordinary word is left alone.
        assert_eq!(redact_text("ashort1x word", &pairs), "ashort1x word");
    }

    #[test]
    fn request_without_purpose_scope_or_until_is_refused() {
        let (_dir, store) = temp_store("missingfields");
        let now = fixed_now();
        register_openrouter(&store, now);
        let base = |purpose: &str, scope: &str, until: &str| RequestInput {
            name: "OPENROUTER_KEY".to_string(),
            seat: "zd-developer".to_string(),
            task_id: None,
            purpose: purpose.to_string(),
            scope: scope.to_string(),
            until: until.to_string(),
            one_shot: false,
            broker_action: None,
        };
        let until = rfc3339(now + Duration::days(1));
        for (purpose, scope, until) in [
            ("", "task:DS-7", until.as_str()),
            ("work", "", until.as_str()),
            ("work", "task:DS-7", ""),
            ("work", "task:DS-7", "2026-11-01T00:00:00Z"),
        ] {
            let err = request_secret(&store, base(purpose, scope, until), now).unwrap_err();
            assert_eq!(err.code(), exit_code::VALIDATION);
        }
        assert!(load_requests(&store).unwrap().is_empty());
        assert!(load_audit(&store).unwrap().is_empty());
    }

    #[test]
    fn near_miss_names_do_not_count() {
        let (_dir, store) = temp_store("nearmiss");
        let now = fixed_now();
        register_openrouter(&store, now);
        let issued = issue_openrouter(&store, "zd-developer", now);
        // The check is by request id and seat: a request for the exact
        // name, presented by another seat, is DENY.
        let check = check_use(&store, &issued.request_id, "zc-developer", now).unwrap();
        assert!(!check.allowed);
        assert!(check.reason.contains("wrong seat"), "{}", check.reason);
        // A request for a near-miss name never exists: it was refused
        // at filing (see unknown_and_typo_names_fail_closed), so no
        // approval or issuance can cite it.
        assert!(
            load_requests(&store)
                .unwrap()
                .iter()
                .all(|r| r.name == "OPENROUTER_KEY")
        );
    }

    #[test]
    fn deregistration_revokes_live_issuances() {
        let (_dir, store) = temp_store("deregister");
        let now = fixed_now();
        register_openrouter(&store, now);
        let issued = issue_openrouter(&store, "zd-developer", now);
        deregister_secret(
            &store,
            "OPENROUTER_KEY",
            LEADER_APPROVER,
            "service retired",
            now,
        )
        .unwrap();
        let check = check_use(&store, &issued.request_id, "zd-developer", now).unwrap();
        assert!(!check.allowed);
        assert!(check.reason.contains("deregistered"), "{}", check.reason);
        let req = load_request(&store, &issued.request_id).unwrap();
        assert_eq!(req.state, RequestState::Revoked);
        // New requests under the name are refused.
        let err = request_secret(
            &store,
            RequestInput {
                name: "OPENROUTER_KEY".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "work".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::days(1)),
                one_shot: false,
                broker_action: None,
            },
            now,
        )
        .unwrap_err();
        assert!(err.to_string().contains("deregistered"));
        // No live issuance remains under the name.
        let status = secret_status(&store, Some("OPENROUTER_KEY"), now).unwrap();
        assert!(status[0].live.is_empty());
        assert!(status[0].deregistered);
    }

    #[test]
    fn approver_authority_follows_class_and_paid_flag() {
        let (_dir, store) = temp_store("authority");
        let now = fixed_now();
        register_openrouter(&store, now);
        register_secret(
            &store,
            RegisterInput {
                name: "PAID_MODEL_KEY".to_string(),
                class: SecretClass::Seat,
                service: "paid provider".to_string(),
                owner: "zc-leader".to_string(),
                registered_by: "zc-leader".to_string(),
                default_until_days: 1,
                rotation_days: 30,
                paid: true,
                reason: "unlocks paid use".to_string(),
            },
            now,
        )
        .unwrap();
        // A seat secret approved by Zen is forbidden; by the Leader ok.
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        let err = approve_secret(&store, &req.request_id, ZEN_APPROVER, "tin 1", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        approve_secret(&store, &req.request_id, LEADER_APPROVER, "tin 1", now).unwrap();
        // A paid-use secret approved by the Leader is forbidden; by
        // Zen ok (D1: paid use needs a fresh Zen decision per use).
        let paid = request_secret(
            &store,
            RequestInput {
                name: "PAID_MODEL_KEY".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "one paid run".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::hours(12)),
                one_shot: true,
                broker_action: None,
            },
            now,
        )
        .unwrap();
        let err =
            approve_secret(&store, &paid.request_id, LEADER_APPROVER, "tin 1", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        approve_secret(&store, &paid.request_id, ZEN_APPROVER, "tin 1", now).unwrap();
    }

    #[test]
    fn issue_is_gated_on_approval_and_issuer() {
        let (_dir, store) = temp_store("issuegate");
        let now = fixed_now();
        register_openrouter(&store, now);
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        // Not approved yet: issuance is refused.
        let err = issue_secret(&store, &req.request_id, OPERATOR_ISSUER, now).unwrap_err();
        assert_eq!(err.code(), exit_code::CONFLICT);
        approve_secret(&store, &req.request_id, LEADER_APPROVER, "tin 1", now).unwrap();
        // An unknown issuer is forbidden; the requesting seat cannot
        // issue its own secret.
        let err = issue_secret(&store, &req.request_id, "zd-developer", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        let issued = issue_secret(&store, &req.request_id, OPERATOR_ISSUER, now).unwrap();
        assert_eq!(issued.state, RequestState::Issued);
        // D4: one Vault entry per (secret, seat) pair.
        assert_eq!(
            issued.vault_entry.as_deref(),
            Some("OPENROUTER_KEY:zd-developer")
        );
    }

    #[test]
    fn lifetime_beyond_registry_default_is_refused() {
        let (_dir, store) = temp_store("lifetime");
        let now = fixed_now();
        register_openrouter(&store, now);
        let err = request_secret(
            &store,
            RequestInput {
                name: "OPENROUTER_KEY".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "work".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::days(8)),
                one_shot: false,
                broker_action: None,
            },
            now,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("lifetime-exceeds-default"),
            "{err}"
        );
    }

    #[test]
    fn one_shot_request_beyond_24h_is_refused() {
        let (_dir, store) = temp_store("oneshot24");
        let now = fixed_now();
        register_openrouter(&store, now);
        // A one-shot asked past 24 hours is refused, even though the
        // registry default for the secret is 7 days (F1).
        let err = request_secret(
            &store,
            RequestInput {
                name: "OPENROUTER_KEY".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "one call".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::days(2)),
                one_shot: true,
                broker_action: None,
            },
            now,
        )
        .unwrap_err();
        assert_eq!(err.code(), exit_code::VALIDATION);
        assert!(err.to_string().contains("lifetime-exceeds-24h"), "{err}");
        assert!(load_requests(&store).unwrap().is_empty());
        // Exactly 24 hours is the cap, not past it: allowed.
        let at_cap = request_openrouter(&store, "zd-developer", now + Duration::hours(24), true);
        assert_eq!(at_cap.until, rfc3339(now + Duration::hours(24)));
        // A task-scoped request is unchanged: 7 days still allowed.
        let task = request_openrouter(&store, "zd-developer", now + Duration::days(7), false);
        assert_eq!(task.until, rfc3339(now + Duration::days(7)));
    }

    #[test]
    fn broker_grant_beyond_24h_is_refused() {
        let (_dir, store) = temp_store("broker24");
        let now = fixed_now();
        register_user_secret(&store, now);
        let broker_request = |until: DateTime<Utc>| {
            request_secret(
                &store,
                RequestInput {
                    name: "ZEN_PERSONAL_LOGIN".to_string(),
                    seat: "zd-developer".to_string(),
                    task_id: Some("DS-7".to_string()),
                    purpose: "read the inbox once".to_string(),
                    scope: "task:DS-7".to_string(),
                    until: rfc3339(until),
                    one_shot: false,
                    broker_action: Some("read inbox".to_string()),
                },
                now,
            )
        };
        // Past 24 hours is refused, even though the registry default
        // for this secret is 1 day (F1).
        let err = broker_request(now + Duration::days(2)).unwrap_err();
        assert_eq!(err.code(), exit_code::VALIDATION);
        assert!(err.to_string().contains("lifetime-exceeds-24h"), "{err}");
        // 12 hours is inside the cap: allowed.
        let grant = broker_request(now + Duration::hours(12)).unwrap();
        assert_eq!(grant.until, rfc3339(now + Duration::hours(12)));
    }

    #[test]
    fn rotation_of_broker_grant_caps_at_24h() {
        let (_dir, store) = temp_store("brokerotate");
        let now = fixed_now();
        // A user secret whose registry default is 7 days: without the
        // cap, the rotated grant would live 7 days (the old code did).
        register_secret(
            &store,
            RegisterInput {
                name: "ZEN_PERSONAL_LOGIN".to_string(),
                class: SecretClass::User,
                service: "personal account".to_string(),
                owner: "zen".to_string(),
                registered_by: "zc-leader".to_string(),
                default_until_days: 7,
                rotation_days: 30,
                paid: false,
                reason: "broker-only personal login".to_string(),
            },
            now,
        )
        .unwrap();
        let req = request_secret(
            &store,
            RequestInput {
                name: "ZEN_PERSONAL_LOGIN".to_string(),
                seat: "zd-developer".to_string(),
                task_id: Some("DS-7".to_string()),
                purpose: "read the inbox once".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::hours(12)),
                one_shot: false,
                broker_action: Some("read inbox".to_string()),
            },
            now,
        )
        .unwrap();
        approve_secret(&store, &req.request_id, ZEN_APPROVER, "tin 8071", now).unwrap();
        issue_secret(&store, &req.request_id, OPERATOR_ISSUER, now).unwrap();
        // The owner rotates; the new broker grant lives at most 24
        // hours, not the registry default (F1, via rotate_secret).
        let rotated = rotate_secret(&store, "ZEN_PERSONAL_LOGIN", "zen", "routine", now).unwrap();
        assert_eq!(rotated.until, rfc3339(now + Duration::hours(24)));
        assert_eq!(rotated.broker_action.as_deref(), Some("read inbox"));
        assert_eq!(rotated.rotates.as_deref(), Some(req.request_id.as_str()));
    }

    #[test]
    fn deny_requires_the_approver() {
        let (_dir, store) = temp_store("denyauth");
        let now = fixed_now();
        register_openrouter(&store, now);
        register_user_secret(&store, now);
        // A seat secret is denied by the Leader only: the requesting
        // seat and Zen are both refused (Q2, design section 4.4).
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        let err = deny_secret(&store, &req.request_id, "zd-developer", "no", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        let err = deny_secret(&store, &req.request_id, ZEN_APPROVER, "no", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        let denied = deny_secret(&store, &req.request_id, LEADER_APPROVER, "no", now).unwrap();
        assert_eq!(denied.state, RequestState::Denied);
        // A user secret is denied by Zen only: the Leader is refused.
        let broker = request_secret(
            &store,
            RequestInput {
                name: "ZEN_PERSONAL_LOGIN".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "read the inbox once".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::hours(12)),
                one_shot: false,
                broker_action: Some("read inbox".to_string()),
            },
            now,
        )
        .unwrap();
        let err = deny_secret(&store, &broker.request_id, LEADER_APPROVER, "no", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        let denied = deny_secret(&store, &broker.request_id, ZEN_APPROVER, "no", now).unwrap();
        assert_eq!(denied.state, RequestState::Denied);
    }

    #[test]
    fn revoke_requires_issuer_or_leader() {
        let (_dir, store) = temp_store("revokeauth");
        let now = fixed_now();
        register_openrouter(&store, now);
        // Issued by Z Ops: the requesting seat and Zen are refused.
        let issued = issue_openrouter(&store, "zd-developer", now);
        let err = revoke_secret(&store, &issued.request_id, "zd-developer", "no", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        let err = revoke_secret(&store, &issued.request_id, ZEN_APPROVER, "no", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        let revoked =
            revoke_secret(&store, &issued.request_id, OPERATOR_ISSUER, "no", now).unwrap();
        assert_eq!(revoked.state, RequestState::Revoked);
        // A second issuance is revoked by the Leader (Q2, design
        // sections 4.4 and 4.7).
        let issued = issue_openrouter(&store, "zd-developer", now);
        let revoked =
            revoke_secret(&store, &issued.request_id, LEADER_APPROVER, "no", now).unwrap();
        assert_eq!(revoked.state, RequestState::Revoked);
    }

    fn request_user_broker(store: &SecretStore, seat: &str, now: DateTime<Utc>) -> SecretRequest {
        request_secret(
            store,
            RequestInput {
                name: "ZEN_PERSONAL_LOGIN".to_string(),
                seat: seat.to_string(),
                task_id: None,
                purpose: "read the inbox once".to_string(),
                scope: "task:DS-7".to_string(),
                until: rfc3339(now + Duration::hours(12)),
                one_shot: false,
                broker_action: Some("read inbox".to_string()),
            },
            now,
        )
        .unwrap()
    }

    fn issue_user_broker(store: &SecretStore, seat: &str, now: DateTime<Utc>) -> SecretRequest {
        let req = request_user_broker(store, seat, now);
        approve_secret(store, &req.request_id, ZEN_APPROVER, "tin 1", now).unwrap();
        issue_secret(store, &req.request_id, OPERATOR_ISSUER, now).unwrap()
    }

    #[test]
    fn withdraw_is_the_requesting_seat_own_act() {
        let (_dir, store) = temp_store("withdraw");
        let now = fixed_now();
        register_openrouter(&store, now);
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        // Anyone but the requesting seat is refused: another seat,
        // and the Leader, who denies instead of withdrawing (F1).
        for by in ["zc-developer", LEADER_APPROVER] {
            let err =
                withdraw_secret(&store, &req.request_id, by, "changed my mind", now).unwrap_err();
            assert_eq!(err.code(), exit_code::FORBIDDEN);
        }
        let stored = load_request(&store, &req.request_id).unwrap();
        assert_eq!(stored.state, RequestState::Requested);
        // The requesting seat withdraws its own undecided request.
        let withdrawn = withdraw_secret(
            &store,
            &req.request_id,
            "zd-developer",
            "no longer needed",
            now,
        )
        .unwrap();
        assert_eq!(withdrawn.state, RequestState::Withdrawn);
        assert_eq!(withdrawn.deny_reason.as_deref(), Some("no longer needed"));
        // Withdrawn is terminal: no approve, deny, issue, revoke, or
        // second withdraw afterwards; the seat files a new request.
        assert_eq!(
            approve_secret(&store, &req.request_id, LEADER_APPROVER, "tin 1", now)
                .unwrap_err()
                .code(),
            exit_code::CONFLICT
        );
        assert_eq!(
            deny_secret(&store, &req.request_id, LEADER_APPROVER, "no", now)
                .unwrap_err()
                .code(),
            exit_code::CONFLICT
        );
        assert_eq!(
            issue_secret(&store, &req.request_id, OPERATOR_ISSUER, now)
                .unwrap_err()
                .code(),
            exit_code::CONFLICT
        );
        assert_eq!(
            revoke_secret(&store, &req.request_id, LEADER_APPROVER, "no", now)
                .unwrap_err()
                .code(),
            exit_code::CONFLICT
        );
        assert_eq!(
            withdraw_secret(&store, &req.request_id, "zd-developer", "again", now)
                .unwrap_err()
                .code(),
            exit_code::CONFLICT
        );
        // The check refuses a withdrawn request, and the audit shows
        // who withdrew it.
        let check = check_use(&store, &req.request_id, "zd-developer", now).unwrap();
        assert!(!check.allowed);
        assert!(check.reason.contains("withdrawn"), "{}", check.reason);
        let audit = load_audit(&store).unwrap();
        assert!(audit.iter().any(|e| e.state == RequestState::Withdrawn
            && e.actor == "zd-developer"
            && e.reason == "no longer needed"));
        // Listing by the withdrawn state finds it.
        let listed = list_requests(&store, None, Some(RequestState::Withdrawn), now).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].request_id, req.request_id);
    }

    #[test]
    fn withdraw_only_from_requested() {
        let (_dir, store) = temp_store("withdrawstate");
        let now = fixed_now();
        register_openrouter(&store, now);
        // An approved request cannot be withdrawn: nothing is issued
        // yet, so it lapses at its until; a live issuance is ended by
        // revoke, not by withdraw.
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        approve_secret(&store, &req.request_id, LEADER_APPROVER, "tin 1", now).unwrap();
        let err =
            withdraw_secret(&store, &req.request_id, "zd-developer", "too late", now).unwrap_err();
        assert_eq!(err.code(), exit_code::CONFLICT);
        assert!(err.to_string().contains("approved"), "{}", err);
        let stored = load_request(&store, &req.request_id).unwrap();
        assert_eq!(stored.state, RequestState::Approved);
        // A denied request cannot be withdrawn either.
        let denied = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        deny_secret(&store, &denied.request_id, LEADER_APPROVER, "no", now).unwrap();
        let err = withdraw_secret(&store, &denied.request_id, "zd-developer", "too late", now)
            .unwrap_err();
        assert_eq!(err.code(), exit_code::CONFLICT);
    }

    #[test]
    fn withdraw_refuses_a_secret_shaped_reason() {
        let (_dir, store) = temp_store("withdrawshaped");
        let now = fixed_now();
        register_openrouter(&store, now);
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        // A 40-hex string built at run time, never a literal in the
        // source: the writer refuses it with the pattern named.
        let shaped = format!("leaked {:040x}", 0xdeadbeefu64);
        let err =
            withdraw_secret(&store, &req.request_id, "zd-developer", &shaped, now).unwrap_err();
        assert_eq!(err.code(), exit_code::VALIDATION);
        assert!(err.to_string().contains("40-hex"), "{}", err);
        let stored = load_request(&store, &req.request_id).unwrap();
        assert_eq!(stored.state, RequestState::Requested);
        assert!(!file_has_shaped_value(&store));
    }

    #[test]
    fn zen_revokes_a_user_class_broker_grant() {
        let (_dir, store) = temp_store("zenrevoke");
        let now = fixed_now();
        register_openrouter(&store, now);
        register_user_secret(&store, now);
        // A broker grant issued by Z Ops: Zen revokes it directly
        // (F2, Leader tin 8174). The grant is live before, dead after.
        let grant = issue_user_broker(&store, "zd-developer", now);
        assert_eq!(grant.state, RequestState::Issued);
        assert!(grant.vault_entry.is_none());
        let check = check_use(&store, &grant.request_id, "zd-developer", now).unwrap();
        assert!(check.allowed, "{check:?}");
        let revoked = revoke_secret(
            &store,
            &grant.request_id,
            ZEN_APPROVER,
            "key rotated by Zen",
            now,
        )
        .unwrap();
        assert_eq!(revoked.state, RequestState::Revoked);
        let check = check_use(&store, &grant.request_id, "zd-developer", now).unwrap();
        assert!(!check.allowed);
        assert!(check.reason.contains("revoked"), "{}", check.reason);
        // The issuer and the Leader can still revoke a user grant;
        // any other seat cannot.
        let grant = issue_user_broker(&store, "zd-developer", now);
        let revoked =
            revoke_secret(&store, &grant.request_id, OPERATOR_ISSUER, "done", now).unwrap();
        assert_eq!(revoked.state, RequestState::Revoked);
        let grant = issue_user_broker(&store, "zd-developer", now);
        let revoked =
            revoke_secret(&store, &grant.request_id, LEADER_APPROVER, "done", now).unwrap();
        assert_eq!(revoked.state, RequestState::Revoked);
        let grant = issue_user_broker(&store, "zd-developer", now);
        let err = revoke_secret(&store, &grant.request_id, "zc-developer", "no", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        // Unchanged for a seat secret: Zen still cannot revoke it
        // (revoke_requires_issuer_or_leader pins this).
        let issued = issue_openrouter(&store, "zd-developer", now);
        let err = revoke_secret(&store, &issued.request_id, ZEN_APPROVER, "no", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
    }

    #[test]
    fn register_and_deregister_are_leader_only() {
        let (_dir, store) = temp_store("regauth");
        let now = fixed_now();
        // A seat may propose a registration; only the Leader lands it
        // (Q1, design section 4.3).
        let err = register_secret(
            &store,
            RegisterInput {
                name: "OTHER_KEY".to_string(),
                class: SecretClass::Seat,
                service: "other".to_string(),
                owner: "zc-leader".to_string(),
                registered_by: "zd-developer".to_string(),
                default_until_days: 7,
                rotation_days: 90,
                paid: false,
                reason: "proposed by a seat".to_string(),
            },
            now,
        )
        .unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        assert!(load_registry(&store).unwrap().is_empty());
        register_openrouter(&store, now);
        let err = deregister_secret(
            &store,
            "OPENROUTER_KEY",
            "zd-developer",
            "a seat tries",
            now,
        )
        .unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        deregister_secret(&store, "OPENROUTER_KEY", LEADER_APPROVER, "retired", now).unwrap();
    }

    #[test]
    fn request_ids_are_sequential_per_day() {
        let (_dir, store) = temp_store("ids");
        let now = fixed_now();
        register_openrouter(&store, now);
        let first = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        let second = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        assert_eq!(first.request_id, "sr-20261108-001");
        assert_eq!(second.request_id, "sr-20261108-002");
    }

    #[test]
    fn rotation_keeps_old_value_through_grace_then_replaces_it() {
        let (_dir, store) = temp_store("rotate");
        let now = fixed_now();
        register_openrouter(&store, now);
        // The issuance lives 7 days, so the 24-hour rotation grace -
        // not the issuance until - is what ends the old value.
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(7), false);
        approve_secret(&store, &req.request_id, LEADER_APPROVER, "tin 1", now).unwrap();
        let issued = issue_secret(&store, &req.request_id, OPERATOR_ISSUER, now).unwrap();
        // Only the owner (or the Leader) rotates.
        let err =
            rotate_secret(&store, "OPENROUTER_KEY", "zd-developer", "routine", now).unwrap_err();
        assert_eq!(err.code(), exit_code::FORBIDDEN);
        let rotated =
            rotate_secret(&store, "OPENROUTER_KEY", LEADER_APPROVER, "routine", now).unwrap();
        assert_eq!(rotated.rotates.as_deref(), Some(issued.request_id.as_str()));
        assert_eq!(rotated.state, RequestState::Requested);
        assert!(rotated.purpose.contains(&issued.request_id));
        // During grace the old issuance still works.
        let during = now + Duration::hours(12);
        let check = check_use(&store, &issued.request_id, "zd-developer", during).unwrap();
        assert!(check.allowed, "{check:?}");
        // After grace the old value is dead: state `rotated`, check DENY.
        let after = now + Duration::hours(25);
        let old = load_request(&store, &issued.request_id).unwrap();
        assert_eq!(effective_state(&old, after, false), RequestState::Rotated);
        let check = check_use(&store, &issued.request_id, "zd-developer", after).unwrap();
        assert!(!check.allowed);
        assert!(check.reason.contains("rotated"), "{}", check.reason);
        // The new request can go through the normal flow.
        approve_secret(&store, &rotated.request_id, LEADER_APPROVER, "tin 1", after).unwrap();
        let new_issued = issue_secret(&store, &rotated.request_id, OPERATOR_ISSUER, after).unwrap();
        assert_eq!(new_issued.state, RequestState::Issued);
    }

    #[test]
    fn denied_requests_are_terminal_and_state_moves_forward_only() {
        let (_dir, store) = temp_store("terminal");
        let now = fixed_now();
        register_openrouter(&store, now);
        let req = request_openrouter(&store, "zd-developer", now + Duration::days(1), false);
        deny_secret(&store, &req.request_id, LEADER_APPROVER, "not needed", now).unwrap();
        let err =
            approve_secret(&store, &req.request_id, LEADER_APPROVER, "tin 1", now).unwrap_err();
        assert_eq!(err.code(), exit_code::CONFLICT);
        let err = revoke_secret(&store, &req.request_id, LEADER_APPROVER, "no", now).unwrap_err();
        assert_eq!(err.code(), exit_code::CONFLICT);
    }

    #[test]
    fn budgets_hold_on_100_line_files() {
        let (_dir, store) = temp_store("budgets");
        let now = fixed_now();
        // Seed 100 registry lines and 100 request lines.
        for i in 0..100 {
            register_secret(
                &store,
                RegisterInput {
                    name: format!("SEED_KEY_{i:03}"),
                    class: SecretClass::Seat,
                    service: "seed".to_string(),
                    owner: "zc-leader".to_string(),
                    registered_by: "zc-leader".to_string(),
                    default_until_days: 7,
                    rotation_days: 90,
                    paid: false,
                    reason: "budget seed".to_string(),
                },
                now,
            )
            .unwrap();
        }
        register_openrouter(&store, now);
        for i in 0..100 {
            request_secret(
                &store,
                RequestInput {
                    name: "OPENROUTER_KEY".to_string(),
                    seat: "zd-developer".to_string(),
                    task_id: None,
                    purpose: "seed work".to_string(),
                    scope: format!("task:S-{i}"),
                    until: rfc3339(now + Duration::days(1)),
                    one_shot: false,
                    broker_action: None,
                },
                now,
            )
            .unwrap();
        }
        let start = Instant::now();
        request_secret(
            &store,
            RequestInput {
                name: "OPENROUTER_KEY".to_string(),
                seat: "zd-developer".to_string(),
                task_id: None,
                purpose: "budget probe".to_string(),
                scope: "task:budget".to_string(),
                until: rfc3339(now + Duration::days(1)),
                one_shot: false,
                broker_action: None,
            },
            now,
        )
        .unwrap();
        let request_elapsed = start.elapsed();
        eprintln!("budget: request on 100-line files took {request_elapsed:?}");
        assert!(
            request_elapsed.as_millis() < 10,
            "request took {request_elapsed:?}, budget is under 10 ms on 100-line files"
        );
        let start = Instant::now();
        let listed = list_requests(&store, None, None, now).unwrap();
        assert_eq!(listed.len(), 101);
        let list_elapsed = start.elapsed();
        eprintln!("budget: list on 100-line files took {list_elapsed:?}");
        assert!(
            list_elapsed.as_millis() < 50,
            "list took {list_elapsed:?}, budget is under 50 ms"
        );
        let start = Instant::now();
        let status = secret_status(&store, None, now).unwrap();
        assert_eq!(status.len(), 101);
        let status_elapsed = start.elapsed();
        eprintln!("budget: status on 100-line files took {status_elapsed:?}");
        assert!(
            status_elapsed.as_millis() < 50,
            "status took {status_elapsed:?}, budget is under 50 ms"
        );
    }
}
