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

Choose one of these runtime options:

- Docker Engine with Docker Compose; or
- Rust 1.96, Node.js with npm, and `ffmpeg` available in `PATH` for native development.

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

## Run with Docker

Build the image locally and start the hardened Compose deployment:

```shell
docker compose up --build -d
docker compose ps
```

Open http://127.0.0.1:3000/ in a browser. The liveness endpoint is available at http://127.0.0.1:3000/health/live, and the stable first-camera stream URL is http://127.0.0.1:3000/api/v1/cameras/stream.

To run a prebuilt image, select a release and start without rebuilding:

```shell
export DOMRU_MTS_IMAGE=ghcr.io/z81/mts-domofon:1.2.3
docker compose pull
docker compose up -d --no-build
```

Release tags are the safer choice for repeatable deployments. The `edge` tag follows the current `main` branch. Published images support `linux/amd64` and `linux/arm64`.

To use a different host-loopback port, set `VDOME_HOST_PORT` for every Compose command in that shell:

```shell
export VDOME_HOST_PORT=3100
docker compose up -d
```

The service remains available only at `http://127.0.0.1:3100/`; changing the port does not change its security boundary.

Common operations:

```shell
docker compose logs -f domru-mts
docker compose restart domru-mts
docker compose pull
docker compose up -d --no-build
docker compose stop
docker compose down
```

`docker compose down` preserves the Compose-managed `domru-mts-data` volume. Do not add `--volumes` unless you intentionally want to delete the local device identity, session state, and generated encryption key.

For a consistent backup, stop the service before copying its data:

```shell
docker compose stop
mkdir -p backup
docker compose cp domru-mts:/var/lib/domru-mts/. ./backup/
```

Protect the entire backup as credential material. It contains encrypted provider state and the `master.key` capable of decrypting that state. Restrict its permissions and never attach it to issues, logs, chats, or public archives. Supplying `VDOME_SECRET_KEY` as a plain Compose environment value is not recommended because container configuration can expose environment values through inspection.

Compose intentionally binds the application to `0.0.0.0` only inside its private bridge and publishes port `3000` exclusively on host `127.0.0.1`. This is reachability control, not local API authentication: any process on the host, and any container deliberately attached to the Compose network, can invoke the API without a separate client credential.

Do not remove the `127.0.0.1` host IP from the port mapping, replace it with `0.0.0.0`, use host networking, or put the service behind an unauthenticated proxy. Doing so exposes camera data and every mutation endpoint, including door-opening, call-control, and auto-open actions. If remote access is required, provide authentication, TLS, and network access controls outside this project before exposing it.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `VDOME_BIND` | `127.0.0.1:3000` | Local listen address. Loopback is required unless the container-only wildcard opt-in is active. |
| `VDOME_ALLOW_NON_LOOPBACK` | unset | Container-only escape hatch. Exact `true` permits only `0.0.0.0` or `[::]`; native use must leave it unset. |
| `VDOME_DATA_DIR` | `./data` | Encrypted session, device, and push state. |
| `VDOME_WEB_DIST_DIR` | `./frontend/dist` | Frontend production build served by the Rust service. |
| `VDOME_SECRET_KEY` | generated locally | Optional Base64-encoded 32-byte encryption key. When unset, a random key is stored under `data/`. |

`RUST_LOG` can be used to change the backend log filter, for example `RUST_LOG=debug cargo run`.

`DOMRU_MTS_IMAGE` and `VDOME_HOST_PORT` configure Compose itself rather than the application. Their defaults are `ghcr.io/z81/mts-domofon:edge` and host-loopback port `3000`.

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
| `/api/v1/cameras/stream` | GET | Redirect to the selected flat's first live-capable camera |
| `/api/v1/cameras/{cameraId}/snapshot` | GET | Get an owned camera snapshot |
| `/api/v1/cameras/{cameraId}/live` | POST | Start a bounded local live session |
| `/api/v1/cameras/{cameraId}/archive` | POST | Start archive playback |
| `/api/v1/events` | GET | Get the bounded event journal |
| `/api/v1/access/intents` | POST | Prepare a one-use door-open intent |
| `/api/v1/access/intents/{intentId}/execute` | POST | Execute a prepared door-open intent |
| `/api/v1/calls/current` | GET | Get the current incoming call |
| `/api/v1/calls/{callId}/answer`, `/decline`, `/ignore`, `/hangup` | POST | Control the current call |
| `/api/v1/calls/auto-open` | GET / DELETE | Inspect or disable the auto-open policy |

Open the default camera stream in a player that follows HTTP redirects:

```shell
ffplay http://127.0.0.1:3000/api/v1/cameras/stream
```

This request needs no local authorization header, token, query parameter, or body. The backend still needs a valid stored provider session and returns a short-lived local HLS URL.

The local API is intentionally bound to loopback because it does not provide a separate client-authentication layer. Any local process able to reach the port can invoke the stable camera stream URL. Do not expose the service through a public bind or an unauthenticated reverse proxy.

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
