# Events, Archive, Call History, and Auto-Open Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the remaining core intercom placeholders with an MTS-backed event journal, call history, camera archive playback with audio, and bounded auto-open modes.

**Architecture:** Rust owns every MTS request, validates flat/device ownership, normalizes provider DTOs, and keeps media URLs and call secrets server-side. Events and call history share one filtered event service; archive sources reuse the bounded ffmpeg/HLS manager; auto-open is an encrypted, session-bound state machine driven by a server-side call watcher. React consumes only stable local models and splits the new screens into focused feature modules.

**Tech Stack:** Rust 1.96, Tokio, Axum, Reqwest, Serde, Chrono, AES-GCM storage, ffmpeg/HLS, React 19, TypeScript, hls.js, Vitest, Testing Library.

## Global Constraints

- MTS DTOs, tokens, phone numbers, SIP data, peer data, upstream URLs, and raw response bodies never cross the local API.
- Every event, archive request, and automatic action fails closed unless ownership is proven.
- Event ranges are at most 31 days; the default is the newest 20 events from seven days.
- Archive playback is at most 30 minutes and preserves audio when the source contains it.
- Auto-open has only `disabled`, `next_call`, and a server-bounded 30-minute `until` mode.
- Arming auto-open requires a short-lived one-use confirmation intent; disabling is immediate.
- Duplicate calls and ambiguous unlock results never cause an automatic retry.
- Automated verification uses synthetic provider data and read-only discovery; it never opens a real door, answers a real call, or arms a real policy.

---

### Task 1: Provider-neutral event journal and local API

**Files:**
- Create: `src/domain/event.rs`
- Create: `src/provider/mts/events.rs`
- Create: `src/service/events.rs`
- Create: `src/api/events.rs`
- Create: `tests/events.rs`
- Modify: `src/domain/mod.rs`
- Modify: `src/provider/mts/mod.rs`
- Modify: `src/service/mod.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/state.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Produces `EventKind`, `CallOutcome`, `EventItem`, `EventPage`, and `EventQuery` in `domain::event`.
- Produces `EventProvider::page(ProviderEventQuery) -> ProviderEventPage` and `MtsEventProvider`.
- Produces `EventService::events(EventQuery) -> Result<EventPage, AppError>`.
- Produces `GET /api/v1/events?flatId=&page=&size=&from=&to=&kind=`.

- [ ] **Step 1: Write failing provider and service tests**

Add literal fixtures for `intercom.incoming.call`, `intercom.unlocked`, camera-context, malformed, and foreign-context events. Assert the exact MTS query contract and these observable results:

```rust
assert_eq!(page.items[0].kind, EventKind::Call);
assert_eq!(page.items[0].call_outcome, Some(CallOutcome::Missed));
assert_eq!(page.items[1].kind, EventKind::Access);
assert!(page.items.iter().all(|event| event.id != "foreign-event"));
assert!(!serialized.contains("pictureUrl"));
assert!(!serialized.contains("sip"));
```

The production mutations caught are wrong alias classification, missing ownership filtering, unsafe nested DTO exposure, invalid timestamp acceptance, and trusting MTS account-wide totals.

- [ ] **Step 2: Run RED**

```bash
cargo test --features test-support --test events
```

Expected: compilation fails because the event domain/provider/service do not exist.

- [ ] **Step 3: Add bounded event models and MTS parsing**

Implement these stable shapes:

```rust
pub enum EventKind { Call, Access, Camera, Other }
pub enum CallOutcome { Accepted, Missed, Rejected, Ignored, Unknown }
pub struct EventItem {
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub kind: EventKind,
    pub title: String,
    pub source_label: Option<String>,
    pub call_outcome: Option<CallOutcome>,
    pub duration_seconds: Option<u64>,
    pub camera_id: Option<String>,
    pub archive_at: Option<DateTime<Utc>>,
}
pub struct EventPage { pub items: Vec<EventItem>, pub page: u32, pub has_more: bool }
```

`MtsEventProvider` calls `/vdome-smart-facade/api/v1/event` with `page`, `size`, `from`, and `to`, formats provider timestamps as UTC `yyyy-MM-dd'T'HH:mm:ss.SSS`, bounds content and nested collections, and retains only the explicit identity/status fields required for normalization.

- [ ] **Step 4: Implement owned scanning and the Axum route**

`EventService` validates the flat with `PropertyService`, gets `DeviceInventory`, builds a set from the flat ID, access-point IDs, and camera IDs, then scans newest-first provider pages until it has skipped `page * size` owned matches and collected `size + 1` matches or reached the bounded scan limit. It emits `hasMore` without exposing upstream totals. The route rejects size outside `1..=50`, page above `20`, reversed timestamps, future `to`, and ranges above 31 days.

- [ ] **Step 5: Run GREEN and commit**

```bash
cargo test --features test-support --test events
cargo test --all-targets
git add Cargo.toml src/domain src/provider/mts/events.rs src/provider/mts/mod.rs src/service/events.rs src/service/mod.rs src/api/events.rs src/api/mod.rs src/state.rs src/lib.rs tests/events.rs
git commit -m "feat: add MTS event journal API"
```

### Task 2: Owned camera archive through local HLS

**Files:**
- Create: `src/domain/archive.rs`
- Create: `src/provider/mts/archive.rs`
- Create: `src/service/archive.rs`
- Create: `tests/archive.rs`
- Modify: `src/domain/mod.rs`
- Modify: `src/provider/mts/mod.rs`
- Modify: `src/service/mod.rs`
- Modify: `src/api/intercom.rs`
- Modify: `src/app.rs`
- Modify: `src/state.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Produces `ArchiveInterval { from, to }` and `ArchiveDistribution { intervals }`.
- Produces `ArchiveProvider::distribution(camera_id)` and `ArchiveProvider::source(camera_id, start, end)`.
- Produces `ArchiveService::distribution(flat, camera)` and `ArchiveService::start(flat, camera, start)`.
- Produces `GET /api/v1/cameras/{cameraId}/archive/distribution?flatId=` and `POST /api/v1/cameras/{cameraId}/archive?flatId=` with `{ "start": "RFC3339" }`.

- [ ] **Step 1: Write failing archive contract and route tests**

Assert exact escaped camera paths, exact UTC millisecond `start`/`end` query values, selection of only the requested camera entry, sorted non-overlapping intervals, rejection of malformed/reversed/oversized distributions, and absence of RTSP in public JSON:

```rust
assert_eq!(distribution.intervals, vec![ArchiveInterval { from, to }]);
assert_eq!(session.playlist_url, format!("/api/v1/live/{}/index.m3u8", session.session_id));
assert!(!body.contains("rtsp"));
```

The production mutations caught are path injection, wrong time formatting, foreign-camera selection, missing ownership checks, interval overflow, and media-secret leakage.

- [ ] **Step 2: Run RED**

```bash
cargo test --features test-support --test archive
```

Expected: compilation fails because archive types and routes do not exist.

- [ ] **Step 3: Implement provider parsing and archive service**

Parse APK-compatible response shapes `{"list":[{"id","distribution":[{"from","to"}]}]}` and `{"list":[{"id","rtsp"}]}`. Require a matching camera ID, at most 2,048 valid intervals, and one bounded RTSP/RTSPS/HTTPS source. `ArchiveService` proves flat linkage and camera ownership/capability, requires `start` inside a recorded interval, clamps `end` to `min(start + 30 minutes, interval.to)`, and starts `LiveSessionManager` with a key containing flat, camera, start, and end.

- [ ] **Step 4: Register routes and redact archive paths**

Add the two archive handlers beside live camera handlers, reuse the existing opaque HLS asset/stop routes, set `Cache-Control: no-store`, and extend request metadata redaction so camera IDs and archive query timestamps are not logged.

- [ ] **Step 5: Run GREEN and commit**

```bash
cargo test --features test-support --test archive
cargo test --features test-support --test live_session
cargo test --all-targets
git add Cargo.toml src/domain src/provider/mts/archive.rs src/provider/mts/mod.rs src/service/archive.rs src/service/mod.rs src/api/intercom.rs src/app.rs src/state.rs src/lib.rs tests/archive.rs
git commit -m "feat: add camera archive playback API"
```

### Task 3: Encrypted and deduplicated auto-open runtime

**Files:**
- Create: `src/domain/auto_open.rs`
- Create: `src/service/auto_open.rs`
- Create: `tests/auto_open.rs`
- Modify: `src/domain/mod.rs`
- Modify: `src/service/mod.rs`
- Modify: `src/service/calls.rs`
- Modify: `src/api/calls.rs`
- Modify: `src/api/auth.rs`
- Modify: `src/app.rs`
- Modify: `src/state.rs`
- Modify: `src/storage.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Produces `AutoOpenMode`, `AutoOpenPolicyView`, `AutoOpenIntentView`, and `AutoOpenResultView`.
- Produces `AutoOpenService::{policy, create_intent, execute_intent, disable, reserve_call, complete}`.
- Adds internal `CallService::auto_open(&CallId)` that allows only a currently ringing call and never creates a browser intent.
- Produces `GET /api/v1/calls/auto-open`, `POST /api/v1/calls/auto-open/intents`, `POST /api/v1/calls/auto-open/intents/{id}/execute`, and `DELETE /api/v1/calls/auto-open`.

- [ ] **Step 1: Write failing state-machine and storage tests**

Use a fake clock, temporary `SecretStore`, and synthetic `CallId`s. Cover disabled mode, intent expiry/one-use, next-call atomic consumption, 30-minute expiry, duplicate reservation, concurrent reservation, restart, session mismatch, logout clear, failed/ambiguous completion, and dismiss-after-success. Assert the persisted file is encrypted and never contains call IDs or mode text in plaintext.

```rust
let reservations = join_all((0..8).map(|_| service.reserve_call(call.clone()))).await;
assert_eq!(reservations.into_iter().filter(Result::is_ok).count(), 1);
assert_eq!(opener.calls(), 0); // reservation itself has no physical side effect
```

The production mutations caught are default-on behavior, intent reuse, reserving after expiry, duplicate dispatch, persistence after logout/session replacement, retry after unknown outcome, and dismissing a failed unlock.

- [ ] **Step 2: Run RED**

```bash
cargo test --features test-support --test auto_open
```

Expected: compilation fails because the auto-open state machine and storage methods do not exist.

- [ ] **Step 3: Implement encrypted session-bound policy state**

Add `auto-open.enc` with AAD `domru-mts/auto-open/v1`, the same owner-only atomic write rules as other secret files, and methods that load/save bounded bytes and clear the file. Persist the authentication `authenticated_at` value as the session binding, the mode/deadline, `dismissAfterOpen`, and a bounded dedupe set. Any corrupt, expired, anonymous, or mismatched state is disabled and cleared.

- [ ] **Step 4: Implement one-use intents and runtime dispatch**

`next_call` is persisted as disabled before returning one reservation. `until` persists the call ID before returning a reservation. Add a cancellation-aware watcher in `main.rs` that polls the real `CallService` for a newly ringing call, reserves it once, calls `CallService::auto_open`, records the safe result, and invokes `ignore` only after a definitive successful open when `dismissAfterOpen` is true. It never retries a failed or unknown unlock.

- [ ] **Step 5: Register API, logout clearing, redaction, then run GREEN**

The POST intent body is exactly:

```json
{ "mode": "next_call", "dismissAfterOpen": true }
```

or:

```json
{ "mode": "until", "dismissAfterOpen": false }
```

The server fixes `until` to 30 minutes. Logout disables the in-memory policy and clears its encrypted file before ending the authenticated session.

```bash
cargo test --features test-support --test auto_open
cargo test --features test-support --test call_api
cargo test --features test-support --test storage
cargo test --all-targets
git add Cargo.toml src/domain src/service/auto_open.rs src/service/mod.rs src/service/calls.rs src/api/calls.rs src/api/auth.rs src/app.rs src/state.rs src/storage.rs src/main.rs src/lib.rs tests/auto_open.rs
git commit -m "feat: add bounded auto-open policies"
```

### Task 4: Journal, call history, archive, and auto-open UI

**Files:**
- Create: `frontend/src/features/events/eventApi.ts`
- Create: `frontend/src/features/events/eventApi.test.ts`
- Create: `frontend/src/features/events/EventJournal.tsx`
- Create: `frontend/src/features/events/EventJournal.test.tsx`
- Create: `frontend/src/features/intercom/ArchivePlayer.tsx`
- Create: `frontend/src/features/intercom/ArchivePlayer.test.tsx`
- Create: `frontend/src/features/calls/AutoOpenPanel.tsx`
- Create: `frontend/src/features/calls/AutoOpenPanel.test.tsx`
- Modify: `frontend/src/features/intercom/intercomApi.ts`
- Modify: `frontend/src/features/intercom/intercomApi.test.ts`
- Modify: `frontend/src/features/calls/callApi.ts`
- Modify: `frontend/src/features/calls/callApi.test.ts`
- Modify: `frontend/src/app/App.tsx`
- Modify: `frontend/src/app/App.test.tsx`
- Modify: `frontend/src/app/styles.css`

**Interfaces:**
- Produces `EventApi.getEvents`, `EventJournal`, and compact/full/calls variants.
- Extends optional `IntercomApi` archive methods with `ArchiveDistribution` and existing `LiveSession`.
- Extends optional `CallApi` auto-open methods and renders `AutoOpenPanel`.
- App owns event-to-archive navigation and the global armed-policy banner.

- [ ] **Step 1: Write failing client and component tests**

Assert URL encoding, no raw response rendering, journal loading/error/empty/items/filter/paging states, calls-only history, home latest three, camera-event navigation, archive interval selection, HLS/audio-capable `<video controls>`, start/stop cleanup, three auto-open modes, confirmation-before-arm, immediate disable, logout reset, and the global armed banner.

```tsx
expect(await screen.findByRole('heading', { name: 'Журнал событий' })).toBeVisible()
expect(screen.getByText('Пропущенный вызов')).toBeVisible()
await user.click(screen.getByRole('button', { name: 'Открыть запись камеры' }))
expect(await screen.findByRole('button', { name: 'Запустить архив' })).toBeVisible()
expect(autoOpenApi.executeAutoOpenIntent).not.toHaveBeenCalled()
```

The production mutations caught are wrong filters, leaking raw errors, archive mode not stopping sessions, event navigation losing timestamp/camera, arming without confirmation, and stale policy after logout.

- [ ] **Step 2: Run RED**

```bash
npm --prefix frontend test -- --run src/features/events src/features/intercom/ArchivePlayer.test.tsx src/features/calls/AutoOpenPanel.test.tsx src/app/App.test.tsx
```

Expected: imports/components/methods are missing.

- [ ] **Step 3: Implement typed local clients and focused components**

`eventApi` sends bounded encoded query parameters. `EventJournal` groups by date, uses safe Russian labels, and only exposes archive navigation when `cameraId` and `archiveAt` are present. `ArchivePlayer` displays recorded ranges, uses `datetime-local`, starts only from an owned returned range, attaches native HLS or hls.js, keeps controls/audio enabled, and always stops the opaque session on cleanup. `AutoOpenPanel` creates an intent before showing the final modal, executes only from that modal, and disables without confirmation.

- [ ] **Step 4: Integrate all real states into App**

Replace the Events placeholder, render the latest three events on Home, render calls-only history below `CallPage`, add Live/Archive controls to `CameraPanel`, and route camera events to the selected camera/time. Poll the safe auto-open policy while authenticated, show its banner on every page when armed, and clear it on logout. Remove the stale copy that says live/archive transport is disconnected.

- [ ] **Step 5: Run GREEN and commit**

```bash
npm --prefix frontend test -- --run
npm --prefix frontend run lint
npm --prefix frontend run typecheck
npm --prefix frontend run build
git add frontend/src
git commit -m "feat: add intercom history archive and auto-open UI"
```

### Task 5: Full verification and safe local handoff

**Files:**
- No source changes expected.

**Interfaces:**
- Consumes all new local routes and the rebuilt `frontend/dist`.
- Produces a verified UI at `http://127.0.0.1:3000` without external browser launch or physical mutations.

- [ ] **Step 1: Run complete quality gates**

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
npm --prefix frontend test -- --run
npm --prefix frontend run lint
npm --prefix frontend run typecheck
npm --prefix frontend run build
```

- [ ] **Step 2: Run read-only loopback smoke**

Check `/health/live`, `/api/v1/events` for the selected flat, and archive distribution for an owned archive-capable camera. A `401`, empty journal, or provider-unavailable response is reported as real provider state; no synthetic data is substituted. Do not POST archive playback if read-only discovery cannot prove a valid interval, and never call door/open or auto-open arming routes.

- [ ] **Step 3: Review repository state**

```bash
git diff --check
git status --short
git log -8 --oneline
```

Confirm no data directory, token, callback JSON, media URL, APK decompile, or generated secret-bearing artifact is tracked.
