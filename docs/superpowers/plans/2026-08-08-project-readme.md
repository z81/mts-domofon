# Project README Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an accurate, concise English root README without exposing local account data or claiming unsupported deployment options.

**Architecture:** The implementation creates one documentation entry point, `README.md`, whose commands and configuration are derived directly from the Rust and frontend manifests. It documents the public local facade rather than provider internals and treats `data/` as sensitive local state.

**Tech Stack:** Markdown, Rust 1.96/Axum, React/Vite, npm, ffmpeg

## Global Constraints

- Write the README in English with a concise, operator-focused tone and section flow.
- Include a standalone disclaimer covering independence, non-affiliation, as-is use, user responsibility, real-world physical actions, and limitation of liability.
- Do not add Docker, Kubernetes, package registry, issue tracker, architecture image, or license claims that the repository does not support.
- Use synthetic placeholders only; include no real token, session, phone, address, or account data.
- Document loopback-only operation and encrypted local storage accurately.
- Keep `feature/domru-mobile-first-ux` exactly one commit ahead of `main` by amending the existing commit.

---

### Task 1: Add the operator and contributor README

**Files:**
- Create: `README.md`
- Reference: `Cargo.toml`
- Reference: `rust-toolchain.toml`
- Reference: `frontend/package.json`
- Reference: `frontend/playwright.config.ts`
- Reference: `src/config.rs`
- Reference: `src/main.rs`
- Reference: `src/api/*.rs`
- Reference: `src/storage.rs`

**Interfaces:**
- Consumes: application defaults, canonical routes, and scripts defined by the referenced source files.
- Produces: a root `README.md` used by operators and contributors; it does not alter application behavior.

- [ ] **Step 1: Run the documentation acceptance check and verify it fails**

Run:

```bash
test -f README.md \
  && rg -q '^# domru-mts$' README.md \
  && rg -q '^## Disclaimer$' README.md \
  && rg -q '^## Run locally$' README.md \
  && rg -q '^## Authentication$' README.md \
  && rg -q '^## API endpoints$' README.md \
  && rg -q '^## Development$' README.md
```

Expected: FAIL because `README.md` does not exist.

- [ ] **Step 2: Create the complete README**

Create `README.md` with exactly this content:

````markdown
# domru-mts

**domru-mts** is an independent, unofficial local web client and API for MTS/VDome intercom services.

## Overview

The application connects a linked MTS/VDome account to a local, mobile-first web interface. It provides:

- linked flats, access points, and door controls;
- camera snapshots, live video with audio, and archive playback;
- incoming call controls and confirmed in-call door opening;
- an event journal and bounded auto-open policies;
- encrypted local persistence for sessions, device identity, and push state.

## Disclaimer

This is an independent, unofficial project. It is not affiliated with, endorsed by, or supported by MTS, Dom.ru, or any of their owners, affiliates, or service providers.

The software is provided "as is", without warranties of any kind. Use it at your own risk. You are solely responsible for complying with applicable laws, service terms, and access-control rules.

Door opening, call handling, and auto-open features can perform real-world actions. Verify the selected property, access point, lock, and policy before confirming an action.

To the maximum extent permitted by applicable law, the authors and contributors are not liable for account restrictions, service interruptions, data loss, unauthorized access, property damage, or other consequences arising from use of or inability to use the software.

## Requirements

- Rust 1.96
- Node.js with npm
- `ffmpeg` available in `PATH` for live camera playback

## Run locally

Install the frontend dependencies and build the production assets:

```shell
npm --prefix frontend ci
npm --prefix frontend run build
```

Start the local service:

```shell
cargo run
```

Open http://127.0.0.1:3000/ in a browser.

The first run creates the local `data/` directory and an encryption key when `VDOME_SECRET_KEY` is not configured.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `VDOME_BIND` | `127.0.0.1:3000` | Local listen address. Only loopback addresses are accepted. |
| `VDOME_DATA_DIR` | `./data` | Encrypted session, device, and push state. |
| `VDOME_WEB_DIST_DIR` | `./frontend/dist` | Frontend production build served by the Rust service. |
| `VDOME_SECRET_KEY` | generated locally | Optional Base64-encoded 32-byte encryption key. When unset, a random key is stored under `data/`. |

`RUST_LOG` can be used to change the backend log filter, for example `RUST_LOG=debug cargo run`.

## Authentication

Open the local web interface and choose MTS ID sign-in. The application redirects to the official `login.mts.ru` authorization page and keeps provider credentials in the local encrypted store.

The interface also supports importing the MTS callback JSON directly. Paste authorization responses only into the local application, never into chats, issues, or logs.

An explicit legacy SMS flow is available through the local API for accounts that require it.

## API endpoints

The canonical local API includes:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health/live` | GET | Liveness check |
| `/api/v1/auth/start` | POST | Start MTS ID authentication |
| `/api/v1/auth/web-sso/import` | POST | Import an MTS callback response |
| `/api/v1/session` | GET / DELETE | Inspect or end the local session |
| `/api/v1/session/refresh` | POST | Refresh the provider session |
| `/api/v1/dashboard` | GET | Get linked flats and account state |
| `/api/v1/devices` | GET | Get owned intercom devices |
| `/api/v1/cameras/{cameraId}/snapshot` | GET | Get an owned camera snapshot |
| `/api/v1/cameras/{cameraId}/live` | POST | Start a bounded local live session |
| `/api/v1/cameras/{cameraId}/archive` | POST | Start archive playback |
| `/api/v1/events` | GET | Get the bounded event journal |
| `/api/v1/access/intents` | POST | Prepare a one-use door-open intent |
| `/api/v1/access/intents/{intentId}/execute` | POST | Execute a prepared door-open intent |
| `/api/v1/calls/current` | GET | Get the current incoming call |
| `/api/v1/calls/{callId}/answer`, `/decline`, `/ignore`, `/hangup` | POST | Control the current call |
| `/api/v1/calls/auto-open` | GET / DELETE | Inspect or disable the auto-open policy |

The local API is intentionally bound to loopback because it does not provide a separate client-authentication layer.

## Local data and security

Session credentials, device identity, and call-push state are encrypted at rest with AES-GCM under `data/`. Both `data/` and `.env` are ignored by Git.

Do not commit, upload, or include `data/` in a project archive. The directory contains sensitive encrypted material and, by default, its local encryption key.

Request logging redacts authorization headers, cookies, sensitive query values, media credentials, and local live-session identifiers.

## Development

Run the backend test suite:

```shell
cargo test --all-features
```

Run the frontend tests, lint, and production build:

```shell
npm --prefix frontend test
npm --prefix frontend run lint
npm --prefix frontend run build
```

Run the Playwright browser suite:

```shell
npm --prefix frontend run e2e
```

### Application architecture

The Rust/Axum backend owns provider authentication, encrypted state, ownership checks, physical-action confirmation, call handling, and bounded media sessions. The React/Vite frontend talks only to the local API. Live provider sources are converted into short-lived local HLS sessions by `ffmpeg`; provider URLs and credentials are never returned to the browser.
````

- [ ] **Step 3: Verify README facts against the source**

Run:

```bash
rg -q 'rust-version = "1\.96"' Cargo.toml
rg -q '127\.0\.0\.1:3000' src/config.rs
rg -q 'VDOME_DATA_DIR' src/config.rs
rg -q 'VDOME_WEB_DIST_DIR' src/config.rs
rg -q 'VDOME_SECRET_KEY' src/storage.rs
rg -q 'Command::new\("ffmpeg"\)' src/service/live.rs
rg -q '"test": "vitest run"' frontend/package.json
rg -q '"lint": "eslint' frontend/package.json
rg -q '"build": "tsc -b && vite build"' frontend/package.json
rg -q '"e2e": "playwright test"' frontend/package.json
```

Expected: every command exits 0.

- [ ] **Step 4: Run documentation and privacy checks**

Run:

```bash
git diff --check
test -f README.md
! rg -n -i 'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{5,}|Bearer[[:space:]]+[A-Za-z0-9._~+/-]{12,}|-----BEGIN ([A-Z ]+ )?PRIVATE KEY-----|[+]7[[:space:]()-]*[0-9]{10}' README.md
! rg -n '^## (Docker|Kubernetes|License)$' README.md
```

Expected: all checks exit 0 and print no sensitive match.

- [ ] **Step 5: Run the project verification suite**

Run:

```bash
cargo test --all-features
npm --prefix frontend test
npm --prefix frontend run lint
npm --prefix frontend run build
```

Expected: Rust tests, frontend tests, lint, and production build all pass. Existing compiler or bundle-size warnings may remain, but there must be no failures.

- [ ] **Step 6: Amend the single feature commit**

Run:

```bash
git add README.md docs/superpowers/plans/2026-08-08-project-readme.md
git commit --amend --no-edit
test "$(git rev-list --count main..HEAD)" -eq 1
git status --short --branch
```

Expected: one commit remains ahead of `main`, with a clean worktree and no co-author trailer.
