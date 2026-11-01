# Ephemeral GitHub Actions runner image for ZenCode self-hosted CI.
# One container = one registration = one job. See README.md.
# Base image pinned by digest (ubuntu:24.04 index digest, checked 2026-11-02).
# Bump the digest on purpose; `docker pull ubuntu:24.04` then read RepoDigests.
FROM ubuntu:24.04@sha256:fe81560990fae86dc4743202b2cb4d3df98d0bcf949173cecef15ce1127ef852

# Pinned runner release. v2.338.0 is the newest stable release past the
# team's 7-day package cooldown on 2026-11-01. Bump RUNNER_VERSION and
# RUNNER_SHA256 together; the SHA comes from the release notes
# (<!-- BEGIN SHA linux-x64 --> marker) of github.com/actions/runner.
ARG RUNNER_VERSION=2.338.0
ARG RUNNER_SHA256=af4b794c1bc41d73d40535e3fe092a39f9679cd8d965954c2aca25a05ca41d32

ENV DEBIAN_FRONTEND=noninteractive \
    RUNNER_HOME=/home/runner/actions-runner \
    RUNNER_ALLOW_RUNASROOT=0

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# Base tools the runner and typical ZenCode jobs need. Rust toolchains are
# installed by the workflow (rustup / dtolnay/rust-toolchain), not baked in.
# Ubuntu archive versions rotate, so packages track the 24.04 pocket.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl git jq tar gzip unzip zstd xz-utils \
       build-essential pkg-config libssl-dev python3 tini \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 1001 --shell /bin/bash runner

WORKDIR /home/runner/actions-runner

RUN curl -fsSL -o runner.tar.gz \
       "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz" \
    && echo "${RUNNER_SHA256}  runner.tar.gz" | sha256sum -c - \
    && tar -xzf runner.tar.gz \
    && rm runner.tar.gz \
    && ./bin/installdependencies.sh \
    && rm -rf /var/lib/apt/lists/* \
    && chown -R runner:runner /home/runner

COPY --chmod=0755 entrypoint.sh /usr/local/bin/zc-runner-entrypoint

USER 1001

# tini reaps zombies and forwards SIGTERM so the runner can stop cleanly.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/zc-runner-entrypoint"]
