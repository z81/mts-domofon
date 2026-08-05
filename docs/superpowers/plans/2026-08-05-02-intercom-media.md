# VDome Intercom, Cameras, Archive, and Events Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add typed flats, dashboard, access-point, intercom, camera, event, live-media, archive, per-flat device favorites, unread badges, call-setting, and confirmed lock-control capabilities to the authenticated service and Web UI.

**Architecture:** Provider modules deserialize VDome responses into private DTOs and immediately map them to stable domain types. Read-only device/media operations are exposed through narrow services; media is fetched through an allowlisted proxy that rewrites HLS references. Lock control crosses a dedicated mutation boundary that requires a short-lived server-issued confirmation intent and never retries an ambiguous upstream result.

**Tech Stack:** The phase-1 Rust/Axum stack; `m3u8-rs` 6.0.1 for playlist validation, `tokio-util` for bounded streaming, `mime`/`http` for response metadata; React 19.2.8, TypeScript 6.0.3, Vitest 4.1.10, and Playwright 1.62.1.

## Global Constraints

- Complete phase 1 first and preserve its `MtsClient`, `AppError`, request-ID, token-redaction, storage, and embedded-SPA contracts.
- Use the observed current routes: `/vdome-smart-facade/estate/api/v1/residents/flats`, `/vdome-smart-facade/devices/api/v1/accessPoints`, `/vdome-smart-facade/devices/api/v1/cameras`, `/vdome-smart-facade/devices/api/v1/cameras/{id}/snapshots`, `/domofon/api/intercom/v2/intercoms`, `/domofon/api/intercom/v1/cameras`, `/api/new/v1/company/elevator`, `/vdome-smart-facade/api/v1/event`, and `/domofon/api/intercom/v1/cameras/{id}/archive`.
- Scope every flat-dependent read and mutation by a flat that belongs to the active session; never trust a browser-supplied upstream URL.
- Accept media URLs only when they are returned for an authorized camera and match `https` plus the configured host allowlist; block private/link-local destinations after DNS resolution.
- Bound manifests, snapshots, JSON bodies, archive ranges, segment sizes, upstream concurrency, and all request timeouts.
- Never put bearer/device tokens or reusable signed media credentials in HTML, JSON, browser storage, logs, redirects, or error bodies.
- Never retry a lock mutation after any bytes may have reached VDome. Require a fresh one-use intent tied to flat, access point, lock, action, and local session.
- Ask the user immediately before the first real favorite, call-setting/blocking, event-deletion, lock, barrier, or elevator mutation; mock mutations need no confirmation outside the application flow.
- Use only redacted synthetic fixtures and original UI assets; do not copy APK code or artwork.
- Do not include `Co-Authored-By` trailers or any Codex/Anthropic co-author line in commits.
- When a task creates a Rust module, update its nearest existing `mod.rs` (and `src/lib.rs` only for a new top-level module) in the same task and include that registry file in the task commit.

---

## Locked File Map

- `src/domain/property.rs`: flat, capability, dashboard, favorite, and call-setting domain types.
- `src/domain/device.rs`: access-point, intercom, lock, and camera domain types.
- `src/domain/media.rs`: snapshot/live/archive/event query and result types.
- `src/provider/mts/dashboard.rs`: flats, services, favorites, and call-settings provider calls only.
- `src/provider/mts/intercom.rs`: access points, legacy intercom enrichment, cameras, and lock calls only.
- `src/provider/mts/media.rs`: snapshot/live/archive provider discovery calls only.
- `src/provider/mts/events.rs`: event query/delete provider calls only.
- `src/service/property.rs`: selected-flat validation and dashboard composition only.
- `src/service/devices.rs`: device aggregation and capability filtering only.
- `src/service/media.rs`: camera ownership, media lookup, and event orchestration only.
- `src/service/mutations.rs`: confirmation-intent issuance/consumption only.
- `src/media/allowlist.rs`: outbound URL and resolved-address policy only.
- `src/media/hls.rs`: playlist parsing/rewriting only.
- `src/media/proxy.rs`: bounded authorized streaming only.
- `src/api/dashboard.rs`, `devices.rs`, `media.rs`, `events.rs`: corresponding local HTTP transport only.
- `frontend/src/features/home/*`, `devices/*`, `cameras/*`: corresponding screens and components only.

### Task 1: Stable Property and Device Domain Models

**Files:**
- Modify: `src/domain/mod.rs`
- Create: `src/domain/property.rs`
- Create: `src/domain/device.rs`
- Create: `src/domain/page.rs`
- Modify: `src/lib.rs`
- Test: `tests/domain_property.rs`

**Interfaces:**

```rust
pub struct FlatId(pub String);
pub struct Page<T> { pub items: Vec<T>, pub next_cursor: Option<String>, pub total: Option<u64> }
pub struct Flat { pub id: FlatId, pub label: String, pub address: String, pub capabilities: CapabilitySet }
pub enum AccessPointKind { Intercom, Barrier, Gate, Elevator, Door, Unknown(String) }
pub struct AccessPoint { pub id: String, pub flat_id: FlatId, pub name: String, pub kind: AccessPointKind, pub locks: Vec<Lock>, pub camera_ids: Vec<String>, pub available: bool }
pub struct Camera { pub id: String, pub flat_id: FlatId, pub name: String, pub access_point_id: Option<String>, pub capabilities: CameraCapabilities }
pub enum FavoriteTarget { AccessPoint(String), Camera(String), Elevator(String), Unknown { kind: String, id: String } }
```

- [ ] **Step 1: Write serialization and unknown-enum tests** in `tests/domain_property.rs`, covering camel-case JSON, an unknown access-point category, multi-lock devices, and a capability that is unavailable with a reason.
- [ ] **Step 2: Run the focused test and confirm it fails:** `cargo test --test domain_property -- --nocapture`.
- [ ] **Step 3: Implement newtypes and provider-neutral models** with `serde`, `#[non_exhaustive]` where consumers may match variants, and explicit `Unknown(String)` preservation.
- [ ] **Step 4: Run quality checks:** `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --test domain_property`.
- [ ] **Step 5: Commit:** `git add src/domain src/lib.rs tests/domain_property.rs && git commit -m "feat: add property and device domain models"`.

### Task 2: Flats, Dashboard Capabilities, Favorites, and Call Settings

**Files:**
- Create: `src/provider/mts/dashboard.rs`
- Create: `src/provider/mts/dto/dashboard.rs`
- Modify: `src/provider/mts/dto/mod.rs`
- Create: `src/service/property.rs`
- Create: `src/service/mod.rs`
- Create: `tests/fixtures/flats.json`
- Create: `tests/fixtures/flat_services.json`
- Create: `tests/fixtures/favorites.json`
- Create: `tests/contract/dashboard_provider.rs`

**Interfaces:**

```rust
#[async_trait]
pub trait PropertyProvider: Send + Sync {
    async fn flats(&self) -> Result<Vec<Flat>, AppError>;
    async fn services(&self, flat: &FlatId) -> Result<CapabilitySet, AppError>;
    async fn favorites(&self, flat: &FlatId) -> Result<Vec<Favorite>, AppError>;
    async fn set_favorite(&self, flat: &FlatId, alias: &str, enabled: bool) -> Result<(), AppError>;
    async fn call_settings(&self, flat: &FlatId) -> Result<CallSettings, AppError>;
    async fn update_call_settings(&self, flat: &FlatId, input: CallSettingsPatch) -> Result<CallSettings, AppError>;
}
```

- [ ] **Step 1: Write Wiremock contract tests** asserting query/path/body behavior for `GET /vdome-smart-facade/estate/api/v1/residents/flats`, `GET /vdome-smart-facade/services/api/v1/residents/flats/{id}`, favorites under `/vdome-smart-facade/devices/api/v1/favorites`, call settings under `/vdome-smart-facade/devices/api/v1/call-settings`, and per-apartment block/unblock through `POST|DELETE /gw-intercom/apartment/v1/apartments/{apartmentId}/block/{userId}` when that legacy capability is returned. Cover add/remove/reorder-safe favorite selection for cameras, intercom/access points, and elevators, unknown target kinds, per-flat local fallback matching the APK's main-settings selection, unread/event badge aggregation, and optional/additive fields.
- [ ] **Step 2: Run and observe failure:** `cargo test --test dashboard_provider -- --nocapture`.
- [ ] **Step 3: Implement DTO mapping and `PropertyService`**, including selected-flat validation, per-flat favorite target ownership, a local revisioned fallback when the provider does not persist a supported favorite kind, unread/event badge counts, and per-capability failures represented as unavailable data rather than a failed dashboard.
- [ ] **Step 4: Run focused and full backend tests:** `cargo test --test dashboard_provider && cargo test`.
- [ ] **Step 5: Commit:** `git add src/provider/mts src/service tests/fixtures tests/contract/dashboard_provider.rs && git commit -m "feat: add flats and dashboard provider"`.

### Task 3: Access Points, Intercom Enrichment, and Cameras

**Files:**
- Create: `src/provider/mts/intercom.rs`
- Create: `src/provider/mts/dto/intercom.rs`
- Create: `src/service/devices.rs`
- Create: `tests/fixtures/access_points.json`
- Create: `tests/fixtures/intercoms.json`
- Create: `tests/fixtures/cameras.json`
- Create: `tests/contract/intercom_provider.rs`

**Interfaces:**

```rust
#[async_trait]
pub trait DeviceProvider: Send + Sync {
    async fn access_points(&self, flat: &FlatId) -> Result<Vec<AccessPoint>, AppError>;
    async fn intercoms(&self, flat: &FlatId) -> Result<Vec<IntercomDetails>, AppError>;
    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError>;
}

impl DeviceService {
    pub async fn inventory(&self, flat: &FlatId) -> Result<DeviceInventory, AppError>;
    pub async fn camera(&self, flat: &FlatId, camera_id: &str) -> Result<Camera, AppError>;
}
```

- [ ] **Step 1: Write contract and aggregation tests** for smart-facade access points/cameras, legacy `domofon/api/intercom/v2/intercoms?category=intercom|barrier`, `domofon/api/intercom/v1/cameras`, and optional resident-rest `/api/new/v1/company/elevator`; prove duplicate IDs merge, unknown categories survive, missing optional enrichment does not hide a usable access point, and cross-flat camera access returns `not_found`.
- [ ] **Step 2: Run and observe failure:** `cargo test --test intercom_provider -- --nocapture`.
- [ ] **Step 3: Implement the provider and `DeviceService`** with deterministic merge precedence: smart-facade identity/capability wins, legacy data may enrich labels, call metadata, lock numbers, and media identifiers only.
- [ ] **Step 4: Run checks:** `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --test intercom_provider`.
- [ ] **Step 5: Commit:** `git add src/provider/mts src/service tests/fixtures tests/contract/intercom_provider.rs && git commit -m "feat: add VDome device inventory"`.

### Task 4: Snapshot Proxy and Outbound Media Allowlist

**Files:**
- Modify: `Cargo.toml`
- Create: `src/domain/media.rs`
- Create: `src/provider/mts/media.rs`
- Create: `src/provider/mts/dto/media.rs`
- Create: `src/media/mod.rs`
- Create: `src/media/allowlist.rs`
- Create: `src/media/proxy.rs`
- Create: `src/service/media.rs`
- Create: `tests/media_allowlist.rs`
- Create: `tests/contract/snapshot_proxy.rs`

**Interfaces:**

```rust
pub struct MediaPolicy { pub allowed_hosts: BTreeSet<String>, pub max_snapshot_bytes: u64, pub connect_timeout: Duration, pub read_timeout: Duration }
pub trait AddressPolicy { fn permit(&self, url: &Url, resolved: &[IpAddr]) -> Result<(), AppError>; }
impl MediaService { pub async fn snapshot(&self, flat: &FlatId, camera_id: &str) -> Result<ProxiedMedia, AppError>; }
```

Add `mime = "0.3.17"` and `tokio-util = { version = "0.7.19", features = ["io"] }` before compiling the proxy.

- [ ] **Step 1: Write security-first tests** rejecting `http`, userinfo, fragments, unlisted hosts, redirects to a new host, loopback/private/link-local/multicast IPs, oversized payloads, non-image content, and a camera belonging to another flat; add success cases for an allowlisted direct image and authorized snapshot bytes.
- [ ] **Step 2: Run and observe failure:** `cargo test --test media_allowlist --test snapshot_proxy -- --nocapture`.
- [ ] **Step 3: Implement snapshot discovery** through `/vdome-smart-facade/devices/api/v1/cameras/{id}/snapshots`, ownership validation before provider access, manual redirect validation, streamed byte limits, safe content headers, and `Cache-Control: private, no-store`.
- [ ] **Step 4: Run the focused suite and clippy:** `cargo test --test media_allowlist --test snapshot_proxy && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit:** `git add Cargo.toml Cargo.lock src/domain/media.rs src/provider/mts src/media src/service/media.rs tests && git commit -m "feat: proxy authorized camera snapshots"`.

### Task 5: HLS Live Stream Proxy and Playlist Rewriting

**Files:**
- Modify: `Cargo.toml`
- Create: `src/media/hls.rs`
- Modify: `src/media/proxy.rs`
- Modify: `src/provider/mts/media.rs`
- Modify: `src/service/media.rs`
- Create: `tests/fixtures/media/master.m3u8`
- Create: `tests/fixtures/media/variant.m3u8`
- Create: `tests/hls_rewrite.rs`
- Create: `tests/contract/live_media.rs`

**Interfaces:**

```rust
pub struct MediaLeaseId(pub Uuid);
pub struct LiveSession { pub lease_id: MediaLeaseId, pub content_type: String, pub expires_at: DateTime<Utc> }
pub fn rewrite_playlist(input: &[u8], base: &Url, lease: &MediaLeaseId) -> Result<Vec<u8>, AppError>;
```

Add `m3u8-rs = "6.0.1"` before compiling the playlist module.

- [ ] **Step 1: Write playlist tests** covering master variants, relative/absolute segments, query strings, `EXT-X-KEY`, `EXT-X-MAP`, CRLF, comments, malformed manifests, and attempts to pivot to an unapproved host; write a contract test proving a lease is bound to session/camera and cannot address arbitrary URLs.
- [ ] **Step 2: Run and observe failure:** `cargo test --test hls_rewrite --test live_media -- --nocapture`.
- [ ] **Step 3: Implement media leases and rewriting** so local segment routes contain only opaque lease/resource IDs; store the upstream URL server-side with a short expiry, validate every nested URL, pass safe range headers, and stream with cancellation/backpressure.
- [ ] **Step 4: Run checks:** `cargo test --test hls_rewrite --test live_media && cargo test && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit:** `git add Cargo.toml Cargo.lock src/media src/provider/mts/media.rs src/service/media.rs tests && git commit -m "feat: proxy VDome live camera streams"`.

### Task 6: Event Timeline, Archive Playback, and Clip Download

**Files:**
- Modify: `Cargo.toml`
- Create: `src/provider/mts/events.rs`
- Create: `src/provider/mts/dto/events.rs`
- Modify: `src/provider/mts/media.rs`
- Modify: `src/service/media.rs`
- Create: `tests/fixtures/events_page.json`
- Create: `tests/fixtures/archive.json`
- Create: `tests/contract/events_archive.rs`

**Interfaces:**

```rust
pub struct EventQuery { pub flat_id: FlatId, pub camera_id: Option<String>, pub from: DateTime<Utc>, pub to: DateTime<Utc>, pub cursor: Option<String>, pub limit: u16 }
pub struct ArchiveQuery { pub flat_id: FlatId, pub camera_id: String, pub from: DateTime<Utc>, pub to: DateTime<Utc> }
impl MediaService {
    pub async fn events(&self, query: EventQuery) -> Result<Page<Event>, AppError>;
    pub async fn archive(&self, query: ArchiveQuery) -> Result<ArchiveSession, AppError>;
    pub async fn download_clip(&self, query: ArchiveQuery) -> Result<ProxiedMedia, AppError>;
    pub(crate) async fn delete_events(&self, flat: &FlatId, event_ids: Vec<String>) -> Result<(), AppError>; // called only after MutationService consumes a validated intent
}
```

Add `chrono-tz = "0.10.4"` so local archive inputs and DST edge cases are converted with IANA timezone rules.

- [ ] **Step 1: Write contracts** for list and body-bearing `DELETE /vdome-smart-facade/api/v1/event` plus `/domofon/api/intercom/v1/cameras/{id}/archive`; cover pagination, typed event-ID deletion without a blind retry, inclusive UTC normalization, DST input conversion, empty pages, unavailable archives, maximum 24-hour playback and 15-minute clip bounds, and cross-flat IDs.
- [ ] **Step 2: Run and observe failure:** `cargo test --test events_archive -- --nocapture`.
- [ ] **Step 3: Implement event mapping and archive sessions** using the same media leases/allowlist as live video; use `Content-Disposition: attachment` with a server-generated filename for downloads and never forward an upstream filename.
- [ ] **Step 4: Run checks:** `cargo test --test events_archive && cargo test && cargo fmt --check`.
- [ ] **Step 5: Commit:** `git add Cargo.toml Cargo.lock src/provider/mts src/service/media.rs tests && git commit -m "feat: add camera events and archive"`.

### Task 7: One-Use Confirmation Intents, Event Deletion, and Lock Control

**Files:**
- Create: `src/domain/mutation.rs`
- Create: `src/service/mutations.rs`
- Modify: `src/provider/mts/intercom.rs`
- Modify: `src/provider/mts/events.rs`
- Create: `tests/mutation_intents.rs`
- Create: `tests/contract/lock_control.rs`

**Interfaces:**

```rust
pub enum AccessAction { Open }
pub struct AccessIntentRequest { pub flat_id: FlatId, pub access_point_id: String, pub lock_number: Option<String>, pub action: AccessAction }
pub struct MutationIntent { pub id: Uuid, pub kind: MutationKind, pub summary: String, pub expires_at: DateTime<Utc> }
pub enum MutationKind { OpenAccessPoint, DeleteEvents }
impl MutationService {
    pub async fn issue_access_intent(&self, session: &LocalSessionId, input: AccessIntentRequest) -> Result<MutationIntent, AppError>;
    pub async fn execute_access_intent(&self, session: &LocalSessionId, intent: Uuid) -> Result<AccessResult, AppError>;
}
```

Later phases extend `MutationKind` and add typed issue/access/order/reading methods; they do not introduce an opaque payload or generic raw executor.

- [ ] **Step 1: Write intent tests** proving expiry after 60 seconds, one-use semantics under concurrency, binding to local session/flat/action payload, access-point/lock and event ownership, rejection when a device/event is unavailable, and no retry on timeout or upstream 5xx after dispatch.
- [ ] **Step 2: Write provider contracts** for `POST /vdome-smart-facade/devices/api/v1/intercoms/{intercomId}/lock`, the `/{lockNumber}` variant, and body-bearing `DELETE /vdome-smart-facade/api/v1/event`, including optional active-call metadata without logging it.
- [ ] **Step 3: Run and observe failure:** `cargo test --test mutation_intents --test lock_control -- --nocapture`.
- [ ] **Step 4: Implement intent storage and lock calls**, atomically consuming the intent before network dispatch and returning `mutation_outcome_unknown` for ambiguous transport results.
- [ ] **Step 5: Run checks and commit:** `cargo test --test mutation_intents --test lock_control && cargo clippy --all-targets --all-features -- -D warnings && git add src tests && git commit -m "feat: add confirmed lock control"`.

### Task 8: Versioned Device and Media HTTP API plus Compatibility Aliases

**Files:**
- Create: `src/api/dashboard.rs`
- Create: `src/api/devices.rs`
- Create: `src/api/media.rs`
- Create: `src/api/events.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/app.rs`
- Create: `tests/api/dashboard_devices.rs`
- Create: `tests/api/media.rs`
- Create: `tests/api/access_action.rs`

**Routes:**

```text
GET    /api/v1/flats
PUT    /api/v1/flats/selected
GET    /api/v1/dashboard?flatId={flatId}
GET    /api/v1/favorites?flatId={flatId}
PUT    /api/v1/favorites/:alias
GET    /api/v1/call-settings?flatId={flatId}
PUT    /api/v1/call-settings
GET    /api/v1/devices?flatId={flatId}
GET    /api/v1/cameras/:id/snapshot?flatId={flatId}
POST   /api/v1/cameras/:id/live
POST   /api/v1/cameras/:id/archive
GET    /api/v1/media/:lease/:resource
POST   /api/v1/cameras/:id/archive/download
GET    /api/v1/events
POST   /api/v1/events/delete-intents
POST   /api/v1/events/delete-intents/:id/execute
POST   /api/v1/access/intents
POST   /api/v1/access/intents/:id/execute
GET    /api/places
GET    /api/snapshot/:camera_id
GET    /api/stream/:camera_id
GET    /api/archive/:camera_id
POST   /api/open-door
```

- [ ] **Step 1: Write Axum tests** for every canonical route, schema, optimistic revision on favorite/call-setting writes, validation limit, ownership check, content type/range behavior, stable error envelope, compatibility alias, and secret non-disclosure; assert `/api/open-door` also uses an intent and cannot bypass confirmation.
- [ ] **Step 2: Run and observe failure:** `cargo test --test dashboard_devices --test media --test access_action -- --nocapture`.
- [ ] **Step 3: Implement thin handlers** that deserialize validated local DTOs, call services, and stream safe responses; add OpenAPI annotations without exposing provider DTOs.
- [ ] **Step 4: Run API and full tests:** `cargo test --test dashboard_devices --test media --test access_action && cargo test`.
- [ ] **Step 5: Commit:** `git add src/api src/app.rs tests/api && git commit -m "feat: expose device and media API"`.

### Task 9: Responsive Home, Device, Camera, and Event UI

**Files:**
- Create: `frontend/src/lib/api/devices.ts`
- Create: `frontend/src/features/home/HomePage.tsx`
- Create: `frontend/src/features/home/FlatSelector.tsx`
- Create: `frontend/src/features/devices/DevicesPage.tsx`
- Create: `frontend/src/features/devices/AccessPointCard.tsx`
- Create: `frontend/src/features/devices/ConfirmAccessDialog.tsx`
- Create: `frontend/src/features/cameras/CamerasPage.tsx`
- Create: `frontend/src/features/cameras/CameraPlayer.tsx`
- Create: `frontend/src/features/cameras/EventTimeline.tsx`
- Create: `frontend/src/features/cameras/ArchiveControls.tsx`
- Modify: `frontend/src/app/App.tsx`
- Test: `frontend/src/features/devices/DevicesPage.test.tsx`
- Test: `frontend/src/features/cameras/CameraPlayer.test.tsx`
- Test: `frontend/src/features/cameras/EventTimeline.test.tsx`

- [ ] **Step 1: Write component tests** for loading/empty/partial-capability/error states, flat switching, selecting/removing device favorites across cameras/intercoms/elevators, favorite persistence isolation per flat, unread dashboard badges and clearing after navigation, call/block settings with stale revisions, keyboard focus, reduced motion, multi-lock confirmation summary, intent expiry, live/archive switching, timeline pagination, confirmed event deletion/unknown outcome, and safe download initiation.
- [ ] **Step 2: Run and observe failure:** `npm --prefix frontend test -- src/features/devices/DevicesPage.test.tsx src/features/cameras/CameraPlayer.test.tsx src/features/cameras/EventTimeline.test.tsx`.
- [ ] **Step 3: Implement the original responsive UI** with semantic controls, visible focus, Russian messages, no upstream URLs in DOM, and mutation confirmation that creates then explicitly executes a one-use intent.
- [ ] **Step 4: Run frontend checks:** `npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build`.
- [ ] **Step 5: Commit:** `git add frontend && git commit -m "feat: add intercom and camera web UI"`.

### Task 10: Mock End-to-End and Read-Only Live Verification

**Files:**
- Create: `tests/mock_provider/devices.rs`
- Create: `frontend/e2e/intercom-media.spec.ts`
- Create: `docs/runbooks/live-device-smoke.md`
- Modify: `README.md`

- [ ] **Step 1: Add a deterministic mock-provider scenario** with two flats, a multi-lock intercom, barrier, elevator, cameras, live HLS, archive, events, partial service outage, and an ambiguous mock mutation response.
- [ ] **Step 2: Write Playwright coverage** for flat selection, dashboard, per-device favorites/main-settings selection, unread badges, device filters, snapshot, live playback request, archive timeline, clip download, confirmation cancellation/expiry/success, keyboard navigation, mobile viewport, and absence of tokens/upstream media URLs in browser-visible state.
- [ ] **Step 3: Run the phase gate:** `cargo test && npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build && npm --prefix frontend exec -- playwright test frontend/e2e/intercom-media.spec.ts`.
- [ ] **Step 4: Perform only the documented read-only live smoke** after MTS ID login: flats, capabilities, devices, snapshots, live metadata, event listing, and archive discovery. Redact request/response metadata and do not commit captured account data. Stop and ask the user before sending a real lock request.
- [ ] **Step 5: Commit:** `git add tests/mock_provider frontend/e2e docs/runbooks/live-device-smoke.md README.md && git commit -m "test: cover intercom and camera flows"`.

## Phase Acceptance Gate

Run from `/Users/alex/dev/domru-mts`:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
npm --prefix frontend run typecheck
npm --prefix frontend test
npm --prefix frontend run build
npm --prefix frontend exec -- playwright test frontend/e2e/intercom-media.spec.ts
git diff --check
```

The phase is complete only when the mock suite passes, the UI exposes every planned device/media state, no browser response leaks credentials or arbitrary proxy capability, and any real lock-control check remains paused until the user gives immediate confirmation.
