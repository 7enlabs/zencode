#!/usr/bin/env bash
# Container entrypoint: register one ephemeral runner, run one job, exit.
#
# The registration token is read from stdin (first line), never from the
# environment, so it does not show up in /proc/<pid>/environ of this
# process or of any job step. It is kept only in a shell variable and
# dropped after `config.sh` returns.
set -euo pipefail

: "${RUNNER_HOME:=/home/runner/actions-runner}"
: "${RUNNER_URL:?RUNNER_URL is required (e.g. https://github.com/7enlabs)}"
: "${RUNNER_NAME:?RUNNER_NAME is required}"
: "${RUNNER_LABELS:=zc-self}"
: "${RUNNER_GROUP:=}"
: "${ZC_RUNNER_DRY_RUN:=0}"

log() { printf '[zc-runner] %s\n' "$*" >&2; }

cd "$RUNNER_HOME"

reg_token=""
IFS= read -r reg_token || true
if [[ -z "$reg_token" ]]; then
  log "no registration token on stdin; refusing to start"
  exit 64
fi
if [[ ! "$reg_token" =~ ^[A-Za-z0-9_-]{8,256}$ ]]; then
  log "registration token has an unexpected shape (whitespace or odd characters); refusing to start"
  reg_token=""
  exit 65
fi

config_args=(
  --unattended
  --ephemeral
  --disableupdate
  --replace
  --url "$RUNNER_URL"
  --name "$RUNNER_NAME"
  --labels "$RUNNER_LABELS"
  --work _work
)
if [[ -n "$RUNNER_GROUP" ]]; then
  config_args+=(--runnergroup "$RUNNER_GROUP")
fi

if [[ "$ZC_RUNNER_DRY_RUN" == "1" ]]; then
  log "dry-run: runner version $(./config.sh --version)"
  log "dry-run: would run ./config.sh ${config_args[*]} --token <redacted>"
  log "dry-run: would then run ./run.sh (one job) and exit"
  reg_token=""
  exit 0
fi

runner_pid=""
# shellcheck disable=SC2329  # invoked via trap
cleanup() {
  local rc=$?
  if [[ -n "$runner_pid" ]] && kill -0 "$runner_pid" 2>/dev/null; then
    log "stopping runner (pid $runner_pid)"
    kill -TERM "$runner_pid" 2>/dev/null || true
    wait "$runner_pid" 2>/dev/null || true
  fi
  # An ephemeral runner deregisters itself after its job. If we stop before
  # a job ran, the registration stays (offline) until GitHub expires it.
  if [[ -f .runner ]]; then
    log "runner still registered (no job ran); GitHub removes stale ephemeral runners, or remove it in Settings > Actions > Runners"
  fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 143' TERM INT

log "registering $RUNNER_NAME at $RUNNER_URL labels=$RUNNER_LABELS"
./config.sh "${config_args[@]}" --token "$reg_token" >/dev/null
reg_token=""

log "waiting for one job"
# Run in background so the TERM trap can forward the signal.
./run.sh &
runner_pid=$!
set +e
wait "$runner_pid"
rc=$?
set -e
runner_pid=""
log "runner exited with code $rc"
exit "$rc"
