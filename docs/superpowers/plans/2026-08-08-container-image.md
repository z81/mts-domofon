# Container Image Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a hardened, reproducible Docker/OCI image, safe loopback-only Compose deployment, and multi-architecture GHCR release workflow for `domru-mts`.

**Architecture:** A narrow configuration flag allows only wildcard binds inside a bridged container, while Compose publishes the port exclusively on host loopback. Separate Node and Rust build stages feed a non-root Debian runtime image; a smoke script verifies the effective runtime controls, and GitHub Actions publishes attested `amd64`/`arm64` manifests only after tests and runtime checks pass.

**Tech Stack:** Rust 1.96, Axum, Node.js 24, React/Vite, Docker BuildKit/Buildx, Docker Compose, Debian Bookworm, GitHub Actions, GHCR.

## Global Constraints

- Keep `VDOME_BIND=127.0.0.1:3000` as the application and image default.
- Permit only `0.0.0.0` or `[::]` when `VDOME_ALLOW_NON_LOOPBACK` is exactly `true`; reject concrete non-loopback addresses and reject a stale `true` flag with loopback.
- Publish Compose port `3000` only on host `127.0.0.1`.
- Use non-root UID/GID `10001`, a read-only root filesystem, dropped capabilities, no-new-privileges, a named data volume, and a 512 MiB `/tmp` tmpfs.
- Support `linux/amd64` and `linux/arm64`; do not add `linux/arm/v7`.
- Publish only to `ghcr.io/z81/mts-domofon` on `main` pushes and valid `vMAJOR.MINOR.PATCH` tags.
- Never put provider tokens, sessions, master keys, property identifiers, real addresses, `.env`, or `data/` into the image or Compose model.
- Do not mention any reference repository in standalone project files.
- Preserve the existing disclaimer.
- Make no intermediate commits. Stage all work and finish with exactly one `git commit --amend --no-edit`, without a co-author trailer.

---

### Task 1: Container bind policy

**Files:**
- Modify: `src/config.rs`
- Modify: `tests/config.rs`

**Interfaces:**
- Consumes: `AppConfig::from_iter<I, K, V>(values) -> Result<AppConfig, ConfigError>`.
- Produces: strict parsing of `VDOME_ALLOW_NON_LOOPBACK` and stable `ConfigError` variants for invalid flag values and rejected bind/flag combinations.

- [ ] **Step 1: Replace the broad bind test with the complete failing policy matrix**

```rust
#[test]
fn wildcard_bind_requires_exact_opt_in() {
    for bind in ["0.0.0.0:3000", "[::]:3000"] {
        assert!(AppConfig::from_iter([("VDOME_BIND", bind)]).is_err());
        assert!(AppConfig::from_iter([
            ("VDOME_BIND", bind),
            ("VDOME_ALLOW_NON_LOOPBACK", "false"),
        ])
        .is_err());
        assert_eq!(
            AppConfig::from_iter([
                ("VDOME_BIND", bind),
                ("VDOME_ALLOW_NON_LOOPBACK", "true"),
            ])
            .unwrap()
            .bind
            .to_string(),
            bind
        );
    }
}

#[test]
fn rejects_invalid_or_stale_non_loopback_opt_in() {
    for value in ["", "1", "TRUE", " true", "true "] {
        assert!(matches!(
            AppConfig::from_iter([("VDOME_ALLOW_NON_LOOPBACK", value)]),
            Err(ConfigError::InvalidAllowNonLoopback)
        ));
    }
    assert!(matches!(
        AppConfig::from_iter([("VDOME_ALLOW_NON_LOOPBACK", "true")]),
        Err(ConfigError::AllowNonLoopbackRequiresWildcard)
    ));
}
```

Also retain explicit acceptance tests for `127.0.0.2:3100` and `[::1]:3100` with the flag absent or `false`, and rejection tests for `192.168.1.50`, `10.0.0.8`, and `2001:db8::1` even with the flag set to `true`.

- [ ] **Step 2: Run the focused tests and verify the new cases fail**

Run: `cargo test --test config`

Expected: the exact-`true` wildcard cases fail because current code rejects every non-loopback bind, and the new error variants do not exist yet.

- [ ] **Step 3: Implement exact flag parsing and bind validation**

```rust
let allow_non_loopback = match values.get("VDOME_ALLOW_NON_LOOPBACK").map(String::as_str) {
    None | Some("false") => false,
    Some("true") => true,
    Some(_) => return Err(ConfigError::InvalidAllowNonLoopback),
};

match (bind.ip().is_loopback(), bind.ip().is_unspecified(), allow_non_loopback) {
    (true, _, false) | (false, true, true) => {}
    (true, _, true) => return Err(ConfigError::AllowNonLoopbackRequiresWildcard),
    _ => return Err(ConfigError::BindNotAllowed),
}
```

Define `InvalidAllowNonLoopback`, `AllowNonLoopbackRequiresWildcard`, and `BindNotAllowed` with messages that name only configuration variables and never echo the rejected value.

- [ ] **Step 4: Run focused formatting, lint, and tests**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --test config`

Expected: all commands exit zero.

- [ ] **Step 5: Stage without committing**

Run: `git add src/config.rs tests/config.rs`

Expected: changes are staged and `main..HEAD` still contains exactly one commit.

---

### Task 2: Reproducible non-root image and Compose runtime

**Files:**
- Create: `.dockerignore`
- Create: `Dockerfile`
- Create: `docker-compose.yml`

**Interfaces:**
- Consumes: the bind policy from Task 1, binary `domru-mts`, frontend output `frontend/dist`, and runtime paths read by `AppConfig`.
- Produces: image `domru-mts:local` and Compose service `domru-mts` with named volume `domru-mts-data`.

- [ ] **Step 1: Add a denylist build context**

Create `.dockerignore` containing root repository metadata/worktrees, `target/`, frontend dependencies/build/test output, `.env` and `.env.*` at any depth, `data/`, coverage, editor/OS files, logs, and temporary files. Keep `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src/`, `vendor/`, `frontend/package.json`, `frontend/package-lock.json`, and frontend source/config files available.

Required patterns include:

```dockerignore
.git
.github
.worktrees
target
frontend/node_modules
frontend/dist
frontend/test-results
frontend/playwright-report
frontend/tsconfig.tsbuildinfo
.env
.env.*
**/.env
**/.env.*
data
**/data
coverage
*.log
.DS_Store
```

- [ ] **Step 2: Add the three-stage Dockerfile**

Use these reviewed manifest pins:

```dockerfile
# syntax=docker/dockerfile:1.7
FROM node:24-bookworm-slim@sha256:3638d9a6fe4030bd716be989438248074489337ba3275657f93595428be4fc03 AS frontend-build
FROM rust:1.96-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663 AS rust-build
FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime
```

The Node stage uses `npm ci` with a locked npm cache and `npm run build`. The Rust stage installs `build-essential pkg-config libssl-dev`, uses locked registry/Git/target caches whose target ID contains `${TARGETARCH}`, runs `cargo build --locked --release`, and copies the binary from the cache mount into `/out/domru-mts` during the same `RUN` instruction.

The runtime stage installs `ca-certificates curl ffmpeg libssl3`, removes apt lists, creates UID/GID `10001`, copies the binary to `/usr/local/bin/domru-mts` and assets to `/opt/domru-mts/frontend/dist`, owns `/var/lib/domru-mts`, and defines:

```dockerfile
ENV VDOME_BIND=127.0.0.1:3000 \
    VDOME_DATA_DIR=/var/lib/domru-mts \
    VDOME_WEB_DIST_DIR=/opt/domru-mts/frontend/dist \
    TMPDIR=/tmp
USER 10001:10001
EXPOSE 3000
STOPSIGNAL SIGINT
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=6 CMD curl --noproxy '*' --fail --silent --show-error --max-time 2 http://127.0.0.1:3000/health/live >/dev/null || curl --noproxy '*' --fail --silent --show-error --max-time 2 'http://[::1]:3000/health/live' >/dev/null || exit 1
ENTRYPOINT ["/usr/local/bin/domru-mts"]
```

Add OCI title, description, source, revision, version, created, and license labels. Accept revision/version/created as build arguments; hard-code this repository's own HTTPS source URL.

- [ ] **Step 3: Add hardened Compose defaults**

Create a single service with both `image: ${DOMRU_MTS_IMAGE:-ghcr.io/z81/mts-domofon:edge}` and `build: .`, environment values `VDOME_BIND=0.0.0.0:3000` and `VDOME_ALLOW_NON_LOOPBACK=true`, long-form port mapping with `host_ip: 127.0.0.1`, target/published port `3000`, and named volume destination `/var/lib/domru-mts`.

Set `user: "10001:10001"`, `read_only: true`, `tmpfs: /tmp:size=536870912,mode=1777,nodev,nosuid,noexec`, `cap_drop: [ALL]`, `security_opt: [no-new-privileges:true]`, `init: true`, `restart: unless-stopped`, and `stop_grace_period: 45s`. Do not include secrets or provider configuration.

- [ ] **Step 4: Validate what is available locally**

Run: `git diff --check && rg -n -i 'token|session|secret|password|address|улиц|дом ' Dockerfile docker-compose.yml .dockerignore`

Expected: only defensive ignore patterns or variable names appear; there are no values. If Docker is installed, also run `docker compose config --quiet` and `docker build -t domru-mts:local .`.

- [ ] **Step 5: Stage without committing**

Run: `git add .dockerignore Dockerfile docker-compose.yml`

Expected: changes are staged and no new commit exists.

---

### Task 3: Runtime smoke test and GHCR workflow

**Files:**
- Create: `scripts/container-smoke.sh`
- Create: `.github/workflows/container.yml`

**Interfaces:**
- Consumes: `DOMRU_MTS_IMAGE`, Compose service `domru-mts`, port variable `VDOME_HOST_PORT`, and the image/runtime contract from Task 2.
- Produces: repeatable amd64/arm64 smoke checks and an immutable-action, least-privilege GHCR publication pipeline.

- [ ] **Step 1: Implement the runtime smoke script**

Write a POSIX-compatible Bash script with `set -euo pipefail` that accepts `DOMRU_MTS_IMAGE` and optional `VDOME_HOST_PORT`, runs `docker compose config --quiet`, starts the service with `docker compose up --no-build --wait --wait-timeout 90 --detach`, and registers an `EXIT` trap for `docker compose down --volumes --remove-orphans`.

The script must assert with `docker inspect`, `curl`, and `docker compose exec -T`:

```text
health/live returns {"status":"ok"}; web root contains <!doctype html;
Config.User equals 10001:10001; HostConfig.ReadonlyRootfs is true;
HostConfig.Privileged is false; CapDrop contains ALL; SecurityOpt contains no-new-privileges;
port 3000/tcp HostIp equals 127.0.0.1; /tmp is tmpfs with rw,nodev,nosuid,noexec,size=536870912;
/var/lib/domru-mts is a Docker volume; /var/run/docker.sock is absent;
root write fails; application user can write/remove probes in /tmp and the data directory;
master.key and device.enc are UID/GID 10001 and mode 600;
ffmpeg exposes libx264, AAC, HLS, and concat; cargo and node are absent.
```

Hash `master.key` and `device.enc`, remove only the service container with `docker compose rm --stop --force`, recreate it with the same named volume, and verify both hashes are unchanged. Stop it with `docker compose stop --timeout 45`, assert exit code `0`, and assert `State.OOMKilled` is false and no forced-kill indication appears.

- [ ] **Step 2: Add clean-checkout quality and amd64 smoke jobs**

Create `.github/workflows/container.yml` with triggers `pull_request` to `main`, pushes to `main` and `v*`, and `workflow_dispatch`; set workflow-level `permissions: contents: read` and concurrency cancellation by workflow/ref.

Pin actions to:

```text
actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803 # v6
actions/setup-node@249970729cb0ef3589644e2896645e5dc5ba9c38 # v6
docker/login-action@dbcb813823bdd20940b903addbd779551569679f # v4
docker/metadata-action@dc802804100637a589fabce1cb79ff13a1411302 # v6
docker/setup-qemu-action@96fe6ef7f33517b61c61be40b68a1882f3264fb8 # v4
docker/setup-buildx-action@bb05f3f5519dd87d3ba754cc423b652a5edd6d2c # v4
docker/build-push-action@53b7df96c91f9c12dcc8a07bcb9ccacbed38856a # v7
actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6 # v4
```

Checkout always sets `persist-credentials: false`. Quality installs Rust 1.96 with rustup and runs format, clippy, all-feature tests, then Node 24 `npm ci`, tests, lint, and build. Smoke creates unique canaries beneath ignored `.env/` and `data/`, builds `linux/amd64` with `load: true`, runs `scripts/container-smoke.sh`, and proves the canary absent from image inspect, history, saved layers, exported filesystem, and service logs. Cleanup runs under `if: always()`.

- [ ] **Step 3: Add release guards, arm64 smoke, and publication**

For pushes eligible to publish, set up QEMU and build/load `linux/arm64`, then run the image-level health, UID, FFmpeg capability, and missing-tool checks under emulation. The publish job must depend on quality, amd64 smoke, and arm64 smoke and use this independent condition:

```yaml
if: >-
  github.event_name == 'push' &&
  (github.ref == 'refs/heads/main' || startsWith(github.ref, 'refs/tags/v'))
permissions:
  contents: read
  packages: write
  id-token: write
  attestations: write
```

Set `DOCKER_METADATA_SHORT_SHA_LENGTH=12`; generate `edge` for main, full/minor SemVer aliases, major only when not `v0.*`, automatic stable-only `latest`, and `sha-*`. Before login, validate `refs/tags/v*` against `^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$` and assert metadata contains the complete version tag.

Publish `linux/amd64,linux/arm64` with GHA cache, `provenance: mode=max`, `sbom: true`, OCI labels/args, then run `actions/attest` with `push-to-registry: true` and `create-storage-record: false`. Inspect the pushed digest and assert both platforms exist. Never pass a secret as a build argument.

- [ ] **Step 4: Lint shell/YAML and inspect workflow security**

Run: `bash -n scripts/container-smoke.sh && git diff --check && rg -n 'pull_request_target|permissions: write-all|password:|VDOME_SECRET_KEY' .github/workflows/container.yml scripts/container-smoke.sh`

Expected: Bash syntax succeeds; the search finds only the GHCR login `password: ${{ secrets.GITHUB_TOKEN }}` and no forbidden trigger, broad permission, or application key.

- [ ] **Step 5: Stage without committing**

Run: `git add scripts/container-smoke.sh .github/workflows/container.yml`

Expected: both files are staged and executable mode is set on the smoke script.

---

### Task 4: Docker operator documentation

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: Compose variables/commands and GHCR tags created by Tasks 2–3.
- Produces: standalone operator instructions with explicit unauthenticated-API and backup warnings.

- [ ] **Step 1: Extend requirements and configuration accurately**

Add Docker with Compose as an alternative to the native Rust/Node/FFmpeg toolchain. Add `VDOME_ALLOW_NON_LOOPBACK` to the table as a container-only escape hatch that accepts exactly `true` and only wildcard binds; state that ordinary native use must leave it unset.

- [ ] **Step 2: Add runnable Docker commands and URLs**

Document:

```shell
docker compose up --build -d
docker compose ps
docker compose logs -f domru-mts
docker compose pull
docker compose up -d --no-build
docker compose stop
docker compose down
```

Explain `DOMRU_MTS_IMAGE=ghcr.io/z81/mts-domofon:1.2.3`, `VDOME_HOST_PORT`, browser URL, `/health/live`, and `/api/v1/cameras/stream`. State that `edge` follows main and a version tag is safer for repeatable deployments. State support for `linux/amd64` and `linux/arm64`.

- [ ] **Step 3: Document persistence, backups, and the real security boundary**

Name the Compose-managed volume, require stopping the service before backup, and say that the backup is credential-equivalent because `master.key` and encrypted provider state coexist. Do not recommend plain `VDOME_SECRET_KEY` in Compose because environment values are visible via container inspection.

Warn that loopback publishing is reachability control rather than authentication. Any host process and any deliberately attached peer container can reach the API; changing the mapping to wildcard, using host networking, or placing it behind an unauthenticated proxy exposes camera data and physical-action endpoints. Preserve the existing disclaimer verbatim.

- [ ] **Step 4: Audit the rendered text**

Run: `rg -n -i 'co-authored-by|anthropic|codex' README.md Dockerfile docker-compose.yml .github scripts docs/superpowers/specs/2026-08-08-container-image-design.md`

Expected: no matches. Manually confirm no example contains a reference-repository name, personal name, real token, session, property identifier, or street/property address.

- [ ] **Step 5: Stage without committing**

Run: `git add README.md`

Expected: README is staged without modifying the disclaimer text.

---

### Task 5: Integrated verification and single-commit delivery

**Files:**
- Modify if verification exposes a defect: files from Tasks 1–4
- Stage: `docs/superpowers/plans/2026-08-08-container-image.md`

**Interfaces:**
- Consumes: every deliverable above.
- Produces: one clean, verified feature commit with no sensitive material or co-author trailer.

- [ ] **Step 1: Run source verification**

Run:

```shell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
npm --prefix frontend ci
npm --prefix frontend test
npm --prefix frontend run lint
npm --prefix frontend run build
bash -n scripts/container-smoke.sh
git diff --check
```

Expected: every command exits zero; the existing Vite chunk-size warning is informational only.

- [ ] **Step 2: Run container verification where a Docker engine is available**

Run:

```shell
docker compose config --quiet
DOMRU_MTS_IMAGE=domru-mts:local docker build -t domru-mts:local .
DOMRU_MTS_IMAGE=domru-mts:local scripts/container-smoke.sh
```

Expected: build and all effective-runtime checks pass. If the local host has no Docker-compatible engine, record that limitation explicitly and rely on the committed CI smoke workflow to run these checks on GitHub-hosted Docker.

- [ ] **Step 3: Run repository leak audits**

Search tracked files for private street/property/address terms, credential-like assignments, authorization/session dumps, private keys, forbidden reference names, and co-author trailers. Review every match; provider endpoint constants and synthetic test fixtures are allowed, real personal/provider state is not.

- [ ] **Step 4: Amend the only feature commit**

Run:

```shell
git add -A
git commit --amend --no-edit
```

Expected: amend succeeds without adding any trailer.

- [ ] **Step 5: Prove final branch invariants**

Run:

```shell
test "$(git rev-list --count main..HEAD)" -eq 1
test -z "$(git status --porcelain)"
git log -1 --format='%H%n%B'
git diff --stat main...HEAD
```

Expected: one commit ahead of `main`, clean worktree, and no `Co-Authored-By` line.
