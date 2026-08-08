# Default Camera Stream Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `GET /api/v1/cameras/stream`, an unauthenticated local URL that selects the default live-capable camera and redirects to its short-lived local HLS playlist.

**Architecture:** The Axum handler will reuse `PropertyService`, `DeviceService`, the existing `LiveSourceProvider`, and `LiveSessionManager`; it will not introduce a second media pipeline. Selection stays in the API layer, while a private helper shared with the existing camera-specific POST route owns source resolution and session startup. A dedicated integration-test fixture injects deterministic in-memory providers and a fake local media runner.

**Tech Stack:** Rust 1.96, Axum 0.8, Tokio, Tower integration tests, `secrecy`, UUID, Markdown.

## Global Constraints

- The public contract is exactly `GET /api/v1/cameras/stream`; it requires no query string, request body, cookies, `Authorization` header, or provider token from the caller.
- A valid stored MTS/VDome provider session is still required inside the backend.
- Prefer a valid `selectedFlatId`; otherwise use the first dashboard flat. Never fall back to another flat when the chosen flat has no live-capable camera.
- Within the chosen flat, use the first camera with `Capability::Available` after the existing camera-ID sort.
- Success is `307 Temporary Redirect` to `/api/v1/live/<uuid>/index.m3u8` with `Cache-Control: private, no-store` and an empty body.
- Responses must not expose provider tokens, private media URLs, flat IDs, or camera IDs.
- Missing flat or live camera is `404 not_found`; existing authentication and provider errors retain their current envelope and status.
- The route is safe only under the existing loopback-only bind. Any process or browser origin that can reach the loopback port can invoke it.
- Preserve the JSON contract of `POST /api/v1/cameras/{camera_id}/live?flatId=...`.
- Keep the branch exactly one commit ahead of `main` by using `git commit --amend --no-edit`; never add a co-author trailer.

---

### Task 1: Build the default-stream contract test harness

**Files:**
- Modify: `Cargo.toml:74-83`
- Modify: `src/state.rs:106-147`
- Create: `tests/default_camera_stream.rs`

**Interfaces:**
- Consumes: `PropertyService`, `DeviceService`, `LiveSourceProvider`, `LiveSessionManager`, and `api::router()`.
- Produces: feature-gated `AppState::with_intercom_video_services(property_service, device_service, live_source_provider, live_session_manager) -> AppState` test support and an integration-test fixture able to exercise real Axum responses and local HLS assets.

- [ ] **Step 1: Register the feature-gated integration test target**

Add this target beside `live_session` in `Cargo.toml`:

```toml
[[test]]
name = "default_camera_stream"
path = "tests/default_camera_stream.rs"
required-features = ["test-support"]
```

- [ ] **Step 2: Write the fixture against the wished-for state constructor**

Create `tests/default_camera_stream.rs` with concrete fakes:

```rust
struct TestProperties {
    flats: Vec<Flat>,
}

struct TestDevices {
    cameras: BTreeMap<FlatId, Vec<Camera>>,
}

struct ExpectedLiveSource {
    expected_flat: FlatId,
    expected_camera: String,
    result: SourceResult,
    calls: AtomicUsize,
}

enum SourceResult {
    Ready,
    SessionExpired,
}

#[derive(Default)]
struct TestRunner {
    starts: AtomicUsize,
}
```

Implement the real provider traits. `TestRunner::start` must write this literal playlist and segment into the supplied output directory before returning a process that waits forever:

```text
#EXTM3U
#EXTINF:1,
segment-000001.ts
```

Build the router with:

```rust
let state = AppState::with_intercom_video_services(
    properties,
    devices,
    live_source,
    Arc::clone(&sessions),
);
let app = with_http_middleware(api::router().with_state(state));
```

When a selected flat is needed, call `PropertyService::select_flat` before constructing the state. Keep the `TempDir` and `LiveSessionManager` in the fixture, and call `stop_all()` at the end of successful-session tests.

- [ ] **Step 3: Run the fixture test target and verify the constructor is missing**

Run: `cargo test --all-features --test default_camera_stream`

Expected: compilation fails because `AppState::with_intercom_video_services` does not exist. This proves the test fixture is using the intended injection boundary.

- [ ] **Step 4: Add the minimal state constructor**

Add this constructor to `impl AppState` without changing `with_dependencies` or `with_core_services`:

```rust
#[cfg(feature = "test-support")]
pub fn with_intercom_video_services(
    property_service: Arc<PropertyService>,
    device_service: Arc<DeviceService>,
    live_source_provider: Arc<dyn LiveSourceProvider>,
    live_session_manager: Arc<LiveSessionManager>,
) -> Self {
    Self {
        property_service: Some(property_service),
        device_service: Some(device_service),
        live_source_provider: Some(live_source_provider),
        live_session_manager: Some(live_session_manager),
        ..Self::default()
    }
}
```

- [ ] **Step 5: Verify the fixture compiles before endpoint assertions are added**

Run: `cargo test --all-features --test default_camera_stream --no-run`

Expected: PASS.

---

### Task 2: Drive camera selection and redirect behavior test-first

**Files:**
- Modify: `tests/default_camera_stream.rs`
- Modify: `src/api/intercom.rs:20-46,151-195`

**Interfaces:**
- Consumes: the Task 1 fixture and the existing deterministic sorting in `DeviceService::inventory`.
- Produces: `start_default_live(...) -> Result<Response<Body>, ApiError>`, `start_live_session(&AppState, RequestId, &FlatId, &str) -> Result<LiveSessionView, ApiError>`, and the registered GET route.

- [ ] **Step 1: Add all observable endpoint tests before production code**

Add these tests with literal expectations:

```rust
#[tokio::test]
async fn redirects_an_empty_request_to_the_first_flats_first_live_camera() {
    // Two flats, no selection. The first flat has one live camera.
    // Send Request::get("/api/v1/cameras/stream") with Body::empty() and no added headers.
    // Assert 307, Cache-Control "private, no-store", empty body, and a Location matching
    // /api/v1/live/<valid UUID>/index.m3u8. Follow Location and assert the playlist is 200,
    // private/no-store, and contains only the local segment name.
}

#[tokio::test]
async fn selected_flat_wins_and_unavailable_cameras_are_skipped_in_id_order() {
    // Select flat-b. Return its cameras in z-live, a-unavailable, b-live order.
    // Expect the successful source lookup to be exactly flat-b / b-live.
}

#[tokio::test]
async fn selected_flat_without_live_video_does_not_fall_back_to_another_flat() {
    // Give flat-a a live camera and selected flat-b only unavailable cameras.
    // Assert 404 not_found with "Live video is unavailable." and zero source calls.
}

#[tokio::test]
async fn no_linked_flat_returns_the_stable_not_found_envelope() {
    // Return an empty dashboard and assert 404 not_found with "Linked flat was not found.".
}

#[tokio::test]
async fn provider_session_errors_keep_their_existing_envelope() {
    // Let source resolution return ErrorCode::SessionExpired.
    // Assert 401 session_expired and that neither provider source nor selected IDs appear in body.
}

#[tokio::test]
async fn non_get_method_uses_the_method_not_allowed_envelope() {
    // POST the stable route and assert 405 method_not_allowed.
}
```

In both success tests, assert that the `Location`, headers, and body do not contain these fixture markers:

```rust
[
    "provider-access-secret",
    "rtsp://provider.invalid/private-source",
    "flat-a",
    "flat-b",
    "a-unavailable",
    "b-live",
    "z-live",
]
```

Request the stable URL twice in the first success test; assert both redirects use the same `Location` and `TestRunner::starts == 1`, proving active-session reuse.

- [ ] **Step 2: Run the tests and verify RED**

Run: `cargo test --all-features --test default_camera_stream -- --nocapture`

Expected: endpoint tests fail because `/api/v1/cameras/stream` is not registered (success cases receive 404, the session-error case receives 404 instead of 401, and POST receives 404 instead of 405).

- [ ] **Step 3: Register the route and implement minimal selection**

Register the static route before camera-ID routes:

```rust
.route("/api/v1/cameras/stream", get(start_default_live))
```

Implement the handler with the existing services:

```rust
async fn start_default_live(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response<Body>, ApiError> {
    let dashboard = load_dashboard(&state, request_id).await?;
    let flat_id = dashboard
        .selected_flat_id
        .filter(|selected| dashboard.flats.iter().any(|flat| &flat.id == selected))
        .or_else(|| dashboard.flats.first().map(|flat| flat.id.clone()))
        .ok_or_else(|| ApiError::new(request_id, linked_flat_not_found()))?;
    let inventory = load_inventory(&state, request_id, flat_id.0.clone()).await?;
    let camera = inventory
        .cameras
        .into_iter()
        .find(|camera| matches!(camera.capabilities.live, Capability::Available))
        .ok_or_else(|| ApiError::new(request_id, live_video_not_found()))?;
    let view = start_live_session(&state, request_id, &flat_id, &camera.id).await?;
    temporary_live_redirect(request_id, view)
}
```

Extract `linked_flat_not_found()` and `live_video_not_found()` only if they remove duplicate error construction. `temporary_live_redirect` must derive the exact local path from `view.session_id`, verify it equals `view.playlist_url`, and use `Response::builder()` to emit status 307, `Location`, `Cache-Control: private, no-store`, and `Body::empty()`.

Place only source resolution, the collision-safe camera key, and `LiveSessionManager::start` in `start_live_session` for the new handler. Leave the existing POST handler unchanged until its current behavior is characterized in Task 3; ownership and live-capability checks stay in the handlers that already possess their respective `Camera` values.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run: `cargo test --all-features --test default_camera_stream -- --nocapture`

Expected: all default-camera-stream tests PASS.

- [ ] **Step 5: Format, inspect, and amend the single commit**

Run:

```shell
cargo fmt --check
git diff --check
git add Cargo.toml src/state.rs src/api/intercom.rs tests/default_camera_stream.rs docs/superpowers/specs/2026-08-08-default-camera-stream-design.md docs/superpowers/plans/2026-08-08-default-camera-stream.md
git commit --amend --no-edit
```

Expected: the branch remains one commit ahead of `main` and the commit message has no co-author trailer.

---

### Task 3: Preserve the existing camera-specific live contract during refactor

**Files:**
- Modify: `tests/default_camera_stream.rs`
- Modify: `src/api/intercom.rs:155-195`

**Interfaces:**
- Consumes: `start_live_session` from Task 2.
- Produces: unchanged JSON output for `POST /api/v1/cameras/{camera_id}/live?flatId=...`.

- [ ] **Step 1: Add a characterization test before changing the POST handler**

Add:

```rust
#[tokio::test]
async fn camera_specific_live_post_keeps_its_json_contract() {
    // POST /api/v1/cameras/b-live/live?flatId=flat-b.
    // Assert 200 application/json, a valid UUID in sessionId, and the exact corresponding
    // /api/v1/live/<uuid>/index.m3u8 value in playlistUrl. Assert the JSON contains no source URL,
    // provider token, flat ID, or camera ID.
}
```

- [ ] **Step 2: Run the characterization test before refactoring**

Run: `cargo test --all-features --test default_camera_stream camera_specific_live_post_keeps_its_json_contract -- --exact`

Expected: PASS against the existing POST behavior plus Task 2's injected services.

- [ ] **Step 3: Finish the shared-helper refactor**

Ensure `start_live` performs its existing opaque-ID validation, linked-flat ownership check, camera lookup, and live-capability check, then calls:

```rust
let view = start_live_session(&state, request_id, &flat_id, &camera_id).await?;
Ok(Json(view))
```

Delete the duplicated source-resolution and manager-start block from `start_live`. Do not change its status, request query, or JSON type.

- [ ] **Step 4: Verify both contracts together**

Run: `cargo test --all-features --test default_camera_stream`

Expected: all tests PASS, including the 307 GET contract and existing 200 JSON POST contract.

- [ ] **Step 5: Amend the same commit**

Run:

```shell
git add src/api/intercom.rs tests/default_camera_stream.rs
git commit --amend --no-edit
```

Expected: still exactly one commit ahead of `main`.

---

### Task 4: Document the player URL and its security boundary

**Files:**
- Modify: `README.md:69-100`

**Interfaces:**
- Consumes: the final HTTP contract from Tasks 2 and 3.
- Produces: copy-pasteable playback instructions and an explicit no-auth warning.

- [ ] **Step 1: Add the canonical endpoint to the API table**

Add this row before the camera-specific routes:

```markdown
| `/api/v1/cameras/stream` | GET | Redirect to the selected flat's first live-capable camera |
```

- [ ] **Step 2: Add a minimal playback example**

Immediately after the table, add:

````markdown
Open the default camera stream in a player that follows HTTP redirects:

```shell
ffplay http://127.0.0.1:3000/api/v1/cameras/stream
```

The request needs no local authorization header or token. The backend still needs a valid stored provider session and returns a short-lived local HLS URL.
````

- [ ] **Step 3: Strengthen the loopback warning**

State that any local process able to reach the port can invoke the stable stream URL, and that the service must not be placed behind an unauthenticated public bind or reverse proxy.

- [ ] **Step 4: Check documentation and amend**

Run:

```shell
git diff --check
git add README.md
git commit --amend --no-edit
```

Expected: `git diff --check` passes and the branch remains one commit ahead. Repeat the previously established private-address audit directly against the resulting tree without adding those private markers to repository files.

---

### Task 5: Run final verification and audit the single commit

**Files:**
- Verify only: entire repository and final commit

**Interfaces:**
- Consumes: all implementation and documentation tasks.
- Produces: evidence that backend, frontend, secret-redaction, and one-commit constraints remain intact.

- [ ] **Step 1: Run backend verification**

Run:

```shell
cargo fmt --check
cargo test --all-features
```

Expected: formatting passes and every Rust test passes; only already-known vendored generated-code warnings are acceptable.

- [ ] **Step 2: Run frontend verification**

Run:

```shell
npm --prefix frontend test
npm --prefix frontend run lint
npm --prefix frontend run build
```

Expected: all frontend tests and lint pass; production build completes, with only the existing Vite chunk-size warning acceptable.

- [ ] **Step 3: Audit leaks, patch shape, and commit topology**

Run:

```shell
git diff --check main..HEAD
git grep -n -I -E 'provider-access-secret|rtsp://provider\.invalid/private-source' HEAD -- ':!tests/default_camera_stream.rs' ':!docs/superpowers/plans/2026-08-08-default-camera-stream.md'
git log -1 --format='%B'
git rev-list --count main..HEAD
git status --short --branch
```

Expected: no forbidden address or test-secret marker outside the test fixture, no co-author trailer, exactly `1` commit ahead of `main`, and a clean worktree.

- [ ] **Step 4: Amend any verification-only formatting changes, then re-run the affected checks**

If `cargo fmt --check` requests formatting, run `cargo fmt`, stage only task files, amend with `git commit --amend --no-edit`, and rerun every check affected by the formatting. Do not create a second commit.
