//! DS-7 `zen secret` commands.
//!
//! A thin wrapper over `zen_core::secret`: argument parsing and output
//! formatting only. All rules live in the engine - the request
//! lifecycle, the approver split, the writer refusal for secret-shaped
//! values, expiry, revocation, rotation, and redaction. No command in
//! this file ever prints a secret value, because no value ever passes
//! through it: `issue` records the human act of placing the value, it
//! does not carry the value.

use std::path::Path;

use anyhow::Result;
use chrono::Utc;

use crate::cli::{OutputFormat, SecretArgs, SecretSubcommand};
use crate::{coded_error, exit_code};
use zen_core::secret::{
    CheckOutcome, RegisterInput, RegistryEntry, RequestInput, RequestState, SecretClass,
    SecretRequest, SecretStatus, SecretStore, UseOutcome, approve_secret, check_use, deny_secret,
    deregister_secret, issue_secret, list_requests, record_use, register_secret, request_secret,
    revoke_secret, rotate_secret, secret_status, withdraw_secret,
};

fn print_json(value: &serde_json::Value, raw: bool) -> Result<()> {
    if raw {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}

fn render_request_text(req: &SecretRequest) -> String {
    let mut line = format!(
        "{} {} {} {} until {}",
        req.request_id,
        req.name,
        req.seat,
        req.state.as_str(),
        req.until
    );
    if let Some(vault) = &req.vault_entry {
        line.push_str(&format!(" vault-entry {vault}"));
    }
    if let Some(rotates) = &req.rotates {
        line.push_str(&format!(" rotates {rotates}"));
    }
    line
}

fn render_status_text(status: &SecretStatus) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} registered={} class={} deregistered={} live={}",
        status.name,
        status.registered,
        status.class.map(|c| c.as_str()).unwrap_or("unregistered"),
        status.deregistered,
        status.live.len()
    ));
    if let Some(due) = &status.next_rotation_due {
        out.push_str(&format!(" next-rotation-due {due}"));
    }
    out.push('\n');
    for live in &status.live {
        out.push_str(&format!(
            "  {} {} {} until {}\n",
            live.request_id,
            live.seat,
            live.state.as_str(),
            live.until
        ));
    }
    out
}

fn render_entry_text(entry: &RegistryEntry) -> String {
    format!(
        "{} class={} service={} owner={} registered-by={} default-until-days={} rotation-days={} paid={} deregistered={}",
        entry.name,
        entry.class.as_str(),
        entry.service,
        entry.owner,
        entry.registered_by,
        entry.default_until_days,
        entry.rotation_days,
        entry.paid,
        entry.deregistered
    )
}

/// The seat filing a request: `--seat`, or the `ZEN_SEAT` environment
/// variable. There is no default seat: a request filed under the wrong
/// seat is worse than a request refused for naming none.
fn request_seat(flag: Option<&str>) -> Result<String> {
    if let Some(seat) = flag.filter(|s| !s.trim().is_empty()) {
        return Ok(seat.to_string());
    }
    if let Ok(seat) = std::env::var("ZEN_SEAT")
        && !seat.trim().is_empty()
    {
        return Ok(seat);
    }
    Err(coded_error(
        exit_code::VALIDATION,
        "no seat named: pass --seat, or set ZEN_SEAT",
    ))
}

fn map_err(e: zen_core::secret::SecretError) -> anyhow::Error {
    coded_error(e.code(), e.to_string())
}

/// Run one `zen secret` subcommand. `--quiet` silences stdout only:
/// the record is still written, errors still surface, and the exit
/// code never changes.
pub fn run_secret(
    args: &SecretArgs,
    format: &OutputFormat,
    quiet: bool,
    raw: bool,
    home: Option<&Path>,
) -> Result<i32> {
    let Some(home) = home else {
        anyhow::bail!("cannot determine home directory");
    };
    let store = SecretStore::new(home.join(".zen"));
    let now = Utc::now();
    match &args.command {
        SecretSubcommand::Request {
            name,
            purpose,
            scope,
            until,
            task,
            one_shot,
            broker_action,
            seat,
        } => {
            let seat = request_seat(seat.as_deref())?;
            let req = request_secret(
                &store,
                RequestInput {
                    name: name.clone(),
                    seat,
                    task_id: task.clone(),
                    purpose: purpose.clone(),
                    scope: scope.clone(),
                    until: until.clone(),
                    one_shot: *one_shot,
                    broker_action: broker_action.clone(),
                },
                now,
            )
            .map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&req)?, raw)?
                    }
                    OutputFormat::Text => println!("{}", render_request_text(&req)),
                }
            }
            Ok(0)
        }
        SecretSubcommand::List { seat, state } => {
            let state = match state {
                Some(s) => Some(RequestState::parse(s).ok_or_else(|| {
                    coded_error(
                        exit_code::VALIDATION,
                        format!(
                            "invalid state '{s}': expected one of requested, approved, denied, issued, active, expired, revoked, rotated, withdrawn"
                        ),
                    )
                })?),
                None => None,
            };
            let requests = list_requests(&store, seat.as_deref(), state, now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::json!({ "requests": requests }), raw)?
                    }
                    OutputFormat::Text => {
                        for req in &requests {
                            println!("{}", render_request_text(req));
                        }
                    }
                }
            }
            Ok(0)
        }
        SecretSubcommand::Approve {
            request,
            by,
            evidence,
        } => {
            let req = approve_secret(&store, request, by, evidence, now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&req)?, raw)?
                    }
                    OutputFormat::Text => {
                        println!("approved {}", render_request_text(&req));
                    }
                }
            }
            Ok(0)
        }
        SecretSubcommand::Deny {
            request,
            by,
            reason,
        } => {
            let req = deny_secret(&store, request, by, reason, now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&req)?, raw)?
                    }
                    OutputFormat::Text => println!("denied {}", render_request_text(&req)),
                }
            }
            Ok(0)
        }
        SecretSubcommand::Withdraw {
            request,
            by,
            reason,
        } => {
            let req = withdraw_secret(&store, request, by, reason, now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&req)?, raw)?
                    }
                    OutputFormat::Text => println!("withdrawn {}", render_request_text(&req)),
                }
            }
            Ok(0)
        }
        SecretSubcommand::Issue { request, by } => {
            let req = issue_secret(&store, request, by, now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&req)?, raw)?
                    }
                    OutputFormat::Text => println!("issued {}", render_request_text(&req)),
                }
            }
            Ok(0)
        }
        SecretSubcommand::Revoke {
            request,
            by,
            reason,
        } => {
            let req = revoke_secret(&store, request, by, reason, now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&req)?, raw)?
                    }
                    OutputFormat::Text => println!("revoked {}", render_request_text(&req)),
                }
            }
            Ok(0)
        }
        SecretSubcommand::Rotate { name, by, reason } => {
            let req = rotate_secret(&store, name, by, reason, now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&req)?, raw)?
                    }
                    OutputFormat::Text => {
                        println!("rotation {}", render_request_text(&req));
                    }
                }
            }
            Ok(0)
        }
        SecretSubcommand::Register {
            name,
            class,
            service,
            owner,
            by,
            default_until_days,
            rotation_days,
            paid,
            reason,
        } => {
            let class = SecretClass::parse(class).ok_or_else(|| {
                coded_error(
                    exit_code::VALIDATION,
                    format!("invalid class '{class}': expected seat or user"),
                )
            })?;
            let entry = register_secret(
                &store,
                RegisterInput {
                    name: name.clone(),
                    class,
                    service: service.clone(),
                    owner: owner.clone(),
                    registered_by: by.clone(),
                    default_until_days: *default_until_days,
                    rotation_days: *rotation_days,
                    paid: *paid,
                    reason: reason.clone(),
                },
                now,
            )
            .map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&entry)?, raw)?
                    }
                    OutputFormat::Text => println!("registered {}", render_entry_text(&entry)),
                }
            }
            Ok(0)
        }
        SecretSubcommand::Deregister { name, by, reason } => {
            let entry = deregister_secret(&store, name, by, reason, now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::to_value(&entry)?, raw)?
                    }
                    OutputFormat::Text => println!("deregistered {}", render_entry_text(&entry)),
                }
            }
            Ok(0)
        }
        SecretSubcommand::Status { name } => {
            let status = secret_status(&store, name.as_deref(), now).map_err(map_err)?;
            if !quiet {
                match format {
                    OutputFormat::Json | OutputFormat::StreamJson => {
                        print_json(&serde_json::json!({ "secrets": status }), raw)?
                    }
                    OutputFormat::Text => {
                        for s in &status {
                            print!("{}", render_status_text(s));
                        }
                    }
                }
            }
            Ok(0)
        }
    }
}

/// The check that gates a use, exposed for the engine's callers (the
/// DS-5 cross-check seam): a probe that never consumes a one-shot
/// issuance and never writes.
pub fn check_secret_use(
    store: &SecretStore,
    request_id: &str,
    seat: &str,
    now: chrono::DateTime<Utc>,
) -> Result<CheckOutcome> {
    check_use(store, request_id, seat, now).map_err(map_err)
}

/// Record a real use of an issued secret: the first use moves `issued`
/// to `active`, and consumes a one-shot issuance.
pub fn use_secret(
    store: &SecretStore,
    request_id: &str,
    seat: &str,
    now: chrono::DateTime<Utc>,
) -> Result<UseOutcome> {
    record_use(store, request_id, seat, now).map_err(map_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use zen_core::secret::{
        RegisterInput, SecretClass, SecretStore as Store, load_requests, register_secret,
    };

    fn temp_home(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zen_secret_cli_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seed_registry(home: &Path) {
        let store = Store::new(home.join(".zen"));
        register_secret(
            &store,
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
            Utc::now(),
        )
        .unwrap();
    }

    fn seed_user_registry(home: &Path) {
        let store = Store::new(home.join(".zen"));
        register_secret(
            &store,
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
            Utc::now(),
        )
        .unwrap();
    }

    fn request_args(until: &str) -> SecretArgs {
        SecretArgs {
            command: SecretSubcommand::Request {
                name: "OPENROUTER_KEY".to_string(),
                purpose: "verification runs".to_string(),
                scope: "task:DS-7".to_string(),
                until: until.to_string(),
                task: Some("DS-7".to_string()),
                one_shot: false,
                broker_action: None,
                seat: Some("zd-developer".to_string()),
            },
        }
    }

    #[test]
    fn secret_request_list_status_round_trip() {
        let home = temp_home("roundtrip");
        seed_registry(&home);
        let until =
            (Utc::now() + Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let code = run_secret(
            &request_args(&until),
            &OutputFormat::Text,
            true,
            false,
            Some(&home),
        )
        .unwrap();
        assert_eq!(code, 0);
        let requests = load_requests(&Store::new(home.join(".zen"))).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].request_id, requests[0].request_id);
        assert!(requests[0].request_id.starts_with("sr-"));
        // Unknown names fail closed through the CLI too, exit 4.
        let bad = SecretArgs {
            command: SecretSubcommand::Request {
                name: "NOPE_KEY".to_string(),
                purpose: "work".to_string(),
                scope: "task:DS-7".to_string(),
                until,
                task: None,
                one_shot: false,
                broker_action: None,
                seat: Some("zd-developer".to_string()),
            },
        };
        let err = run_secret(&bad, &OutputFormat::Text, true, false, Some(&home)).unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::VALIDATION);
    }

    #[test]
    fn secret_request_needs_a_seat() {
        let home = temp_home("noseat");
        seed_registry(&home);
        unsafe { std::env::remove_var("ZEN_SEAT") };
        let args = SecretArgs {
            command: SecretSubcommand::Request {
                name: "OPENROUTER_KEY".to_string(),
                purpose: "work".to_string(),
                scope: "task:DS-7".to_string(),
                until: (Utc::now() + Duration::days(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                task: None,
                one_shot: false,
                broker_action: None,
                seat: None,
            },
        };
        let err = run_secret(&args, &OutputFormat::Text, true, false, Some(&home)).unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::VALIDATION);
    }

    #[test]
    fn secret_full_lifecycle_through_cli() {
        let home = temp_home("lifecycle");
        seed_registry(&home);
        let until =
            (Utc::now() + Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        run_secret(
            &request_args(&until),
            &OutputFormat::Text,
            true,
            false,
            Some(&home),
        )
        .unwrap();
        let requests = load_requests(&Store::new(home.join(".zen"))).unwrap();
        let id = requests[0].request_id.clone();
        // Self-approval is forbidden, exit 5.
        let self_approve = SecretArgs {
            command: SecretSubcommand::Approve {
                request: id.clone(),
                by: "zd-developer".to_string(),
                evidence: "tin 1".to_string(),
            },
        };
        let err =
            run_secret(&self_approve, &OutputFormat::Text, true, false, Some(&home)).unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::FORBIDDEN);
        // Leader approves, Z Ops issues, then revoke.
        let approve = SecretArgs {
            command: SecretSubcommand::Approve {
                request: id.clone(),
                by: "zc-leader".to_string(),
                evidence: "tin 8071".to_string(),
            },
        };
        assert_eq!(
            run_secret(&approve, &OutputFormat::Text, true, false, Some(&home)).unwrap(),
            0
        );
        let issue = SecretArgs {
            command: SecretSubcommand::Issue {
                request: id.clone(),
                by: "z-ops".to_string(),
            },
        };
        assert_eq!(
            run_secret(&issue, &OutputFormat::Text, true, false, Some(&home)).unwrap(),
            0
        );
        let status = SecretArgs {
            command: SecretSubcommand::Status {
                name: Some("OPENROUTER_KEY".to_string()),
            },
        };
        assert_eq!(
            run_secret(&status, &OutputFormat::Json, true, false, Some(&home)).unwrap(),
            0
        );
        let revoke = SecretArgs {
            command: SecretSubcommand::Revoke {
                request: id.clone(),
                by: "zc-leader".to_string(),
                reason: "no longer needed".to_string(),
            },
        };
        assert_eq!(
            run_secret(&revoke, &OutputFormat::Text, true, false, Some(&home)).unwrap(),
            0
        );
        let requests = load_requests(&Store::new(home.join(".zen"))).unwrap();
        assert_eq!(requests[0].state, RequestState::Revoked);
    }

    fn register_args(by: &str) -> SecretArgs {
        SecretArgs {
            command: SecretSubcommand::Register {
                name: "OPENROUTER_KEY".to_string(),
                class: "seat".to_string(),
                service: "openrouter".to_string(),
                owner: "zc-leader".to_string(),
                by: by.to_string(),
                default_until_days: 7,
                rotation_days: 90,
                paid: false,
                reason: "tin 8071".to_string(),
            },
        }
    }

    #[test]
    fn secret_register_deregister_round_trip() {
        let home = temp_home("regcli");
        assert_eq!(
            run_secret(
                &register_args("zc-leader"),
                &OutputFormat::Text,
                true,
                false,
                Some(&home)
            )
            .unwrap(),
            0
        );
        // The registry now holds the name: a request files cleanly.
        let until =
            (Utc::now() + Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        run_secret(
            &request_args(&until),
            &OutputFormat::Text,
            true,
            false,
            Some(&home),
        )
        .unwrap();
        let deregister = SecretArgs {
            command: SecretSubcommand::Deregister {
                name: "OPENROUTER_KEY".to_string(),
                by: "zc-leader".to_string(),
                reason: "service retired".to_string(),
            },
        };
        assert_eq!(
            run_secret(&deregister, &OutputFormat::Text, true, false, Some(&home)).unwrap(),
            0
        );
        let entries = zen_core::secret::load_registry(&Store::new(home.join(".zen"))).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].deregistered);
        // A new request under the deregistered name is refused.
        let err = run_secret(
            &request_args(&until),
            &OutputFormat::Text,
            true,
            false,
            Some(&home),
        )
        .unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::VALIDATION);
    }

    #[test]
    fn secret_register_is_leader_only_through_cli() {
        let home = temp_home("regauth");
        let err = run_secret(
            &register_args("zd-developer"),
            &OutputFormat::Text,
            true,
            false,
            Some(&home),
        )
        .unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::FORBIDDEN);
        // An unknown class is a validation error, not a registration.
        let bad_class = SecretArgs {
            command: SecretSubcommand::Register {
                name: "OPENROUTER_KEY".to_string(),
                class: "shared".to_string(),
                service: "openrouter".to_string(),
                owner: "zc-leader".to_string(),
                by: "zc-leader".to_string(),
                default_until_days: 7,
                rotation_days: 90,
                paid: false,
                reason: "tin 8071".to_string(),
            },
        };
        let err =
            run_secret(&bad_class, &OutputFormat::Text, true, false, Some(&home)).unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::VALIDATION);
    }

    #[test]
    fn secret_withdraw_round_trip_through_cli() {
        let home = temp_home("withdrawcli");
        seed_registry(&home);
        let until =
            (Utc::now() + Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        run_secret(
            &request_args(&until),
            &OutputFormat::Text,
            true,
            false,
            Some(&home),
        )
        .unwrap();
        let requests = load_requests(&Store::new(home.join(".zen"))).unwrap();
        let id = requests[0].request_id.clone();
        // Another seat cannot withdraw it, exit 5.
        let other = SecretArgs {
            command: SecretSubcommand::Withdraw {
                request: id.clone(),
                by: "zc-developer".to_string(),
                reason: "not mine".to_string(),
            },
        };
        let err = run_secret(&other, &OutputFormat::Text, true, false, Some(&home)).unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::FORBIDDEN);
        // The requesting seat withdraws it, exit 0.
        let withdraw = SecretArgs {
            command: SecretSubcommand::Withdraw {
                request: id.clone(),
                by: "zd-developer".to_string(),
                reason: "no longer needed".to_string(),
            },
        };
        assert_eq!(
            run_secret(&withdraw, &OutputFormat::Text, true, false, Some(&home)).unwrap(),
            0
        );
        let requests = load_requests(&Store::new(home.join(".zen"))).unwrap();
        assert_eq!(requests[0].state, RequestState::Withdrawn);
        // Approving a withdrawn request is a conflict, exit 7.
        let approve = SecretArgs {
            command: SecretSubcommand::Approve {
                request: id.clone(),
                by: "zc-leader".to_string(),
                evidence: "tin 1".to_string(),
            },
        };
        let err = run_secret(&approve, &OutputFormat::Text, true, false, Some(&home)).unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::CONFLICT);
    }

    fn user_request_args(until: &str) -> SecretArgs {
        SecretArgs {
            command: SecretSubcommand::Request {
                name: "ZEN_PERSONAL_LOGIN".to_string(),
                purpose: "read the inbox once".to_string(),
                scope: "task:DS-7".to_string(),
                until: until.to_string(),
                task: None,
                one_shot: false,
                broker_action: Some("read inbox".to_string()),
                seat: Some("zd-developer".to_string()),
            },
        }
    }

    #[test]
    fn secret_zen_revokes_user_broker_grant_through_cli() {
        let home = temp_home("zenrevokecli");
        seed_registry(&home);
        seed_user_registry(&home);
        // A user-class broker grant: Zen approves, Z Ops issues, Zen
        // revokes it directly (F2).
        let until =
            (Utc::now() + Duration::hours(12)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        run_secret(
            &user_request_args(&until),
            &OutputFormat::Text,
            true,
            false,
            Some(&home),
        )
        .unwrap();
        let requests = load_requests(&Store::new(home.join(".zen"))).unwrap();
        let id = requests[0].request_id.clone();
        let approve = SecretArgs {
            command: SecretSubcommand::Approve {
                request: id.clone(),
                by: "zen".to_string(),
                evidence: "tin 1".to_string(),
            },
        };
        assert_eq!(
            run_secret(&approve, &OutputFormat::Text, true, false, Some(&home)).unwrap(),
            0
        );
        let issue = SecretArgs {
            command: SecretSubcommand::Issue {
                request: id.clone(),
                by: "z-ops".to_string(),
            },
        };
        assert_eq!(
            run_secret(&issue, &OutputFormat::Text, true, false, Some(&home)).unwrap(),
            0
        );
        let revoke = SecretArgs {
            command: SecretSubcommand::Revoke {
                request: id.clone(),
                by: "zen".to_string(),
                reason: "key rotated by Zen".to_string(),
            },
        };
        assert_eq!(
            run_secret(&revoke, &OutputFormat::Text, true, false, Some(&home)).unwrap(),
            0
        );
        let requests = load_requests(&Store::new(home.join(".zen"))).unwrap();
        assert_eq!(requests[0].state, RequestState::Revoked);
        // Unchanged for a seat secret: Zen cannot revoke it, exit 5.
        let seat_until =
            (Utc::now() + Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        run_secret(
            &request_args(&seat_until),
            &OutputFormat::Text,
            true,
            false,
            Some(&home),
        )
        .unwrap();
        let requests = load_requests(&Store::new(home.join(".zen"))).unwrap();
        let seat_id = requests
            .iter()
            .find(|r| r.name == "OPENROUTER_KEY")
            .unwrap()
            .request_id
            .clone();
        let approve = SecretArgs {
            command: SecretSubcommand::Approve {
                request: seat_id.clone(),
                by: "zc-leader".to_string(),
                evidence: "tin 1".to_string(),
            },
        };
        run_secret(&approve, &OutputFormat::Text, true, false, Some(&home)).unwrap();
        let issue = SecretArgs {
            command: SecretSubcommand::Issue {
                request: seat_id.clone(),
                by: "z-ops".to_string(),
            },
        };
        run_secret(&issue, &OutputFormat::Text, true, false, Some(&home)).unwrap();
        let revoke = SecretArgs {
            command: SecretSubcommand::Revoke {
                request: seat_id.clone(),
                by: "zen".to_string(),
                reason: "no".to_string(),
            },
        };
        let err = run_secret(&revoke, &OutputFormat::Text, true, false, Some(&home)).unwrap_err();
        assert_eq!(crate::exit_code_for_error(&err), exit_code::FORBIDDEN);
    }
}
