#!/usr/bin/env bash
# Host launcher: start one throwaway container that registers an ephemeral
# GitHub Actions runner, takes exactly one job, then exits and is removed.
#
# Usage:
#   RUNNER_REG_TOKEN=... ci/runner/run-ephemeral.sh [--build] [--loop] [--dry-run]
#
# Environment:
#   RUNNER_REG_TOKEN  registration token (required unless --dry-run). Read
#                     from env, passed to the container on stdin, never
#                     written to disk or to the container environment.
#                     Org/repo tokens expire after 1 hour; with --loop you
#                     need a fresh token per job (see RUNNER_TOKEN_CMD).
#   RUNNER_TOKEN_CMD  optional command that prints a fresh registration token
#                     on stdout; used instead of RUNNER_REG_TOKEN when set.
#   RUNNER_URL        org or repo URL (default https://github.com/7enlabs)
#   RUNNER_LABELS     comma list (default zc-self)
#   RUNNER_GROUP      runner group (org only; default unset)
#   RUNNER_IMAGE      image tag (default zc-runner:local)
#   RUNNER_CPUS       docker --cpus (default 3)
#   RUNNER_MEMORY     docker --memory (default 6g)
#   RUNNER_MAX_SECONDS  wall clock cap per container (default 21600 = 6h)
#   RUNNER_CACHE_VOLUME optional named volume mounted at /home/runner/.cache-shared
#                       (point CARGO_HOME / sccache there in the workflow)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

RUNNER_URL="${RUNNER_URL:-https://github.com/7enlabs}"
RUNNER_LABELS="${RUNNER_LABELS:-zc-self}"
RUNNER_GROUP="${RUNNER_GROUP:-}"
RUNNER_IMAGE="${RUNNER_IMAGE:-zc-runner:local}"
RUNNER_CPUS="${RUNNER_CPUS:-3}"
RUNNER_MEMORY="${RUNNER_MEMORY:-6g}"
RUNNER_MAX_SECONDS="${RUNNER_MAX_SECONDS:-21600}"
RUNNER_CACHE_VOLUME="${RUNNER_CACHE_VOLUME:-}"
RUNNER_TOKEN_CMD="${RUNNER_TOKEN_CMD:-}"
# Keep the token in a non-exported variable so child processes (docker CLI)
# do not inherit it.
reg_token_static="${RUNNER_REG_TOKEN:-}"
unset RUNNER_REG_TOKEN

dry_run=0
build=0
loop=0

usage() { sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
log() { printf '[run-ephemeral] %s\n' "$*" >&2; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) dry_run=1 ;;
    --build) build=1 ;;
    --loop) loop=1 ;;
    -h|--help) usage; exit 0 ;;
    *) log "unknown argument: $1"; usage; exit 64 ;;
  esac
  shift
done

case "$RUNNER_URL" in
  https://github.com/*) ;;
  *) log "RUNNER_URL must start with https://github.com/"; exit 64 ;;
esac
if [[ ! "$RUNNER_LABELS" =~ ^[A-Za-z0-9._,-]+$ ]]; then
  log "RUNNER_LABELS has invalid characters"; exit 64
fi
if [[ ! "$RUNNER_MAX_SECONDS" =~ ^[0-9]+$ ]]; then
  log "RUNNER_MAX_SECONDS must be an integer"; exit 64
fi

command -v docker >/dev/null || { log "docker not found"; exit 69; }

if [[ "$build" == "1" ]] || ! docker image inspect "$RUNNER_IMAGE" >/dev/null 2>&1; then
  log "building $RUNNER_IMAGE"
  docker build -t "$RUNNER_IMAGE" "$here"
fi

get_token() {
  if [[ -n "$RUNNER_TOKEN_CMD" ]]; then
    bash -c "$RUNNER_TOKEN_CMD"
  elif [[ "$dry_run" == "1" ]]; then
    printf 'DRYRUN-PLACEHOLDER-TOKEN\n'
  elif [[ -n "$reg_token_static" ]]; then
    printf '%s\n' "$reg_token_static"
  else
    log "RUNNER_REG_TOKEN (or RUNNER_TOKEN_CMD) is required"; return 65
  fi
}

run_once() {
  local name token rc
  name="zc-$(hostname -s)-$(date -u +%Y%m%d%H%M%S)-$$"
  local args=(
    run --rm -i
    --name "$name"
    --init=false
    --cpus "$RUNNER_CPUS"
    --memory "$RUNNER_MEMORY"
    --pids-limit 4096
    --security-opt no-new-privileges
    --cap-drop ALL
    -e "RUNNER_URL=$RUNNER_URL"
    -e "RUNNER_NAME=$name"
    -e "RUNNER_LABELS=$RUNNER_LABELS"
    -e "RUNNER_GROUP=$RUNNER_GROUP"
    -e "ZC_RUNNER_DRY_RUN=$dry_run"
  )
  if [[ -n "$RUNNER_CACHE_VOLUME" ]]; then
    args+=(-v "$RUNNER_CACHE_VOLUME:/home/runner/.cache-shared")
  fi
  args+=("$RUNNER_IMAGE")

  log "starting $name (dry_run=$dry_run, cap ${RUNNER_MAX_SECONDS}s)"
  token="$(get_token)" || return $?
  if [[ -z "$token" ]]; then
    log "token command returned nothing"; return 65
  fi
  if [[ ! "$token" =~ ^[A-Za-z0-9_-]{8,256}$ ]]; then
    log "token has an unexpected shape (whitespace or odd characters)"; token=""; return 66
  fi
  set +e
  printf '%s\n' "$token" | timeout --signal=TERM --kill-after=60 "$RUNNER_MAX_SECONDS" docker "${args[@]}"
  rc=$?
  set -e
  token=""
  # Belt and braces: --rm removes it, but a killed client can leave it behind.
  docker rm -f "$name" >/dev/null 2>&1 || true
  log "$name finished rc=$rc"
  return "$rc"
}

if [[ "$loop" == "0" ]]; then
  run_once
  exit $?
fi

if [[ -z "$RUNNER_TOKEN_CMD" && "$dry_run" == "0" ]]; then
  log "warning: --loop with a static RUNNER_REG_TOKEN stops working when it expires (1h); set RUNNER_TOKEN_CMD"
fi
failures=0
while true; do
  if run_once; then
    failures=0
  else
    failures=$((failures + 1))
    if [[ "$failures" -ge 5 ]]; then
      log "5 failures in a row, giving up"; exit 1
    fi
    sleep $((failures * 30))
  fi
  [[ "$dry_run" == "1" ]] && break
done
