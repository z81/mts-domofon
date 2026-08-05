# VDome Access, Marketplace, Settings, Security, and Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete individual/vehicle access requests, marketplace, product settings/about/legal surfaces, and provider compatibility diagnostics, then harden, document, package, and verify the entire VDome Rust service and Web UI as a releasable single-service application.

**Architecture:** Access requests and marketplace use their own typed providers and stable domain APIs. The basket is local and revisioned; checkout revalidates provider products before producing a one-use order intent. Final hardening adds loopback-safe defaults, password or trusted-proxy protection for remote binding, CSRF/origin controls, a generated OpenAPI contract, structured health/capability diagnostics, reproducible frontend embedding, container packaging, and full mock E2E/security gates.

**Tech Stack:** Phases 1–4; Argon2, HMAC/SHA-256, Utoipa 5.5.0, Tower HTTP 0.7.0, Rust Embed 8.12.0, Cargo/Docker BuildKit; React 19.2.8, TypeScript 6.0.3, Vite 8.2.0, Vitest 4.1.10, Playwright 1.62.1, npm 11.16.0.

## Global Constraints

- Finish phases 1–4 and preserve all stable API/domain/error contracts unless a failing cross-phase test proves a correction is required.
- Access-request types are exactly `PASS` (individual) and `DRIVE` (vehicle) for `POST /passage-service/api/v1/events?companyId={companyId}`.
- Marketplace calls use `/marketplace-service/api/v1/resident/*`; the observed catalog path is intentionally spelled `katalog` and must not be corrected in upstream requests.
- Guest phones, license plates, addresses, comments, basket contents, orders, and provider price data are personal data; redact logs and retain only bounded local data needed for the user-visible feature.
- Basket totals shown by the browser are estimates. Immediately before issuing an order intent, refetch every product and require the user to confirm the provider-current items, quantities, total, delivery address, and comment.
- Access requests and order creation consume one-use intents before dispatch, have no blind retry, and return an explicit unknown outcome for ambiguous failures.
- Ask the user immediately before every real access-request or order mutation. No payment/card processing is added to marketplace checkout unless VDome itself returns an official safe redirect covered by phase 4.
- Default binding remains `127.0.0.1:3000`. Any non-loopback bind must fail startup unless password authentication or explicit trusted reverse-proxy authentication is configured.
- Production builds must contain no APK/decompilation artifacts, real-account fixtures, source maps with secrets, test-only unsafe flags, or network access to hosts outside explicit provider/media/webhook policies.
- Do not include `Co-Authored-By` trailers or any Codex/Anthropic co-author line in commits.
- When a task creates a Rust module, update its nearest existing `mod.rs` (and `src/lib.rs` only for a new top-level module) in the same task and include that registry file in the task commit.

---

## Locked File Map

- `src/domain/access.rs`: individual/vehicle request types and status only.
- `src/domain/marketplace.rs`: catalog/product/basket/order types only.
- `src/provider/mts/access.rs`: access-request upstream mapping only.
- `src/provider/mts/marketplace.rs`: catalog/product/order upstream calls only.
- `src/service/access.rs`: request validation/history/policy only.
- `src/service/marketplace.rs`: catalog composition, basket, price revalidation, order intents only.
- `src/api/access.rs`, `marketplace.rs`: corresponding HTTP transport only.
- `src/local_auth.rs`: local password/trusted-proxy session authentication only.
- `src/csrf.rs`: CSRF token issuance/validation only.
- `src/security_headers.rs`: CSP and response security headers only.
- `src/openapi.rs`: API schema assembly only.
- `src/health.rs`: liveness/readiness/capability snapshots only.
- `frontend/src/features/access/*`, `marketplace/*`: corresponding UI only.
- `.github/workflows/ci.yml`, `Dockerfile`, `docker-compose.yml`: build/release automation only.

### Task 1: Individual and Vehicle Access Requests

**Files:**
- Create: `src/domain/access.rs`
- Create: `src/provider/mts/access.rs`
- Create: `src/provider/mts/dto/access.rs`
- Create: `src/service/access.rs`
- Create: `tests/fixtures/access_request_profile.json`
- Create: `tests/contract/access_requests.rs`

**Interfaces and observed body mapping:**

```rust
pub enum AccessRequestKind { Individual { guest_phone: PhoneNumber }, Vehicle { license_plate: LicensePlate } }
pub struct AccessRequestDraft { pub flat_id: FlatId, pub company_id: String, pub active_from: DateTime<Utc>, pub active_to: DateTime<Utc>, pub comment: Option<String>, pub kind: AccessRequestKind }

// Provider body fields: type, activeFrom, activeTo, userPhoneNumber, fias,
// guestPhoneNumber, comment, source, authorId, companyId, accessLevel,
// createdBy, licenseNumber, apartment.
```

- [ ] **Step 1: Write contracts** for `POST /passage-service/api/v1/events`, exact `PASS`/`DRIVE` mapping, profile/flat-derived author/address fields, company query/body consistency, Moscow-local input converted to UTC, maximum duration/provider bounds, normalized Russian phone/plate, required per-kind fields, flat/company ownership, intent expiry/one-use semantics, and ambiguous outcomes.
- [ ] **Step 2: Run and observe failure:** `cargo test --test access_requests -- --nocapture`.
- [ ] **Step 3: Implement provider/service** with a bounded encrypted local ledger of confirmed/unknown submissions for UI feedback only; it must never claim remote revocation/history endpoints that VDome does not expose.
- [ ] **Step 4: Run checks:** `cargo test --test access_requests && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit:** `git add src/domain/access.rs src/provider/mts/access.rs src/provider/mts/dto/access.rs src/service/access.rs tests && git commit -m "feat: add guest and vehicle access requests"`.

### Task 2: Marketplace Catalog, Search, and Product Details

**Files:**
- Create: `src/domain/marketplace.rs`
- Create: `src/provider/mts/marketplace.rs`
- Create: `src/provider/mts/dto/marketplace.rs`
- Create: `src/service/marketplace.rs`
- Create: `tests/fixtures/marketplace_catalog.json`
- Create: `tests/fixtures/marketplace_product.json`
- Create: `tests/contract/marketplace_catalog.rs`

**Observed upstream routes:**

```text
GET /marketplace-service/api/v1/resident/katalog/{companyId}?parent={parentId}&page={page}&size={pageSize}
GET /marketplace-service/api/v1/resident/katalog/{companyId}?name={query}&page={page}&size={pageSize}
GET /marketplace-service/api/v1/resident/product/{id}?companyId={companyId}
```

- [ ] **Step 1: Write catalog/search/product contracts** for categories/children/items, pagination, Unicode search, empty query, company scope, integer/decimal price normalization, availability, options, images through the phase-2 safe media proxy, sanitized descriptions, unknown fields/statuses, missing marketplace capability, and cross-company product IDs.
- [ ] **Step 2: Run and observe failure:** `cargo test --test marketplace_catalog -- --nocapture`.
- [ ] **Step 3: Implement the provider/service** with a short bounded per-company read cache, exact upstream `katalog` spelling, immediate DTO conversion, safe image leases, and cache invalidation on session/flat/company change.
- [ ] **Step 4: Run checks:** `cargo test --test marketplace_catalog && cargo test`.
- [ ] **Step 5: Commit:** `git add src/domain/marketplace.rs src/provider/mts/marketplace.rs src/provider/mts/dto/marketplace.rs src/service/marketplace.rs tests && git commit -m "feat: add marketplace catalog and search"`.

### Task 3: Revisioned Local Basket and Provider Price Revalidation

**Files:**
- Create: `src/marketplace/mod.rs`
- Create: `src/marketplace/basket.rs`
- Modify: `src/service/marketplace.rs`
- Modify: `src/storage.rs`
- Create: `tests/marketplace_basket.rs`

**Interfaces:**

```rust
pub struct BasketRevision(pub u64);
pub struct Basket { pub company_id: String, pub revision: BasketRevision, pub items: Vec<BasketLine>, pub estimated_total: Money }
impl MarketplaceService {
    pub async fn update_basket(&self, session: &LocalSessionId, expected: BasketRevision, command: BasketCommand) -> Result<Basket, AppError>;
    pub async fn checkout_summary(&self, session: &LocalSessionId, expected: BasketRevision, input: CheckoutInput) -> Result<MutationIntent, AppError>;
}
```

- [ ] **Step 1: Write basket tests** for add/remove/set quantity, maximum quantity/line count, one company per basket, stale revisions, concurrent updates, unavailable/deleted products, money/currency, persistence encryption/atomicity, logout cleanup, and no trust in browser price/name/total.
- [ ] **Step 2: Write checkout-summary tests** that refetch every product, reject changed availability, surface changed prices as a new basket revision, calculate only from provider-current values, validate delivery address/comment, and bind the complete summary hash to a one-use order intent.
- [ ] **Step 3: Run and observe failure:** `cargo test --test marketplace_basket -- --nocapture`.
- [ ] **Step 4: Implement the revisioned basket** with per-local-session locking, encrypted atomic persistence, provider revalidation, and no background order dispatch.
- [ ] **Step 5: Run checks and commit:** `cargo test --test marketplace_basket && cargo clippy --all-targets --all-features -- -D warnings && git add src/marketplace src/service/marketplace.rs src/storage.rs tests/marketplace_basket.rs && git commit -m "feat: add revisioned marketplace basket"`.

### Task 4: Order Creation and Order History

**Files:**
- Modify: `src/provider/mts/marketplace.rs`
- Modify: `src/provider/mts/dto/marketplace.rs`
- Modify: `src/service/marketplace.rs`
- Create: `tests/fixtures/marketplace_orders.json`
- Create: `tests/contract/marketplace_orders.rs`

**Observed upstream routes/body:**

```text
POST /marketplace-service/api/v1/resident/order
     {companyId,address,comment,items:[{itemId,quantity}]}
GET  /marketplace-service/api/v1/resident/order?page={page}&size={pageSize}
GET  /marketplace-service/api/v1/resident/allitem/{id}
```

- [ ] **Step 1: Write contracts** for order creation/body, consumed checkout intent, provider-current summary hash, duplicate execute, ambiguous timeout, confirmed basket clearing only, retained basket on failure/unknown, history pagination, `/allitem/{id}` order details, order number/status/created time/items/address/total mapping, unknown statuses, and order/company ownership.
- [ ] **Step 2: Run and observe failure:** `cargo test --test marketplace_orders -- --nocapture`.
- [ ] **Step 3: Implement order dispatch/history**, publishing a redacted `order_updated` event, keeping an encrypted local unknown-outcome marker for reconciliation, and never synthesizing success from a subsequent history mismatch.
- [ ] **Step 4: Run checks:** `cargo test --test marketplace_orders && cargo test && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit:** `git add src/provider/mts/marketplace.rs src/provider/mts/dto/marketplace.rs src/service/marketplace.rs tests && git commit -m "feat: add marketplace checkout and orders"`.

### Task 5: Access and Marketplace Local APIs

**Files:**
- Create: `src/api/access.rs`
- Create: `src/api/marketplace.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/app.rs`
- Create: `tests/api/access_marketplace.rs`

**Routes:**

```text
GET    /api/v1/access-requests
POST   /api/v1/access-requests/intents
POST   /api/v1/access-requests/intents/:id/execute
GET    /api/v1/marketplace/catalog
GET    /api/v1/marketplace/search
GET    /api/v1/marketplace/products/:id
GET    /api/v1/marketplace/basket
PATCH  /api/v1/marketplace/basket
POST   /api/v1/marketplace/checkout/intents
POST   /api/v1/marketplace/checkout/intents/:id/execute
GET    /api/v1/marketplace/orders
GET    /api/v1/marketplace/orders/:id
```

- [ ] **Step 1: Write Axum tests** for every route, flat/company scoping, paging/search limits, decimal/money JSON, basket revisions/409 conflicts, mutation/CSRF/session checks, safe product images, intent summaries/expiry, ambiguous results, stable errors, and secret/PII minimization.
- [ ] **Step 2: Run and observe failure:** `cargo test --test access_marketplace -- --nocapture`.
- [ ] **Step 3: Implement thin handlers** and OpenAPI schemas without exposing provider DTOs or a generic order/access proxy.
- [ ] **Step 4: Run API/full checks:** `cargo test --test access_marketplace && cargo test`.
- [ ] **Step 5: Commit:** `git add src/api src/app.rs tests/api/access_marketplace.rs && git commit -m "feat: expose access and marketplace API"`.

### Task 6: Access, Marketplace, Settings, About, and Legal Web UI

**Files:**
- Create: `frontend/src/features/access/AccessRequestsPage.tsx`
- Create: `frontend/src/features/access/AccessRequestForm.tsx`
- Create: `frontend/src/features/marketplace/MarketplacePage.tsx`
- Create: `frontend/src/features/marketplace/CatalogTree.tsx`
- Create: `frontend/src/features/marketplace/ProductCard.tsx`
- Create: `frontend/src/features/marketplace/ProductPage.tsx`
- Create: `frontend/src/features/marketplace/BasketDrawer.tsx`
- Create: `frontend/src/features/marketplace/CheckoutPage.tsx`
- Create: `frontend/src/features/marketplace/OrdersPage.tsx`
- Create: `frontend/src/features/settings/SettingsPage.tsx`
- Create: `frontend/src/features/settings/AppearanceSettings.tsx`
- Create: `frontend/src/features/settings/AboutPage.tsx`
- Create: `frontend/src/features/settings/ProviderDiagnostics.tsx`
- Modify: `frontend/src/app/App.tsx`
- Test: `frontend/src/features/access/AccessRequestForm.test.tsx`
- Test: `frontend/src/features/marketplace/CheckoutPage.test.tsx`
- Test: `frontend/src/features/settings/AboutPage.test.tsx`

- [ ] **Step 1: Write UI tests** for PASS/DRIVE switching, date/time/phone/plate validation, confirmation/unknown outcomes/local ledger, unsupported capability, category navigation/search/product details, safe images, revision conflicts, basket updates, live price changes, checkout summary, order outcomes/history, light/dark/system theme persistence, app/provider versions, compatibility/capability states, legal PDF/privacy links, current Play Store share/copy action, official MTS ID links, Russian loading/empty/error states, mobile/keyboard/focus/reduced-motion behavior.
- [ ] **Step 2: Run and observe failure:** `npm --prefix frontend test -- src/features/access/AccessRequestForm.test.tsx src/features/marketplace/CheckoutPage.test.tsx src/features/settings/AboutPage.test.tsx`.
- [ ] **Step 3: Implement responsive pages** that never submit directly from the edit form, always display the server-generated final summary, handle stale basket revisions without losing user input, do not imitate proprietary MTS marketplace visuals, use browser-native PDF viewing/download, and mark every legal/Play/MTS ID transition as an official external destination with `noopener,noreferrer`. Replace the hidden APK developer menu with local redacted health/compatibility diagnostics; do not reproduce ESMART internals.
- [ ] **Step 4: Run frontend checks:** `npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build`.
- [ ] **Step 5: Commit:** `git add frontend && git commit -m "feat: add access and marketplace web UI"`.

### Task 7: Local Authentication, CSRF, Origin, and Remote-Bind Safety

**Files:**
- Modify: `Cargo.toml`
- Create: `src/local_auth.rs`
- Create: `src/csrf.rs`
- Create: `src/security_headers.rs`
- Modify: `src/config.rs`
- Modify: `src/storage.rs`
- Modify: `src/app.rs`
- Create: `tests/local_security.rs`

**Configuration contract:**

```text
VDOME_BIND=127.0.0.1:3000
VDOME_LOCAL_PASSWORD_HASH=<argon2id PHC string>
VDOME_TRUST_PROXY_AUTH=false
VDOME_TRUSTED_PROXY_CIDRS=<required when trust proxy auth is true>
VDOME_PUBLIC_ORIGIN=https://vdome.example.test   # required off loopback
```

Add `argon2 = { version = "0.5.3", features = ["std", "zeroize"] }` and `ipnet = "2.12.1"`; reuse the phase-3 HMAC/SHA-256 dependencies for CSRF/session token authentication.

- [ ] **Step 1: Write startup-policy tests** rejecting non-loopback without password/trusted proxy, plaintext passwords, missing public origin, wildcard origin, trusted-proxy mode without CIDRs, forwarded identity from untrusted peers, weak session key, and production TLS-disable flags.
- [ ] **Step 2: Write request-security tests** for Argon2id login/rate limit, session rotation/logout/expiry, Secure/HttpOnly/SameSite cookies, CSRF token binding and replay, Origin/Host validation, WebSocket/SSE auth, body limits, CSP/nonces, HSTS only behind HTTPS, `nosniff`, frame denial, referrer policy, cache control, and redacted auth failures.
- [ ] **Step 3: Run and observe failure:** `cargo test --test local_security -- --nocapture`.
- [ ] **Step 4: Implement fail-closed security middleware**, keeping loopback single-user startup friction low while making remote exposure impossible without explicit protection.
- [ ] **Step 5: Run checks and commit:** `cargo test --test local_security && cargo clippy --all-targets --all-features -- -D warnings && git add Cargo.toml Cargo.lock src/local_auth.rs src/csrf.rs src/security_headers.rs src/config.rs src/storage.rs src/app.rs tests/local_security.rs && git commit -m "feat: harden local and remote access"`.

### Task 8: OpenAPI, Capabilities, Configuration, and Compatibility Contract

**Files:**
- Create: `src/openapi.rs`
- Create: `src/api/config.rs`
- Create: `src/api/documents.rs`
- Create: `src/documents.rs`
- Create: `src/health.rs`
- Modify: `src/api/health.rs`
- Modify: `src/app.rs`
- Create: `tests/openapi.rs`
- Create: `tests/api/compatibility.rs`
- Create: `tests/document_policy.rs`
- Create: `docs/api-compatibility.md`

**Public diagnostics:**

```text
GET /health/live
GET /health/ready
GET /api/v1/config
GET /api/v1/capabilities
GET /api/v1/about
GET /api/v1/legal/terms
GET /api/v1/legal/privacy
GET /api/v1/documents/:lease
GET /api/v1/openapi.json
```

`/api/v1/about` returns this application's version plus normalized provider bootstrap compatibility, the current APK interoperability reference, and fixed official links: terms `https://resident-rest.vdome.mts.ru/docs/terms-of-use-resident.pdf`, privacy `https://domofon.mts.ru/privacy-policy.html`, and Play Store `https://play.google.com/store/apps/details?id=ru.mts.vdome.resident`. Android `/config` minimum/latest-version flags are diagnostics, never a false forced-update gate for this Rust/Web client.

- [ ] **Step 1: Write OpenAPI tests** asserting every canonical route/method/schema/security/error response is present, all operation IDs are unique, provider DTOs/secrets are absent, and generated JSON is stable under a checked snapshot.
- [ ] **Step 2: Write health/capability/compatibility tests** for process/auth/provider bootstrap config/feature flags, call/media/support states, optional-service availability, unknown flags, Rust-vs-Android version interpretation, no PII, exact official about/legal/Play links, and the promised aliases `/api/login`, `/api/confirm`, `/api/session`, `/api/logout`, `/api/places`, `/api/snapshot/:id`, `/api/stream/:id`, `/api/archive/:id`, `/api/open-door` using canonical safety rules.
- [ ] **Step 3: Write document-policy tests** for the fixed terms PDF and opaque document leases created only from authenticated invoice/support/issue metadata: HTTPS/host/DNS/redirect allowlists, PDF/image signature and MIME checks, byte/time bounds, content-disposition filename sanitization, range handling, expiry/ownership, no HTML execution, and no arbitrary caller-supplied URL.
- [ ] **Step 4: Run and observe failure:** `cargo test --test openapi --test compatibility --test document_policy -- --nocapture`.
- [ ] **Step 5: Implement schema assembly, redacted diagnostics, fixed legal metadata, and the bounded document service**, document exact alias differences from `domru-z81`, and return readiness failure only for conditions required by configured features rather than absent optional company modules.
- [ ] **Step 6: Run checks and commit:** `cargo test --test openapi --test compatibility --test document_policy && cargo test && git add src/openapi.rs src/api src/documents.rs src/health.rs src/app.rs tests docs/api-compatibility.md && git commit -m "feat: publish compatibility and legal contract"`.

### Task 9: Background Polling, Cache Bounds, Shutdown, and Observability

**Files:**
- Create: `src/tasks.rs`
- Create: `src/cache.rs`
- Create: `src/metrics.rs`
- Modify: `src/app.rs`
- Modify: `src/main.rs`
- Create: `tests/background_tasks.rs`
- Create: `tests/shutdown.rs`

- [ ] **Step 1: Write supervisor tests** for independent call/support-websocket/webhook/poll/cache tasks, cancellation propagation, panic reporting without process-wide secret dump, auth logout pause, refresh wake-up, capped exponential backoff/jitter, no polling of unavailable capabilities, provider bootstrap refresh, and deterministic clock control.
- [ ] **Step 2: Write resource-bound tests** for maximum cache entries/bytes/TTL, media/concurrent-request semaphores, webhook/call/event queues, upload cleanup, graceful 10-second shutdown, in-flight mutation outcome handling, and non-sensitive metrics labels.
- [ ] **Step 3: Run and observe failure:** `cargo test --test background_tasks --test shutdown -- --nocapture`.
- [ ] **Step 4: Implement supervised tasks and observability** with tracing fields limited to stable codes/timings/counts, Prometheus text available only on loopback/authenticated routes, and last-known read-only data retained through transient outages.
- [ ] **Step 5: Run checks and commit:** `cargo test --test background_tasks --test shutdown && cargo clippy --all-targets --all-features -- -D warnings && git add src tests && git commit -m "feat: supervise background services"`.

### Task 10: Reproducible Frontend Embedding and Production Container

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

- Add `rust-embed = { version = "8.12.0", features = ["compression", "deterministic-timestamps"] }` and switch only `src/web.rs`'s asset source from phase-1 `ServeDir` to compile-time embedded Vite output; routing/cache/CSP behavior must remain unchanged.

- [ ] **Step 1: Write embedded-asset tests** for hashed JS/CSS, SPA fallback only on non-API GETs, correct MIME/cache headers, CSP nonce/bootstrap behavior, no source maps in release, compressed variants, and a helpful compile error when `frontend/dist` is absent outside the documented build pipeline.
- [ ] **Step 2: Implement deterministic builds** so the local release script and Docker Node stage run `npm ci` and Vite before Rust compilation; `build.rs` validates `frontend/dist`, emits precise rerun directives, and never performs network installation itself. Pin the Rust toolchain and lock both Cargo/npm dependency graphs.
- [ ] **Step 3: Build a multi-stage non-root container** with Node frontend stage, Rust build stage, minimal runtime, writable `/data` owned by the service UID, read-only root filesystem compatibility, health check, signal handling, and no compiler/package-manager/runtime Node.
- [ ] **Step 4: Run packaging checks:** `npm --prefix frontend ci && npm --prefix frontend run build && cargo test --test embedded_assets && docker build -t domru-mts:test . && ./scripts/smoke-container.sh domru-mts:test`.
- [ ] **Step 5: Commit:** `git add frontend/package-lock.json build.rs Dockerfile docker-compose.yml .dockerignore scripts/build-release.sh scripts/smoke-container.sh Cargo.toml Cargo.lock src/web.rs tests/embedded_assets.rs && git commit -m "build: package embedded UI and container"`.

### Task 11: CI, Dependency Policy, and Full Security/E2E Regression

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `deny.toml`
- Create: `.gitleaks.toml`
- Create: `frontend/e2e/full-product.spec.ts`
- Create: `tests/no_secret_leaks.rs`
- Create: `tests/host_policy.rs`
- Modify: `Cargo.toml`
- Modify: `frontend/package.json`

- [ ] **Step 1: Add CI jobs** for `cargo fmt --check`, clippy with warnings denied, all Rust tests, frontend typecheck/unit/build, Playwright mock E2E, `cargo audit`, `cargo deny check`, npm audit at configured severity, secret scan, OpenAPI snapshot, container build/smoke, and uploaded test artifacts that contain no response bodies/secrets. In `deny.toml`, add a checksum-pinned license clarification for RSIP 0.4.0's bundled MIT `LICENSE` file because that crate uses `license-file` rather than an SPDX expression.
- [ ] **Step 2: Add repository-wide security tests** scanning logs/API/SSE/WebSocket/HTML/bundles/source maps/crash text for synthetic canary secrets, and host-policy tests covering DNS rebinding/redirect chains/IPv4/IPv6/private ranges for media/documents, payment, WebSSO/MTS ID links, MTS support, webhooks, and marketplace images.
- [ ] **Step 3: Write full Playwright regression** covering login challenge variants, all navigation/screens, flat switching, device favorites/unread badges, devices/cameras/archive/events, calls/auto-open settings, access requests, resident/support services, marketplace, settings/about/theme/legal/share, local auth, mobile/tablet/desktop, keyboard focus, reduced motion, partial outages, session expiry, reconnect, and every confirmation summary.
- [ ] **Step 4: Run the exact local CI gate:** `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build && npm --prefix frontend exec -- playwright test && cargo deny check && git diff --check`.
- [ ] **Step 5: Commit:** `git add .github deny.toml .gitleaks.toml frontend/e2e tests Cargo.toml frontend/package.json && git commit -m "ci: enforce full product quality gates"`.

### Task 12: Documentation, Migration Guide, and Controlled Live Acceptance

**Files:**
- Modify: `README.md`
- Create: `LICENSE`
- Create: `SECURITY.md`
- Create: `docs/configuration.md`
- Create: `docs/deployment.md`
- Create: `docs/api.md`
- Create: `docs/live-acceptance.md`
- Create: `docs/privacy.md`
- Create: `docs/troubleshooting.md`
- Create: `CHANGELOG.md`

- [ ] **Step 1: Document installation/login/configuration** in Russian with loopback and Docker examples, WebSSO/SMS expectations, secret-key backup/rotation, remote-bind protection, webhook signatures, browser permissions, auto-open safety flag/disarm behavior, theme/about/legal/official external-link boundaries, feature/capability behavior, and recovery from expired/unknown mutation states.
- [ ] **Step 2: Document every local API group and `domru-z81` alias**, OpenAPI location, complete included/excluded/equivalent feature list (including TNPS, ESMART, Wear/NFC/BLE, update/rate/share, MTS Profile SDK, and deep-link/push replacements), current APK/version/hash used for interoperability research, provider trademark/non-affiliation notice, data retention, and the prohibition on card-data handling.
- [ ] **Step 3: Write the controlled live acceptance checklist** in this order: login/SMS; all read-only modules including support history/appeals and legal/provider diagnostics; passive call observation; then one separately confirmed action at a time for door, auto-open arming/test, call control, access request, issue, management/support chat, support upload/reaction/appeal close, feedback, resident/profile, reading, notification setting, payment redirect/offline status, and order. Include redaction, disarming, and cleanup after each check.
- [ ] **Step 4: Run all gates from task 11 plus container smoke**, manually inspect the production bundle/OpenAPI/container filesystem for secrets/decompilation artifacts, and record only pass/fail plus redacted version/request IDs in `docs/live-acceptance.md`.
- [ ] **Step 5: Commit:** `git add README.md LICENSE SECURITY.md docs CHANGELOG.md && git commit -m "docs: add deployment and acceptance guide"`.

## Final Release Gate

Run from `/Users/alex/dev/domru-mts` on a clean checkout:

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

The product is complete only when all five phase gates pass, the complete feature matrix is represented in API/UI/E2E coverage, the container starts with persistent encrypted state and no build tools, live read-only checks pass, and every real mutation is performed only after immediate user confirmation with its result reported explicitly.
