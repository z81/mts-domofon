# Live Camera HLS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Play an owned MTS camera's live video and audio inside the local web UI after an explicit button click.

**Architecture:** Resolve the private RTSP source inside the Rust provider boundary, then give it to a bounded local `ffmpeg` session manager which produces rolling same-origin HLS. The browser receives only an opaque local session URL and plays it with native HLS or `hls.js`.

**Tech Stack:** Rust 2024, Axum, Tokio process/fs, `tempfile`, React 19, TypeScript 6, `hls.js` 1.6.17, Vitest.

## Global Constraints

- Never expose an MTS stream URL, token, media credential, temporary path, or ffmpeg stderr through JSON, HTML, logs, debug output, or process arguments.
- Accept only bounded `rtsp` and `rtsps` sources and invoke ffmpeg directly without a shell.
- Start playback only from **Смотреть live**; keep audio available and stop on explicit stop or component cleanup.
- Enforce ownership before resolving or starting a source, at most two sessions, a 15-second startup deadline, and a 10-minute maximum session lifetime.
- Do not add archive, recording, download, screenshots, external players, WebRTC, or non-loopback serving.

---

### Task 1: Private MTS live-source resolution

**Files:**
- Modify: `src/provider/mts/dto/intercom.rs`
- Modify: `src/provider/mts/intercom.rs`
- Modify: `src/provider/mts/mod.rs`
- Test: `tests/contract/intercom_provider.rs`

**Interfaces:**
- Produces: `LiveSourceProvider::live_source(&FlatId, &str) -> Result<SecretString, AppError>` implemented by `MtsDeviceProvider`.
- Selection order: `streamsToWatch.rtsp`, first non-blank `streamsToWatch`, `streams.rtsp`, first non-blank `streams`.

- [ ] Write a provider test with four distinct synthetic URLs and assert the returned secret is the `streamsToWatch.rtsp` literal while inventory JSON contains none of them.
- [ ] Run `cargo test --all-features --test intercom_provider live_source -- --nocapture` and verify RED because `LiveSourceProvider` does not exist.
- [ ] Change `StreamMapEvidence` to retain a bounded preferred `SecretString`, validate with `url::Url`, and implement the trait by using the existing flat-scoped camera pagination and exact camera ID matching.
- [ ] Run the focused provider tests and verify GREEN.
- [ ] Commit with `feat: resolve private MTS live sources`.

### Task 2: Bounded local HLS session manager

**Files:**
- Modify: `Cargo.toml`
- Create: `src/service/live.rs`
- Modify: `src/service/mod.rs`
- Test: `tests/live_session.rs`
- Modify: `Cargo.toml` test target declarations

**Interfaces:**
- Produces: `LiveSessionManager::start(SecretString) -> Result<LiveSessionView, AppError>`, `read(Uuid, &str) -> Result<LiveAsset, AppError>`, and `stop(Uuid) -> Result<(), AppError>`.
- `LiveSessionView { session_id: Uuid, playlist_url: String }`; `LiveAsset { content_type: &'static str, bytes: Bytes }`.

- [ ] Add a failing test around an injected `LiveProcessRunner` proving readiness, playlist sanitization, fixed asset names, two-session capacity, stop idempotency, and secret-free debug/errors.
- [ ] Run `cargo test --all-features --test live_session` and verify RED because the module is absent.
- [ ] Move `tempfile = "3"` to runtime dependencies and implement a manager using `tokio::process::Command`, 0700 temp directories, a rolling six-segment HLS playlist, H.264/AAC output, and a concat description written to child stdin.
- [ ] Permit only `index.m3u8` and `segment-[0-9]+.ts`, cap playlist/segment reads, strip paths from playlist URI lines, kill children on failure/stop/TTL, and discard stderr.
- [ ] Run `cargo test --all-features --test live_session` and verify GREEN.
- [ ] Commit with `feat: add bounded local HLS sessions`.

### Task 3: Ownership-checked live API

**Files:**
- Modify: `src/state.rs`
- Modify: `src/api/intercom.rs`
- Modify: `src/app.rs`
- Modify: `tests/runtime_api.rs`

**Interfaces:**
- Produces: `POST /api/v1/cameras/{camera_id}/live?flatId=...`, `GET /api/v1/live/{session_id}/{asset}`, and `DELETE /api/v1/live/{session_id}`.
- Start response is exactly `{ "sessionId": "...", "playlistUrl": "/api/v1/live/.../index.m3u8" }`.

- [ ] Add failing API tests proving unowned/unavailable cameras never resolve a source, valid starts return only local fields, playlist/segments use private no-store headers and exact MIME types, and malformed descendants are 404.
- [ ] Run `cargo test --all-features --test runtime_api live` and verify RED.
- [ ] Wire the concrete source provider and manager into `AppState`, implement the three routes, validate all opaque values before dispatch, and redact `/api/v1/live/{session}` paths from tracing.
- [ ] Run the focused API tests and verify GREEN.
- [ ] Commit with `feat: expose local camera live sessions`.

### Task 4: Browser HLS player with sound

**Files:**
- Modify: `frontend/package.json`
- Modify: `frontend/package-lock.json`
- Modify: `frontend/src/features/intercom/intercomApi.ts`
- Modify: `frontend/src/features/intercom/intercomApi.test.ts`
- Modify: `frontend/src/app/App.tsx`
- Modify: `frontend/src/app/App.test.tsx`
- Modify: `frontend/src/app/styles.css`

**Interfaces:**
- Produces optional `IntercomApi.startLive(flatId, cameraId, signal)` and `stopLive(sessionId)` methods returning/consuming `LiveSession`.
- UI button copy is **Смотреть live** and **Остановить**; video uses `controls`, `playsInline`, and audio is not forced muted.

- [ ] Install exact `hls.js@1.6.17` and add failing API/UI tests for the start URL, local playlist validation, button-driven startup, video controls/audio, generic errors, stop, and unmount cleanup.
- [ ] Run `npm test -- --run` in `frontend` and verify RED.
- [ ] Implement native HLS first and `hls.js` fallback, best-effort DELETE cleanup, retry state, and camera/flat/page cancellation without opening another window.
- [ ] Run frontend tests, lint, typecheck, and build; verify GREEN.
- [ ] Commit with `feat: play camera live video with audio`.

### Task 5: End-to-end verification

**Files:**
- Modify only files required by failures found in this task.

**Interfaces:**
- Consumes all prior task contracts; produces a running verified local server.

- [ ] Run `cargo test --all-features`, `cargo clippy --all-features --lib --tests -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check`.
- [ ] Run `npm test -- --run`, `npm run lint`, `npm run typecheck`, and `npm run build` in `frontend`.
- [ ] Rebuild and restart only the exact `domru-mts` listener on `127.0.0.1:3000`.
- [ ] Create one real live session through the local API, verify a 200 HLS playlist references a 200 non-empty `video/mp2t` segment, then DELETE the session without printing or saving private provider material.
- [ ] Commit any verification-only corrections with a focused message and leave the worktree clean.
