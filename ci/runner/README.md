# Ephemeral self-hosted runner (ZenCode CI)

One container registers one GitHub Actions runner with `--ephemeral`, takes
exactly one job, then exits and is removed. Nothing from a job survives into
the next one unless you mount a cache volume on purpose.

GitHub-hosted runners stay the default. This runner is an extra pool for the
private `7enlabs` repos, used when the box is awake.

## Files

| File | Role |
|---|---|
| `Dockerfile` | Ubuntu 24.04 + pinned `actions/runner` (SHA256 checked), non-root user `runner`, `tini` as PID 1 |
| `entrypoint.sh` | Inside the container: reads the token from stdin, `config.sh --ephemeral --disableupdate`, `run.sh`, exit |
| `run-ephemeral.sh` | On the host: builds the image if missing, starts one throwaway container, `--loop` to repeat |

## Token handling

- The registration token comes from `RUNNER_REG_TOKEN` (or a command in
  `RUNNER_TOKEN_CMD` that prints a fresh one). It is never written to disk.
- The host script copies it into a non-exported variable and unsets the env
  var, then pipes it to the container on stdin. It is not in the container
  environment, so job steps cannot read it from `env` or `/proc/*/environ`.
- Inside the container it lives in one shell variable until `config.sh`
  returns, then it is cleared. The runner's own credentials (`.credentials`,
  `.runner`) die with the container.
- Registration tokens expire after one hour. For `--loop`, use
  `RUNNER_TOKEN_CMD` so each job gets a fresh token, for example a
  `gh api -X POST orgs/7enlabs/actions/runners/registration-token --jq .token`
  call made with a token that has org self-hosted runner admin rights. That
  admin credential is an L3 item and is not part of this change.

## Usage

```bash
# check everything without a token or a GitHub call
ci/runner/run-ephemeral.sh --build --dry-run

# one real job (token from Org Settings > Actions > Runners > New runner)
RUNNER_REG_TOKEN=... ci/runner/run-ephemeral.sh

# keep serving jobs, one container per job
RUNNER_TOKEN_CMD='gh api -X POST orgs/7enlabs/actions/runners/registration-token --jq .token' \
  ci/runner/run-ephemeral.sh --loop
```

Settings (env): `RUNNER_URL` (default `https://github.com/7enlabs`),
`RUNNER_LABELS` (default `zc-self`), `RUNNER_GROUP`, `RUNNER_IMAGE`
(default `zc-runner:local`), `RUNNER_CPUS` (3), `RUNNER_MEMORY` (6g),
`RUNNER_MAX_SECONDS` (21600), `RUNNER_CACHE_VOLUME` (named volume mounted
at `/home/runner/.cache-shared`).

## Workflow side

```yaml
jobs:
  test:
    runs-on: [self-hosted, linux, x64, zc-self]
```

With a persistent cache volume, point `CARGO_HOME` or `SCCACHE_DIR` at
`/home/runner/.cache-shared` and set `cache-bin: "false"` on
`Swatinem/rust-cache`. Do not route `pull_request` jobs from forks to this
runner. Self-hosted runners are only for private repos.

## Container hardening

`--rm`, `--cap-drop ALL`, `--security-opt no-new-privileges`, CPU, memory
and PID limits, non-root user, wall clock cap via `timeout`. The Docker
socket is not mounted, so Docker actions, `container:` jobs and service
containers do not work on this runner. That is on purpose: mounting the
socket gives the job root on the host.

## Box notes (InstaBox zc-platform)

- The box sleeps after 30 minutes idle. A sleeping box means queued jobs
  wait. Whether the runner's long-poll keeps the box awake is not verified.
- Run under systemd for `--loop`:

```ini
[Unit]
Description=ZenCode ephemeral GitHub runner
After=docker.service
Requires=docker.service

[Service]
User=user
EnvironmentFile=/etc/zc-runner.env
ExecStart=/data/home/work/zencode/ci/runner/run-ephemeral.sh --loop
Restart=on-failure
RestartSec=60

[Install]
WantedBy=multi-user.target
```

`/etc/zc-runner.env` holds `RUNNER_TOKEN_CMD` and must be mode 0600.

## Updating the runner

`--disableupdate` means the image must be bumped by hand. GitHub stops
sending jobs to runner versions that are too old, so bump `RUNNER_VERSION`
and `RUNNER_SHA256` in the Dockerfile together (SHA from the release notes)
after the 7-day cooldown, then rebuild with `--build`.

## Not verified yet

- A real registration and job run: needs an org registration token.
- What happens to a registered runner that never got a job before the
  container was stopped: it stays offline until GitHub cleans it up or it is
  removed by hand.
- Whether the box auto-sleep pauses a waiting runner.

## Hardening notes (follow-up to #13)

- Base image is pinned by digest in the Dockerfile; bump it on purpose.
- `sudo` is not installed. Jobs run as uid 1001 with `no-new-privileges` and all capabilities dropped; install system packages in the image, not at job time.
- Registration tokens are shape-checked on the host and in the container (8-256 chars of `A-Za-z0-9_-`). A malformed token exits 66 on the host, 65 in the container; an empty one exits 65 on the host, 64 in the container.
- The dry-run log no longer prints the token length.
- The shared cache volume stays opt-in via `RUNNER_CACHE_VOLUME`; nothing is mounted by default.
