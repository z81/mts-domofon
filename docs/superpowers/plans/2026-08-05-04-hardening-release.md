# VDome Core Security, Packaging, and Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden, document, package, and verify the completed core intercom service as a secure single-service Rust/Web release with no dormant excluded-feature surface.

**Architecture:** The core provider/domain/API/UI contracts from phases 1–3 remain unchanged. This phase adds local access protection, CSRF/origin/security headers, redacted health/capability/configuration APIs, connection settings, bounded background supervision, deterministic frontend embedding, a minimal non-root container, repository-wide scope/security tests, CI, and controlled live acceptance.

**Tech Stack:** Phases 1–3; Argon2 0.5.3, IPNet 2.12.1, Rust Embed 8.12.0, Utoipa 5.5.0, Tokio cancellation primitives, Prometheus text metrics, React 19.2.8, TypeScript 6.0.3, Vitest 4.1.10, Playwright 1.62.1, Docker BuildKit, cargo-audit, cargo-deny, and gitleaks.

## Global Constraints

- Finish phases 1–3 and preserve their stable auth, flat/device/media, manual mutation, call, realtime, and error contracts.
- The final product contains only authentication, linked-flat selection, core dashboard/devices, manual opening, cameras/live/archive/read-only events, calls/call settings, Browser Notifications, SSE, webhooks, health, and deployment support.
- Do not scaffold or expose support/FAQ, automatic opening, favorites, documents/PDF, theme/share/About, resident/profile/onboarding, news/issues/chat, billing/meters/preferences, passes, marketplace, or event deletion.
- Bind to `127.0.0.1:3000` by default. Any non-loopback bind requires an Argon2id local password or explicitly trusted reverse-proxy identity plus an exact public origin.
- Preserve one-use intent and immediate-live-confirmation rules for every real door or call-control action; call-setting/blocking writes require CSRF, expected revision, and immediate live confirmation.
- No reusable provider/media/SIP credentials, phone/address/device identifiers, raw SIP/SDP, or upstream URLs may enter browser-visible output, logs, metrics, test artifacts, OpenAPI, or container layers.
- Build/install scripts may access package registries; `build.rs` and runtime code may not install tools or require Node.
- Every task follows TDD, runs focused and full checks, and ends in a small commit without any Codex/Anthropic `Co-Authored-By` trailer.
- When a task creates a Rust module, update its nearest `mod.rs` and include that registry file in the same commit.

---

## Locked File Map

- `src/local_auth.rs`: local password/trusted-proxy session authentication only.
- `src/csrf.rs`: CSRF issuance, binding, validation, and rotation only.
- `src/security_headers.rs`: HTTP security headers and exact Origin/Host policy only.
- `src/openapi.rs`: canonical core OpenAPI assembly only.
- `src/health.rs`: liveness/readiness/capability snapshots only.
- `src/api/config.rs`: redacted local connection configuration HTTP transport only.
- `src/tasks.rs`: task supervision and cancellation only.
- `src/cache.rs`: bounded non-sensitive cache implementation only.
- `src/metrics.rs`: low-cardinality, non-sensitive metrics only.
- `src/web.rs`: embedded SPA asset lookup, cache headers, and fallback only.
- `frontend/src/features/settings/*`: connection, notification, webhook, session, and health settings only.
- `tests/core_scope.rs`: excluded route/module/navigation assertions only.

### Task 1: Local Authentication, CSRF, Origin, and Remote-Bind Safety

**Files:**
- Modify: `Cargo.toml`
- Create: `src/local_auth.rs`
- Create: `src/csrf.rs`
- Create: `src/security_headers.rs`
- Modify: `src/config.rs`
- Modify: `src/storage.rs`
- Modify: `src/app.rs`
- Modify: `src/lib.rs`
- Test: `tests/local_security.rs`

**Configuration contract:**

```text
VDOME_BIND=127.0.0.1:3000
VDOME_LOCAL_PASSWORD_HASH=<argon2id PHC string>
VDOME_TRUST_PROXY_AUTH=false
VDOME_TRUSTED_PROXY_CIDRS=<required when trust proxy auth is true>
VDOME_PUBLIC_ORIGIN=https://vdome.example.test   # required off loopback
```

Add `argon2 = { version = "0.5.3", features = ["std", "zeroize"] }` and `ipnet = "2.12.1"`; reuse phase-3 HMAC/SHA-256 for authenticated local sessions and CSRF binding.

```rust
pub struct LocalPrincipal { pub session_id: LocalSessionId }
pub struct CsrfToken(SecretString);
pub struct OriginPolicy { pub public_origin: Option<Url>, pub trusted_proxy_cidrs: Vec<IpNet> }
pub fn require_mutation(request: &Request<Body>, principal: &LocalPrincipal, policy: &OriginPolicy) -> Result<(), AppError>;
```

- [ ] **Step 1: Write startup-policy tests** rejecting non-loopback binds without password/trusted proxy, plaintext passwords, weak/wrong-algorithm PHC strings, missing/wildcard public origin, trusted proxy without CIDRs, forwarded identity from an untrusted peer, and production TLS-disable flags.
- [ ] **Step 2: Write request-security tests** for Argon2id login/rate limiting, local-session rotation/logout/expiry, Secure/HttpOnly/SameSite cookies, CSRF binding/replay/rotation, exact Origin/Host checking, proxy CIDR enforcement, WebSocket/SSE authentication, body limits, and redacted failures.
- [ ] **Step 3: Run and observe failure:** `cargo test --test local_security -- --nocapture`.
- [ ] **Step 4: Implement fail-closed middleware** while keeping loopback startup usable without a second password. Apply CSP, `nosniff`, frame denial, strict referrer policy, private/no-store API caching, and HSTS only when the configured public origin is HTTPS.
- [ ] **Step 5: Run checks and commit:** `cargo test --test local_security && cargo test && cargo clippy --all-targets --all-features -- -D warnings && git add Cargo.toml Cargo.lock src tests/local_security.rs && git commit -m "feat: harden local and remote access"`.

### Task 2: Core OpenAPI, Health, Capabilities, and Connection Settings

**Files:**
- Create: `src/openapi.rs`
- Create: `src/health.rs`
- Create: `src/api/config.rs`
- Modify: `src/api/health.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/app.rs`
- Create: `frontend/src/features/settings/ConnectionSettings.tsx`
- Create: `frontend/src/features/settings/NotificationSettings.tsx`
- Modify: `frontend/src/app/App.tsx`
- Test: `tests/openapi.rs`
- Test: `tests/api/core_config.rs`
- Test: `frontend/src/features/settings/ConnectionSettings.test.tsx`

**Public contract:**

```text
GET   /health/live
GET   /health/ready
GET   /api/v1/config
PATCH /api/v1/config
GET   /api/v1/capabilities
GET   /api/v1/openapi.json
```

```rust
pub struct PublicConfig {
    pub webhook_enabled: bool,
    pub webhook_destination_label: Option<String>,
    pub browser_notifications_supported: bool,
    pub polling_seconds: u32,
}
pub enum WebhookPatch { Disable, Enable { url: String, secret: SecretString } }
pub struct ConfigPatch { pub expected_revision: String, pub webhook: Option<WebhookPatch>, pub polling_seconds: Option<u32> }
```

- [ ] **Step 1: Write OpenAPI snapshot tests** asserting every core canonical route/method/schema/error/security requirement appears exactly once, operation IDs are unique, compatibility aliases are documented separately, and excluded feature names/routes/provider DTOs/secrets are absent.
- [ ] **Step 2: Write config/health tests** for process/auth/provider/call/media/webhook states, partial per-flat capabilities, provider-bootstrap compatibility, `PATCH` CSRF/revision conflicts, webhook-secret write-only behavior, masked destination labels, polling bounds of 5–3600 seconds, and no PII.
- [ ] **Step 3: Write settings component tests** for Browser Notification feature detection/permission, webhook enabled/disabled/revision/error states, health/capability degradation, session logout, fixed visual styling, mobile/keyboard access, and absence of theme/share/About/document/favorite controls.
- [ ] **Step 4: Run and observe failure:** `cargo test --test openapi --test core_config -- --nocapture && npm --prefix frontend test -- src/features/settings/ConnectionSettings.test.tsx`.
- [ ] **Step 5: Implement schema, redacted diagnostics, revisioned configuration, and settings UI** without returning raw webhook URLs/secrets or Android version/update actions. `ready` fails only for configured core dependencies; a flat lacking an optional camera/archive/call capability remains a valid ready state.
- [ ] **Step 6: Run checks and commit:** `cargo test --test openapi --test core_config && npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build && git add src frontend tests && git commit -m "feat: expose core health and connection settings"`.

### Task 3: Background Supervision, Cache Bounds, Shutdown, and Metrics

**Files:**
- Create: `src/tasks.rs`
- Create: `src/cache.rs`
- Create: `src/metrics.rs`
- Modify: `src/app.rs`
- Modify: `src/main.rs`
- Test: `tests/background_tasks.rs`
- Test: `tests/shutdown.rs`

```rust
pub enum CoreTask { TokenRefresh, CallRegistration, WebhookDelivery, DevicePoll, EventPoll, CacheJanitor }
pub enum TaskState { Starting, Healthy, BackingOff, Paused, Stopped, Failed }
pub struct TaskHealth { pub task: CoreTask, pub state: TaskState, pub last_success_at: Option<DateTime<Utc>>, pub failures: u32 }
```

- [ ] **Step 1: Write supervisor tests** for independent refresh/call/webhook/device/event/cache tasks, auth-logout pause, login wake-up, cancellation propagation, panic isolation/reporting without secret dumps, capped jittered backoff, and deterministic fake-clock behavior.
- [ ] **Step 2: Write resource-bound tests** for maximum cache entries/bytes/TTL, media/request semaphores, webhook/call/event queue bounds, lag reporting, graceful 10-second shutdown, in-flight manual mutation outcome preservation, and low-cardinality metrics labels.
- [ ] **Step 3: Add scope tests** proving no resident/support/marketplace/document/favorite/automatic-open task is registered and no excluded cache namespace exists.
- [ ] **Step 4: Run and observe failure:** `cargo test --test background_tasks --test shutdown -- --nocapture`.
- [ ] **Step 5: Implement supervised tasks and observability** with last-known bounded read-only core state through transient outages. Expose Prometheus text only on loopback or through authenticated local access.
- [ ] **Step 6: Run checks and commit:** `cargo test --test background_tasks --test shutdown && cargo test && cargo clippy --all-targets --all-features -- -D warnings && git add src tests && git commit -m "feat: supervise core background services"`.

### Task 4: Deterministic Frontend Embedding and Production Container

**Files:**
- Modify: `frontend/package-lock.json`
- Create: `build.rs`
- Create: `Dockerfile`
- Create: `docker-compose.yml`
- Create: `.dockerignore`
- Create: `scripts/build-release.sh`
- Create: `scripts/smoke-container.sh`
- Modify: `Cargo.toml`
- Modify: `src/web.rs`
- Test: `tests/embedded_assets.rs`

Add `rust-embed = { version = "8.12.0", features = ["compression", "deterministic-timestamps"] }`. Only the asset source changes from phase-1 `ServeDir` to compile-time embedded Vite output; routing, CSP, cache, and API-404 behavior remain identical.

- [ ] **Step 1: Write asset tests** for hashed JS/CSS, immutable asset caching, `index.html` no-cache, SPA fallback only on non-API GETs, correct MIME/compressed variants, CSP bootstrap behavior, no production source maps, and a precise build error when `frontend/dist` is missing.
- [ ] **Step 2: Implement deterministic release ordering:** `npm --prefix frontend ci`, frontend typecheck/tests/build, then Rust compilation. `build.rs` validates assets and emits exact rerun directives; it performs no network or package installation.
- [ ] **Step 3: Build a non-root multi-stage container** with separate Node/Rust build stages, minimal runtime, owner-writable `/data`, read-only root filesystem compatibility, signal handling, liveness check, and no compiler/package-manager/Node runtime.
- [ ] **Step 4: Run packaging checks:** `npm --prefix frontend ci && npm --prefix frontend run build && cargo test --test embedded_assets && docker build -t domru-mts:test . && ./scripts/smoke-container.sh domru-mts:test`.
- [ ] **Step 5: Commit:** `git add frontend/package-lock.json build.rs Dockerfile docker-compose.yml .dockerignore scripts Cargo.toml Cargo.lock src/web.rs tests/embedded_assets.rs && git commit -m "build: package core UI and container"`.

### Task 5: Repository-Wide Secret, Host, and Core-Scope Enforcement

**Files:**
- Create: `tests/no_secret_leaks.rs`
- Create: `tests/host_policy.rs`
- Create: `tests/core_scope.rs`
- Create: `tests/fixtures/canary_secrets.json`
- Modify: `Cargo.toml`

**Excluded route prefixes asserted as absent:**

```text
/api/v1/support
/api/v1/favorites
/api/v1/documents
/api/v1/legal
/api/v1/profile
/api/v1/residents
/api/v1/news
/api/v1/issues
/api/v1/chat
/api/v1/billing
/api/v1/meters
/api/v1/notifications/preferences
/api/v1/access-requests
/api/v1/marketplace
/api/v1/events/delete-intents
/api/v1/calls/auto-open
```

- [ ] **Step 1: Write canary tests** scanning captured tracing, errors, API/SSE/WebSocket payloads, HTML/bundles/source maps, OpenAPI, panic text, and container inspection output for synthetic access/refresh/WebSSO/device/SIP/media/webhook secrets and synthetic phone/address/device/call identifiers.
- [ ] **Step 2: Write outbound-policy tests** for WebSSO, provider, media, and webhook clients across HTTPS/WSS schemes, exact hosts, userinfo/fragments, manual redirect validation, DNS rebinding, IPv4/IPv6 private/link-local/multicast ranges, and TOCTOU-resistant connection targeting.
- [ ] **Step 3: Write core-scope tests** asserting every excluded route returns JSON `404`, OpenAPI/navigation contain none of them, excluded source module paths do not exist, no provider call is made, and allowed core routes still resolve.
- [ ] **Step 4: Run and observe failure:** `cargo test --test no_secret_leaks --test host_policy --test core_scope -- --nocapture`.
- [ ] **Step 5: Implement any missing centralized redaction/host/scope guards** without adding a generic proxy or stringly typed route dispatcher.
- [ ] **Step 6: Run checks and commit:** `cargo test --test no_secret_leaks --test host_policy --test core_scope && cargo test && git add Cargo.toml Cargo.lock src tests && git commit -m "test: enforce core security boundary"`.

### Task 6: Full Core Browser Regression

**Files:**
- Create: `frontend/e2e/core-product.spec.ts`
- Modify: `tests/mock_provider/mod.rs`
- Modify: `tests/mock_provider/devices.rs`
- Modify: `tests/mock_provider/calls.rs`
- Modify: `frontend/playwright.config.ts`

- [ ] **Step 1: Extend the deterministic mock system** with login challenge variants, two linked flats, partial capabilities, multi-lock intercom, gate/barrier/elevator, snapshots, HLS/live/archive/events, call settings/blocking, audio/video calls, all manual actions, reconnect, rate limits, schema additions, partial outages, session expiry, and ambiguous mutations.
- [ ] **Step 2: Write Playwright journeys** for authentication/restart/logout, flat switching, dashboard indicators, device filtering/manual opening, snapshots/live/archive/events/clip download, incoming audio/video calls, accept/reject/ignore/hangup/manual-open, local media controls, Browser Notifications, webhooks, reconnect, and health/settings.
- [ ] **Step 3: Add accessibility and scope assertions** for mobile/tablet/desktop, keyboard/focus/reduced motion, Russian validation, no upstream URLs/secrets, and absence of every excluded navigation label/route.
- [ ] **Step 4: Run the full UI gate:** `npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build && npm --prefix frontend exec -- playwright test`.
- [ ] **Step 5: Commit:** `git add tests/mock_provider frontend/e2e frontend/playwright.config.ts && git commit -m "test: cover complete core intercom UI"`.

### Task 7: CI and Dependency Policy

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `deny.toml`
- Create: `.gitleaks.toml`
- Modify: `Cargo.toml`
- Modify: `frontend/package.json`

- [ ] **Step 1: Add CI jobs** for `cargo fmt --check`, clippy with warnings denied, all Rust tests, frontend typecheck/unit/build, full Playwright mock E2E, OpenAPI/scope snapshots, container build/smoke, and artifact secret scans.
- [ ] **Step 2: Add dependency/security jobs** for `cargo audit`, `cargo deny check`, npm audit at the documented severity, gitleaks, locked Cargo/npm graphs, and checksum-pinned RSIP 0.4.0 MIT license-file clarification in `deny.toml`.
- [ ] **Step 3: Configure artifacts** to retain only JUnit/coverage/Playwright diagnostics proven by canary tests to contain no bodies, screenshots with PII, tokens, SIP/SDP, or upstream URLs.
- [ ] **Step 4: Run the exact local CI gate:** `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build && npm --prefix frontend exec -- playwright test && cargo deny check && git diff --check`.
- [ ] **Step 5: Commit:** `git add .github deny.toml .gitleaks.toml Cargo.toml Cargo.lock frontend/package.json frontend/package-lock.json && git commit -m "ci: enforce core product quality gates"`.

### Task 8: Core Documentation and Controlled Live Acceptance

**Files:**
- Modify: `README.md`
- Create: `LICENSE`
- Create: `SECURITY.md`
- Create: `docs/configuration.md`
- Create: `docs/api.md`
- Create: `docs/deployment.md`
- Create: `docs/security.md`
- Create: `docs/troubleshooting.md`
- Create: `docs/live-acceptance.md`
- Create: `CHANGELOG.md`

- [ ] **Step 1: Document setup in Russian** with loopback and Docker examples, WebSSO/SMS expectations, secret-key backup/rotation, remote-bind protection, camera/call browser requirements, webhook signatures, capability behavior, and recovery from expired/unknown manual mutations.
- [ ] **Step 2: Document the exact core API and legacy compatibility aliases**, OpenAPI location, current APK/version/hash used for interoperability research, unofficial/non-affiliated notice, data retention, and the complete excluded-feature list. Clarify that repository documentation remains while APK end-user document/PDF functionality is excluded.
- [ ] **Step 3: Write the controlled live checklist** in this order: login/SMS; all read-only flat/device/camera/archive/event/call-setting checks; passive self-ring observation; then one separately confirmed action at a time for manual door, accept, reject, ignore, hangup, open-during-call, call block, and call-setting update. Include redaction and cleanup after every action.
- [ ] **Step 4: Run the final gate and container smoke**, inspect the production bundle/OpenAPI/container filesystem for secrets, excluded routes/assets/modules, build tools, and decompilation artifacts, then record only pass/fail plus redacted version/request IDs.
- [ ] **Step 5: Commit:** `git add README.md LICENSE SECURITY.md docs CHANGELOG.md && git commit -m "docs: publish core deployment and acceptance guide"`.

## Final Release Gate

Run from the repository root on a clean checkout:

```bash
npm --prefix frontend ci
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
npm --prefix frontend run typecheck
npm --prefix frontend test
npm --prefix frontend run build
npm --prefix frontend exec -- playwright test
cargo deny check
docker build -t domru-mts:acceptance .
./scripts/smoke-container.sh domru-mts:acceptance
git diff --check
git status --short
```

The release is complete only when all four phase gates pass, every included core feature has API/UI/mock coverage, every excluded route/module/navigation assertion passes, the service remains registered for calls without an open browser, the container contains no build tools or secrets, read-only live checks pass, and every real door/call mutation remains paused for immediate user confirmation.
