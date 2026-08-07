# Container Image and Release Design

## Goal

Provide a supported Docker/OCI distribution for `domru-mts`: a reproducible local build, a hardened Compose deployment, and automated multi-architecture publication to `ghcr.io/z81/mts-domofon`.

The container must preserve the application's current security boundary. The local API has no separate client-authentication layer, so the default deployment remains reachable only through the host loopback interface.

## Scope

The implementation adds:

- a multi-stage `Dockerfile`;
- a `.dockerignore` that excludes local state, credentials, build outputs, and repository metadata;
- `docker-compose.yml` for local build or prebuilt-image use;
- an explicit, narrowly scoped container-bind opt-in in `AppConfig`;
- a GitHub Actions workflow for quality checks, image smoke tests, and GHCR publication;
- Docker usage, upgrade, persistence, and security guidance in the README.

The repository remains standalone. Container files, labels, documentation, and workflow metadata contain only this project's own names and repository coordinates.

## Network Security Contract

`VDOME_BIND` keeps its current default of `127.0.0.1:3000`. The application continues to reject non-loopback binds unless the operator explicitly sets `VDOME_ALLOW_NON_LOOPBACK=true`.

The opt-in is intentionally narrow:

- an absent value or the exact value `false` leaves non-loopback binding disabled;
- the exact value `true` permits only unspecified wildcard addresses, `0.0.0.0` and `[::]`, which are needed inside a bridged container;
- any other flag value is a startup configuration error;
- `true` with a loopback bind is also rejected as a stale, unnecessarily dangerous opt-in;
- a concrete non-loopback address remains rejected even when the flag is enabled;
- IPv4 and IPv6 loopback addresses remain accepted only when the flag is absent or `false`.

The image itself defaults to the loopback bind. `docker-compose.yml` is the component that opts into `0.0.0.0:3000` inside the container and publishes it as `127.0.0.1:${VDOME_HOST_PORT:-3000}:3000` on the host.

The application cannot verify Docker's host-side port mapping. Documentation must therefore state that changing the host mapping to `0.0.0.0`, omitting its host IP, using host networking with a wildcard bind, or placing the service behind an unauthenticated proxy exposes every local endpoint, including physical-action mutations and the tokenless default camera stream. Host-loopback publishing is reachability control, not authentication: local host processes and any container deliberately attached to the service's network can still reach the API.

## Image Build

The Dockerfile uses three stages:

1. A `node:24-bookworm-slim` stage runs `npm ci` and `npm run build` under `frontend/`.
2. A `rust:1.96-bookworm` stage installs `build-essential`, `pkg-config`, and `libssl-dev`, copies the locked Rust workspace and vendored dependency, and runs `cargo build --locked --release`. The current dependency graph does not require CMake, Clang, or a system Protocol Buffers compiler. BuildKit cache mounts retain the Cargo registry, Git database, and target artifacts without copying them into the final image.
3. A `debian:bookworm-slim` runtime installs only `ca-certificates`, `ffmpeg`, `libssl3`, and `curl`, then receives the release binary and frontend distribution.

Each base reference is pinned by a reviewed multi-architecture manifest digest as well as its human-readable tag. Buildx executes each build stage for its target platform, natively or through QEMU; the Dockerfile does not force compiler stages onto `$BUILDPLATFORM`, because the native OpenSSL, AWS-LC, ring, and generated-code dependencies do not form a supported cross-compilation toolchain.

The build copies the release binary out of the cached Cargo target directory before the build stage ends. Cargo target caches use architecture-specific IDs and `sharing=locked`. The frontend build output is copied to `/opt/domru-mts/frontend/dist` in the runtime stage.

The runtime image defines:

- `VDOME_BIND=127.0.0.1:3000`;
- `VDOME_DATA_DIR=/var/lib/domru-mts`;
- `VDOME_WEB_DIST_DIR=/opt/domru-mts/frontend/dist`;
- `TMPDIR=/tmp`;
- a dedicated unprivileged UID and GID `10001`;
- an owner-writable `/var/lib/domru-mts` directory;
- `EXPOSE 3000`;
- a bounded HTTP healthcheck that tries `http://127.0.0.1:3000/health/live` and then the IPv6 loopback equivalent, so an explicitly selected `[::]` bind does not become permanently unhealthy;
- `STOPSIGNAL SIGINT`, matching the application's existing graceful-shutdown signal handling;
- an exec-form entrypoint for `/usr/local/bin/domru-mts`;
- OCI labels for title, description, source repository, revision, version, and build date.

No compiler, Node runtime, source tree, local session, master key, or `.env` file enters the runtime image. Debian package metadata is minimized after package installation; the design does not claim that the slim base contains no package-management executable.

## Build Context

`.dockerignore` excludes at least:

- `.git`, `.github` build outputs, and local worktrees;
- `target/`;
- `frontend/node_modules/`, `frontend/dist/`, test results, and TypeScript build state;
- `data/` and every `.env` variant;
- editor, OS, coverage, and temporary files.

The lockfiles, vendored Rust dependency, source tree, and frontend manifests remain in the context because the reproducible build requires them.

## Compose Runtime

`docker-compose.yml` defines one `domru-mts` service and one named data volume. The service supports both a local `build: .` and an overridable image reference, defaulting to `ghcr.io/z81/mts-domofon:edge`.

The service configuration includes:

- `VDOME_BIND=0.0.0.0:3000` and `VDOME_ALLOW_NON_LOOPBACK=true` only inside Compose;
- the host-loopback-only port mapping;
- the named volume mounted at `/var/lib/domru-mts`;
- `restart: unless-stopped` and `init: true`;
- `read_only: true` for the root filesystem;
- a 512 MiB writable `tmpfs` at `/tmp` with `mode=1777,nodev,nosuid,noexec` for short-lived HLS output and other temporary files; exhaustion is the intentional containment boundary for local media disk use;
- all Linux capabilities dropped and `no-new-privileges:true`;
- a 45-second graceful-stop window for HTTP draining, auto-open shutdown, push-supervisor shutdown, and live-process cleanup;
- the image healthcheck.

The Compose file never contains a provider token, session, encryption key, property identifier, or address. `VDOME_SECRET_KEY` remains optional; when absent, the application creates its key inside the persistent named volume. The generated `master.key` lives beside encrypted state, so a data-volume backup is credential-equivalent and must be protected as a secret. Supplying `VDOME_SECRET_KEY` as a plain environment value exposes it through container inspection and is not promoted by the default examples.

## Image Publication

`.github/workflows/container.yml` runs on pull requests, pushes to `main`, version tags matching `v*`, and manual dispatch. Manual runs perform verification only; publication is restricted to successful `push` events for `main` or a valid semantic-version tag.

The workflow has three responsibilities:

1. Run the existing Rust formatting/tests and frontend tests/lint/build.
2. Build a `linux/amd64` image without publishing it, validate the Compose model, start the real container, wait for `/health/live`, and run the runtime smoke checks. Before a publish event, QEMU also runs the image-level smoke checks against a loaded `linux/arm64` image so both advertised runtime architectures are exercised.
3. After successful quality and smoke jobs, authenticate to GHCR with `GITHUB_TOKEN` and publish a Buildx manifest for `linux/amd64` and `linux/arm64` only on a push to `main` or a valid semantic-version tag.

Job permissions follow least privilege: read-only repository contents by default. The publishing job alone receives `packages: write`, `id-token: write`, and `attestations: write`, and its condition independently verifies both the push event and allowed ref. Pull requests, including forked pull requests, never authenticate to GHCR and never push an image; the workflow never uses `pull_request_target`.

Published tags are:

- `edge` for `main`;
- semantic-version tags derived from a validated `vMAJOR.MINOR.PATCH`, including the full version and compatible major/minor aliases; `v0.x.y` deliberately receives no mutable `0` alias;
- `latest` only for the latest stable semantic-version release, never for a prerelease or arbitrary branch build;
- `sha-<12-character-commit>` for traceability.

Although the broad `v*` event filter starts the workflow, a release-tag guard fails publication unless the tag parses to the expected complete semantic-version image tag; an invalid tag can never fall through to a SHA-only release. The published manifest includes OCI metadata, BuildKit cache reuse, SBOM, maximal provenance, and a registry-pushed GitHub artifact attestation without a GitHub-hosted storage record. No secret is passed as a build argument because maximal provenance may record build-argument values. Third-party actions are pinned to immutable commit SHAs with their release versions recorded in comments.

The workflow cannot guarantee public GHCR visibility. After the first publish, repository administrators must verify that package visibility and repository linkage match the intended public distribution.

## Smoke-Test Contract

The container smoke job proves behavior rather than grepping Dockerfile text:

- `docker compose config` accepts the committed model;
- the image builds from a clean checkout with the lockfiles;
- a publish-bound run executes the image-level checks for `linux/arm64` under QEMU in addition to the normal `linux/amd64` Compose smoke test;
- the configured image user is non-root;
- the application starts with a fresh named volume and becomes healthy;
- `GET /health/live` succeeds through the host loopback mapping;
- the web root is served from the image-built frontend assets;
- `ffmpeg` provides the `libx264` and AAC encoders, HLS muxer, and concat demuxer required by the actual live-session command;
- the root filesystem is read-only while `/tmp` and `/var/lib/domru-mts` remain writable by the application user;
- container inspection confirms UID `10001`, exact host IP `127.0.0.1`, read-only root, `CapDrop=ALL`, `NoNewPrivs`, the expected tmpfs options, a named-volume mount, and no privileged mode or Docker socket mount;
- the generated secret files are owned by the application user with mode `0600`;
- removing and recreating the container with the same named volume preserves hashes of `master.key` and `device.enc`;
- build-context canaries placed under `.env` and `data/` are absent from image configuration, history, filesystem layers, and logs;
- the runtime image has no Cargo or Node executable;
- Docker stops the service with `SIGINT`, exit code zero, and no forced-kill status inside the configured grace period.

Unit tests cover every bind-policy branch: default loopback; explicit IPv4/IPv6 loopback with absent or false opt-in; wildcard with absent, false, or true opt-in; concrete non-loopback with true opt-in; true with loopback; and invalid values including empty, `1`, `TRUE`, and surrounding whitespace. Invalid values fail even when the requested bind is loopback.

The normal verification remains mandatory: `cargo test --all-features`, frontend tests, frontend lint, frontend production build, formatting, secret/address audits, and a clean single-commit branch.

## Error Handling

- Invalid bind or opt-in values fail before the TCP listener starts and do not silently broaden network exposure.
- A missing or read-only persistent data mount causes normal startup failure rather than falling back to ephemeral credentials.
- A missing `ffmpeg` executable remains a redacted live-session failure; the smoke test prevents publishing an image with that packaging defect.
- A failed healthcheck keeps the Compose service unhealthy and blocks the publication job.
- Image publication does not run after any quality, build, or smoke failure.
- A tag beginning with `v` but not representing the expected complete semantic version fails before registry authentication or publication.

## Documentation

The README gains a Docker section with:

- local build and detached startup via `docker compose up --build -d`;
- prebuilt-image pull and startup using an explicit version or `edge` tag;
- browser and `/api/v1/cameras/stream` URLs;
- logs, health, restart, update, and shutdown commands;
- named-volume persistence, stopped-service backup guidance, and explicit handling of every backup as credential material;
- warnings that host processes, attached container peers, changed host mappings, host networking, or unauthenticated proxies can reach unauthenticated read and mutation endpoints;
- GHCR architecture support for `linux/amd64` and `linux/arm64`.

The existing disclaimer remains unchanged and prominent.

## Explicit Exclusions

- No Kubernetes manifests or Home Assistant add-on metadata.
- No `linux/arm/v7` image until all Rust, Node, TLS, and FFmpeg build/runtime dependencies are proven on that architecture.
- No embedded credentials or default local password.
- No automatic public exposure, TLS termination, or bundled reverse proxy.
- No publication to Docker Hub.
- No second commit: specification, implementation, tests, workflow, and documentation are folded into the existing feature commit with amend and no co-author trailer.
