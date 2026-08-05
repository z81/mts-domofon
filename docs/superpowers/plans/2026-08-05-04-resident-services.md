# VDome Resident, Support, Issues, Billing, Meters, and Home Services Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement every browser-transferable resident-service feature: profile/flats/onboarding, additional residents, news/companies/contacts/feedback, MTS support FAQ/chat/appeals, issues with messages/files, management chat, invoices/payment redirects, meters/readings, and notification preferences.

**Architecture:** A resident provider is split by upstream resource rather than one large REST client. The separate MTS Support HTTP/WebSocket client authenticates with the server-held ID token and maps only safe message primitives. Stable domain services enforce flat ownership, pagination, upload/payment/document policies, and one-use mutation intents. Optional company-specific modules fail independently and are reflected as capabilities. The React home-services area shares a flat selector but uses separate feature routes, query state, mutation summaries, and accessible forms.

**Tech Stack:** Phases 1–3; Axum multipart/body limits, Reqwest streaming multipart, Serde, Chrono, Rust Decimal for money/readings, Infer/MIME validation, Utoipa; React 19.2.8, TypeScript 6.0.3, Vitest 4.1.10, and Playwright 1.62.1.

## Global Constraints

- Finish phases 1–3 first; reuse their selected-flat validation, capability set, one-use intents, encrypted state, redaction, SSE bus, and stable error envelope.
- Use `resident-rest.vdome.mts.ru` for `/api/new/*`, `/api/v1/*`, and `/api/registration/*` routes; use the gateway for `/vdome-smart-facade/*`, `/domofon/*`, `/gw-intercom/*`, and payment-service routes.
- Do not infer authorization from an object ID. Resolve the active session's flats first and validate every issue, invoice, meter, resident, company, and chat scope.
- All monetary values and meter readings use decimal/string representations; never use binary floating point for validation or JSON output.
- Uploaded issue images/files are limited by count, per-file size, aggregate size, MIME signature, safe filename, and timeout. They are streamed and never persisted unless explicitly staged in an owner-only temporary file that is deleted after dispatch.
- Payment handling may only return/open an HTTPS URL from an allowlist derived from the official VDome response. Never proxy a card form, accept card data, or mark payment complete locally.
- Support HTTP is restricted to `online-chat.mts.ru` and `support.mts.ru`, and its WebSocket to `wss://online-chat.mts.ru/ws/chat`. The MTS ID token stays server-side; never inject it into browser links, HTML, or a generic proxy.
- Dynamic support micro-apps, votes, polls, and surveys are not rendered as arbitrary provider HTML. Only allowlisted official HTTPS links may be handed to a separate `noopener,noreferrer` browser context.
- Creating/editing/cancelling issues, sending feedback/chat, changing profile/residents/preferences, submitting readings, offline-payment marking, and requesting a payment session are mutations: require one-use intents and never blindly retry ambiguous outcomes.
- Sending/uploading/reacting in MTS support and closing an appeal are also mutations requiring one-use intents and no ambiguous retry.
- Ask the user immediately before each real issue, resident, management/support chat, support upload/reaction/appeal-close, feedback, meter, notification, offline-payment, or payment-related mutation during live verification.
- Redacted synthetic fixtures must not include real names, phones, addresses, account numbers, invoice IDs, meter serials, messages, files, or payment URLs.
- Do not include `Co-Authored-By` trailers or any Codex/Anthropic co-author line in commits.
- When a task creates a Rust module, update its nearest existing `mod.rs` (and `src/lib.rs` only for a new top-level module) in the same task and include that registry file in the task commit.

---

## Locked File Map

- `src/domain/resident.rs`: profile, flat onboarding, resident, company, contact, news types.
- `src/domain/issue.rs`: issue/category/message/attachment types.
- `src/domain/chat.rs`: management chat types.
- `src/domain/billing.rs`: service/invoice/money/payment-redirect types.
- `src/domain/meter.rs`: meter/reading/statistics/submission types.
- `src/domain/notification.rs`: provider notification preferences only.
- `src/domain/support.rs`: support conversation/message/attachment/operator/appeal and safe-action types only.
- `src/provider/mts/resident.rs`: profile/flats/residents/address/company/feedback calls only.
- `src/provider/mts/news.rs`, `issues.rs`, `chat.rs`, `billing.rs`, `meters.rs`, `notifications.rs`: corresponding provider calls only.
- `src/provider/mts/support.rs`: MTS Support config/history/files/appeals HTTP and chat WebSocket protocol only.
- `src/service/resident.rs`, `issues.rs`, `chat.rs`, `billing.rs`, `meters.rs`, `notifications.rs`: policy/composition only.
- `src/service/support.rs`: support session, safe-content/action, attachment, and mutation policy only.
- `src/api/residents.rs`, `news.rs`, `companies.rs`, `issues.rs`, `chat.rs`, `billing.rs`, `meters.rs`, `notifications.rs`: local HTTP transport only.
- `src/api/support.rs`: normalized support HTTP/SSE transport only.
- `frontend/src/features/profile/*`, `news/*`, `issues/*`, `chat/*`, `billing/*`, `meters/*`, `settings/*`: corresponding UI only.
- `frontend/src/features/support/*`: FAQ, support chat, and appeals UI only.

### Task 1: Resident-Service Domain Types and Common Pagination

**Files:**
- Create: `src/domain/resident.rs`
- Create: `src/domain/issue.rs`
- Create: `src/domain/chat.rs`
- Create: `src/domain/billing.rs`
- Create: `src/domain/meter.rs`
- Create: `src/domain/notification.rs`
- Modify: `src/domain/page.rs`
- Modify: `src/domain/mod.rs`
- Modify: `Cargo.toml`
- Test: `tests/resident_domain.rs`

**Core types:**

```rust
pub struct Money { pub amount: Decimal, pub currency: CurrencyCode }
pub struct ReadingValue { pub tariff_id: String, pub value: Decimal }
pub enum ServiceAvailability { Available, Unavailable { reason: String } }
pub enum IssueStatus { Draft, Submitted, InProgress, Resolved, Cancelled, Unknown(String) }
```

Add `rust_decimal = { version = "1.42.1", features = ["serde-with-str"] }` and extend Utoipa features with `chrono` and `decimal`; the public DTO layer still serializes every monetary/reading decimal as a JSON string.

- [ ] **Step 1: Write serialization/validation tests** for decimal money/readings, unknown statuses, UTC dates, cursor opacity, empty optional fields, phone/link sanitization, and provider-neutral IDs.
- [ ] **Step 2: Run and observe failure:** `cargo test --test resident_domain -- --nocapture`.
- [ ] **Step 3: Implement domain types** with `rust_decimal`, ISO currency/phone/link wrappers, tolerant enums, and explicit public-vs-secret fields.
- [ ] **Step 4: Run checks:** `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --test resident_domain`.
- [ ] **Step 5: Commit:** `git add Cargo.toml Cargo.lock src/domain tests/resident_domain.rs && git commit -m "feat: add resident service domain models"`.

### Task 2: Profile, Flats, Address Search, and Onboarding

**Files:**
- Create: `src/provider/mts/resident.rs`
- Create: `src/provider/mts/dto/resident.rs`
- Create: `src/service/resident.rs`
- Create: `tests/fixtures/profile.json`
- Create: `tests/fixtures/resident_flats.json`
- Create: `tests/fixtures/address_search.json`
- Create: `tests/contract/resident_profile.rs`

**Observed upstream routes:**

```text
GET/PATCH /api/v1/user/profile
GET/POST  /api/new/v3/user/flat
POST      /api/new/v2/user/flat/company/default
DELETE    /api/new/v1/realtyObjectCheck?flatId={flatId}
GET       /api/new/v1/search/address
GET       /api/new/v2/company/info
GET       /api/new/v2/user/flat/number
GET       /api/v2/realtyObject/{id}
GET       /vdome-smart-facade/estate/api/v1/residents/flats
```

- [ ] **Step 1: Write provider/service contracts** for profile read/update, both flat-list shapes, address pagination, company lookup variants, flat/account-number lookup, add/remove flat, upstream validation errors, duplicate flats, and cross-account IDs.
- [ ] **Step 2: Run and observe failure:** `cargo test --test resident_profile -- --nocapture`.
- [ ] **Step 3: Implement typed DTO mapping, onboarding state, and official MTS ID handoff metadata**, preferring smart-facade flat identity while preserving resident-rest fields needed for billing/meters; expose mutations only through `MutationService` intents containing a redacted final summary. Allow only `https://id.mts.ru/account`, `/account/personal`, `/account/safety`, `/account/links`, and a clearly marked `/account/payments` handoff as external account destinations: do not proxy/reimplement MTS ID profile, linked-service, or payment-card pages, never attach the server's cookie or ID token, and map the APK's “all services” action to the official `https://mts.ru/personal` or account page rather than duplicating the native SDK.
- [ ] **Step 4: Run focused/full tests:** `cargo test --test resident_profile && cargo test`.
- [ ] **Step 5: Commit:** `git add src/provider/mts src/service/resident.rs tests && git commit -m "feat: add profile and flat onboarding"`.

### Task 3: Additional Residents and Intercom User Linking

**Files:**
- Modify: `src/provider/mts/resident.rs`
- Modify: `src/service/resident.rs`
- Create: `tests/fixtures/residents.json`
- Create: `tests/contract/residents.rs`

**Observed upstream routes:**

```text
GET/POST   /api/registration/flat/{id}/residents
DELETE     /api/registration/flat/{id}/eviction
GET        /domofon/api/security/v1/users
POST       /domofon/api/security/v1/users/{userId}/link
DELETE     /domofon/api/security/v1/users/{userId}
DELETE     /gw-intercom/apartment/v1/apartments/{apartments}/tenants/{tenants}
```

- [ ] **Step 1: Write contracts** for list/invite/link/remove, normalized phone input, role/capability mapping, pending invitations, duplicate resident, last-owner protection, flat ownership, mismatched apartment/tenant IDs, and ambiguous mutation handling.
- [ ] **Step 2: Run and observe failure:** `cargo test --test residents -- --nocapture`.
- [ ] **Step 3: Implement resident aggregation** and one-use intents for invite/link/remove; never return intercom auth credentials present in a user payload.
- [ ] **Step 4: Run checks:** `cargo test --test residents && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit:** `git add src/provider/mts/resident.rs src/service/resident.rs tests && git commit -m "feat: manage additional residents"`.

### Task 4: News, Companies, Emergency Contacts, and Feedback

**Files:**
- Create: `src/provider/mts/news.rs`
- Modify: `src/provider/mts/resident.rs`
- Create: `src/provider/mts/dto/news.rs`
- Create: `src/service/news.rs`
- Create: `tests/fixtures/news_page.json`
- Create: `tests/fixtures/company.json`
- Create: `tests/contract/news_companies.rs`

**Observed upstream routes:**

```text
GET  /vdome-smart-facade/publications/api/v1/flats/{id}
GET  /vdome-smart-facade/publications/api/v1/{id}
GET  /vdome-smart-facade/estate/api/v1/companies/{id}
GET  /gw-intercom/company/v1/companies/{id}
POST /gw-intercom/feedback/v1/messages/intercom/{id}
```

- [ ] **Step 1: Write contracts** for news pagination/type filters/details, rich text sanitization, external link allowlisting, image proxying through the phase-2 media policy, company/emergency/intercom contacts, phone normalization, missing services, and feedback intent/validation.
- [ ] **Step 2: Run and observe failure:** `cargo test --test news_companies -- --nocapture`.
- [ ] **Step 3: Implement providers/services** with sanitized plain-text/allowlisted markup output, scoped company IDs, safe contact actions, and non-retried feedback dispatch.
- [ ] **Step 4: Run checks:** `cargo test --test news_companies && cargo test`.
- [ ] **Step 5: Commit:** `git add src/provider/mts src/service/news.rs tests && git commit -m "feat: add news companies and feedback"`.

### Task 5: MTS Support FAQ, Live Chat, Attachments, and Appeals

**Files:**
- Create: `src/domain/support.rs`
- Create: `src/provider/mts/support.rs`
- Create: `src/provider/mts/dto/support.rs`
- Create: `src/service/support.rs`
- Modify: `src/domain/mod.rs`
- Modify: `src/provider/mts/mod.rs`
- Modify: `src/provider/mts/dto/mod.rs`
- Modify: `src/service/mod.rs`
- Create: `tests/fixtures/support_config.json`
- Create: `tests/fixtures/support_history.json`
- Create: `tests/fixtures/support_appeals.json`
- Create: `tests/contract/support.rs`
- Create: `tests/support_content_policy.rs`

**Observed upstream contracts:**

```text
GET  https://online-chat.mts.ru/config-service/api/config?productName=vdome&platformName=ANDROID&version=3.23.0
GET  https://online-chat.mts.ru/api/histories?channelName=vdome&chatVersion=3.23.0&messageId={messageId}&direction={UP|DOWN}&appealId={appealId}&pageSize={pageSize}
POST https://online-chat.mts.ru/api/files?channelName=vdome&chatVersion=3.23.0
GET  https://support.mts.ru/my-appeals/api/appeals?serviceName=vdome
GET  https://support.mts.ru/my-appeals/api/appeals/{id}
POST https://support.mts.ru/my-appeals/api/appeals/{number}/operation
WSS  wss://online-chat.mts.ru/ws/chat
FAQ  https://m.support.mts.ru/mpvdome
```

HTTP requests use `Authorization: Bearer <idToken>`. The first WebSocket frame is a typed authentication envelope containing a generated event ID, ID token, `channelName=vdome`, bounded application/device metadata, selected official referrer/company ID, optional appeal ID, settings, and enabled features. Provider frame types remain private and map to normalized text/file/image/operator/typing/delivery/read/reaction/bot-action/appeal events.

- [ ] **Step 1: Write HTTP/WebSocket contracts** for config, bidirectional history pagination, appeals list/detail, typed first-frame authentication, reconnect/resume, message ordering/deduplication, operator/typing state, delivery/read state, unknown frame types, close codes, token refresh, bounded frames, and proof that ID tokens never enter domain/API/log output.
- [ ] **Step 2: Write content and upload policy tests** for safe text/link/official support-phone actions, supported returned bot actions/reactions, JPEG/PNG/PDF attachment signature/count/size limits, streaming upload cleanup, safe official file downloads, blocked HTML/script/data/file/private-host URLs, and allowlisted external micro-app/poll handoff with no token/cookie propagation.
- [ ] **Step 3: Write mutation tests** for sending text, uploading/sending files or images, reactions, and appeal close: local-session/appeal/message binding, one-use intent summaries, idempotency identifiers, duplicate click, reconnect race, and unknown outcomes without retry.
- [ ] **Step 4: Run and observe failure:** `cargo test --test support --test support_content_policy -- --nocapture`.
- [ ] **Step 5: Implement the typed clean-room support client and service**, using the existing `TokenSource::id_token`, `productName/channelName=vdome`, support SDK compatibility version `3.23.0`, a bounded reconnect loop, normalized safe content, and redacted realtime events. Do not embed the proprietary Support SDK or expose a raw WebSocket/HTTP proxy.
- [ ] **Step 6: Run checks and commit:** `cargo test --test support --test support_content_policy && cargo test && cargo clippy --all-targets --all-features -- -D warnings && git add src/domain src/provider/mts src/service tests && git commit -m "feat: add typed MTS support client"`.

### Task 6: Issues, Messages, Photos, Editing, and Cancellation

**Files:**
- Modify: `Cargo.toml`
- Create: `src/provider/mts/issues.rs`
- Create: `src/provider/mts/dto/issues.rs`
- Create: `src/service/issues.rs`
- Create: `src/uploads.rs`
- Create: `tests/fixtures/issues_page.json`
- Create: `tests/fixtures/issue_details.json`
- Create: `tests/contract/issues.rs`
- Create: `tests/upload_policy.rs`

**Observed current routes:**

```text
GET  /vdome-smart-facade/issues/api/v1/categories/flats/{id}
GET  /vdome-smart-facade/issues/api/v1
GET  /vdome-smart-facade/issues/api/v1/{id}/details
POST /vdome-smart-facade/issues/api/v1
POST /vdome-smart-facade/issues/api/v1/{id}/supplement
POST /vdome-smart-facade/issues/api/v1/{id}/cancel
POST /vdome-smart-facade/issues/api/v1/{id}/photos
POST /vdome-smart-facade/issues/api/v1/messages/{id}/files
```

Add `infer = "0.22.0"` for signature-based upload classification; PDF/HEIC cases not recognized by the crate receive explicit bounded magic-byte checks in `uploads.rs`.

- [ ] **Step 1: Write list/detail/category contracts** covering cursor/offset pagination, messages, statuses, creator, optional deadlines, attachment metadata, unknown additive fields, missing capability, and cross-flat issue IDs.
- [ ] **Step 2: Write mutation/upload tests** for create/supplement/cancel/message attachments, one-use intents, 10-file count, 10 MiB per file, 25 MiB aggregate, JPEG/PNG/HEIC/PDF signature checks, filename stripping, truncated streams, temporary-file permission/cleanup, timeout, and ambiguous results.
- [ ] **Step 3: Run and observe failure:** `cargo test --test issues --test upload_policy -- --nocapture`.
- [ ] **Step 4: Implement issue mapping and streaming multipart**, staging only when Reqwest needs a known length, consuming intents before dispatch, and publishing redacted issue-update events after confirmed results.
- [ ] **Step 5: Run checks and commit:** `cargo test --test issues --test upload_policy && cargo clippy --all-targets --all-features -- -D warnings && git add Cargo.toml Cargo.lock src/provider/mts src/service/issues.rs src/uploads.rs tests && git commit -m "feat: add issue management and attachments"`.

### Task 7: Resident-to-Management Chat and Read State

**Files:**
- Create: `src/provider/mts/chat.rs`
- Create: `src/provider/mts/dto/chat.rs`
- Create: `src/service/chat.rs`
- Create: `tests/fixtures/chat_page.json`
- Create: `tests/contract/chat.rs`

**Observed upstream routes:**

```text
GET  /api/v1/chat/message?companyId={companyId}&query={query}&limit={limit}&offset={offset}&neverAt={timestamp}
POST /api/v1/chat/message
POST /api/v1/chat/message/read
```

- [ ] **Step 1: Write contracts** for pagination, `neverAt`, sender/direction mapping, read/unread state, Unicode/length validation, company/flat scoping, duplicate send protection through an intent, mark-read batching, optional chat capability, and schema drift.
- [ ] **Step 2: Run and observe failure:** `cargo test --test chat -- --nocapture`.
- [ ] **Step 3: Implement the chat provider/service**, preserving server timestamps/order, publishing redacted `chat_updated` events, and storing no message bodies outside bounded response caches.
- [ ] **Step 4: Run checks:** `cargo test --test chat && cargo test`.
- [ ] **Step 5: Commit:** `git add src/provider/mts/chat.rs src/provider/mts/dto/chat.rs src/service/chat.rs tests && git commit -m "feat: add resident management chat"`.

### Task 8: Services, Invoices, Official Payment Links, and Offline Status

**Files:**
- Create: `src/provider/mts/billing.rs`
- Create: `src/provider/mts/dto/billing.rs`
- Create: `src/service/billing.rs`
- Create: `tests/fixtures/invoices.json`
- Create: `tests/fixtures/invoice_details.json`
- Create: `tests/contract/billing.rs`
- Create: `tests/payment_redirect.rs`

**Observed upstream routes:**

```text
GET   /api/new/v1/inv/invoice/clientAccount
GET   /api/new/v1/inv/invoice/{id}/details
GET   /api/new/v1/inv/invoice/{id}/payLink
PATCH /api/new/v1/inv/invoice/{id}/status/offline
POST  /payment-service/api/payment/session
POST  /payment-service/api/v2/payment/session
GET   /gw-intercom/billing/v2/users/{userId}/services
```

- [ ] **Step 1: Write billing contracts** for active/archive invoices, service composition, money/currency, due/paid/overdue states, itemized details, account/flat ownership, missing billing capability, and provider errors.
- [ ] **Step 2: Write redirect/mutation tests** allowing only returned HTTPS hosts after exact allowlist validation, stripping fragments/userinfo, no automatic redirect follow, expiry, new-window response metadata, one-use payment-session/offline-status intents, and no card/payment form data in local requests/logs.
- [ ] **Step 3: Run and observe failure:** `cargo test --test billing --test payment_redirect -- --nocapture`.
- [ ] **Step 4: Implement billing and payment-link policy**, returning `{url, expiresAt, rel:"noopener noreferrer"}` only after validation and distinguishing unknown payment mutation outcomes.
- [ ] **Step 5: Run checks and commit:** `cargo test --test billing --test payment_redirect && cargo clippy --all-targets --all-features -- -D warnings && git add src/provider/mts/billing.rs src/provider/mts/dto/billing.rs src/service/billing.rs tests && git commit -m "feat: add invoices and safe payment redirects"`.

### Task 9: Meters, Reading History/Statistics, and Submission

**Files:**
- Create: `src/provider/mts/meters.rs`
- Create: `src/provider/mts/dto/meters.rs`
- Create: `src/service/meters.rs`
- Create: `tests/fixtures/meters.json`
- Create: `tests/fixtures/meter_readings.json`
- Create: `tests/contract/meters.rs`

**Observed upstream routes:**

```text
GET  /api/new/v2/mtr/meter/flat/{id}
GET  /api/new/v2/mtr/meter/readings/{id}
POST /api/new/v1/mtr/meter/readings
POST /meter-manager/device/v1/flats
POST /meter-manager/metric/v1/devices
```

- [ ] **Step 1: Write contracts** for meter types/units/tariffs, latest values, history/statistics, query ranges, new-manager fallback, unknown meter kinds, flat ownership, decimal precision, monotonic/non-decreasing validation where required, submission windows, duplicate tariffs, one-use summaries, and ambiguous results.
- [ ] **Step 2: Run and observe failure:** `cargo test --test meters -- --nocapture`.
- [ ] **Step 3: Implement read models and submission policy** using provider-supplied validation limits when present, conservative local validation otherwise, no optimistic success on timeout, and a redacted `meter_reading_submitted` event after confirmation.
- [ ] **Step 4: Run checks:** `cargo test --test meters && cargo test && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit:** `git add src/provider/mts/meters.rs src/provider/mts/dto/meters.rs src/service/meters.rs tests && git commit -m "feat: add meters and reading submission"`.

### Task 10: Provider Notification Preferences

**Files:**
- Create: `src/provider/mts/notifications.rs`
- Create: `src/provider/mts/dto/notifications.rs`
- Create: `src/service/notifications.rs`
- Create: `tests/fixtures/notification_settings.json`
- Create: `tests/contract/notification_settings.rs`

**Observed upstream route:** `GET/PUT /api/new/v1/notification/settings`.

- [ ] **Step 1: Write contracts** for setting IDs/categories, unknown categories, partial availability, unchanged updates, full-list replacement semantics, intent summary, concurrent revision checks, and ambiguous PUT outcomes. Explicitly assert `/api/new/v1/notification/token` is not called because Android FCM registration is excluded.
- [ ] **Step 2: Run and observe failure:** `cargo test --test notification_settings -- --nocapture`.
- [ ] **Step 3: Implement read/update service** with an ETag-like local revision hash over the fetched list, preserving unknown entries during updates and separating provider preferences from local Browser Notification/webhook settings.
- [ ] **Step 4: Run checks:** `cargo test --test notification_settings && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit:** `git add src/provider/mts/notifications.rs src/provider/mts/dto/notifications.rs src/service/notifications.rs tests && git commit -m "feat: add VDome notification preferences"`.

### Task 11: Resident-Service Local APIs

**Files:**
- Create: `src/api/residents.rs`
- Create: `src/api/news.rs`
- Create: `src/api/companies.rs`
- Create: `src/api/issues.rs`
- Create: `src/api/chat.rs`
- Create: `src/api/billing.rs`
- Create: `src/api/meters.rs`
- Create: `src/api/notifications.rs`
- Create: `src/api/support.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/app.rs`
- Create: `tests/api/resident_services.rs`

**Route groups:**

```text
/api/v1/profile
/api/v1/flats/*
/api/v1/residents/*
/api/v1/news/*
/api/v1/companies/*
/api/v1/feedback/*
/api/v1/issues/*
/api/v1/chat/*
/api/v1/billing/*
/api/v1/meters/*
/api/v1/notifications/preferences
/api/v1/support/config
/api/v1/support/faq
/api/v1/support/conversations/*
/api/v1/support/messages/*
/api/v1/support/files/*
/api/v1/support/appeals/*
/api/v1/mutation-intents/*
```

- [ ] **Step 1: Write Axum tests** enumerating every GET/list/detail route and mutation-intent kind, including FAQ/support history/files/appeals/send/reaction/close; verify pagination/limit/date/decimal/upload validation, CSRF/session checks, ownership, safe redirects, multipart bounds, stable error envelopes, SSE update emission, and no secret/PII leakage beyond the requested authorized resource.
- [ ] **Step 2: Run and observe failure:** `cargo test --test resident_services -- --nocapture`.
- [ ] **Step 3: Implement thin typed handlers** and OpenAPI schemas, sharing the generic one-use intent executor while restricting each intent to one concrete enum variant and payload hash.
- [ ] **Step 4: Run API/full checks:** `cargo test --test resident_services && cargo test`.
- [ ] **Step 5: Commit:** `git add src/api src/app.rs tests/api/resident_services.rs && git commit -m "feat: expose resident services API"`.

### Task 12: Complete Home-Services and Support Web UI

**Files:**
- Create: `frontend/src/features/profile/ProfilePage.tsx`
- Create: `frontend/src/features/profile/FlatOnboarding.tsx`
- Create: `frontend/src/features/profile/ResidentsPanel.tsx`
- Create: `frontend/src/features/news/NewsPage.tsx`
- Create: `frontend/src/features/companies/ContactsPage.tsx`
- Create: `frontend/src/features/issues/IssuesPage.tsx`
- Create: `frontend/src/features/issues/IssueEditor.tsx`
- Create: `frontend/src/features/chat/ChatPage.tsx`
- Create: `frontend/src/features/billing/BillingPage.tsx`
- Create: `frontend/src/features/meters/MetersPage.tsx`
- Create: `frontend/src/features/settings/ProviderNotifications.tsx`
- Create: `frontend/src/features/support/SupportPage.tsx`
- Create: `frontend/src/features/support/AppealsPage.tsx`
- Modify: `frontend/src/app/App.tsx`
- Test: `frontend/src/features/profile/ProfilePage.test.tsx`
- Test: `frontend/src/features/issues/IssuesPage.test.tsx`
- Test: `frontend/src/features/billing/BillingPage.test.tsx`
- Test: `frontend/src/features/meters/MetersPage.test.tsx`
- Test: `frontend/src/features/support/SupportPage.test.tsx`

- [ ] **Step 1: Write component/flow tests** for every loading/empty/unsupported/error state, profile/onboarding/residents, official MTS ID link boundaries, news/details/contacts/feedback, FAQ, support history/live/operator/typing/send/files/reactions/bot actions/appeals/close, issue create/detail/message/files/edit/cancel, management chat/read state, active/archive invoice/details/pay redirect, meters/history/statistics/submission, preference editing, mutation summaries, ambiguous outcomes, keyboard/mobile/reduced-motion behavior, and Russian validation messages.
- [ ] **Step 2: Run and observe failure:** `npm --prefix frontend test -- src/features/profile/ProfilePage.test.tsx src/features/support/SupportPage.test.tsx src/features/issues/IssuesPage.test.tsx src/features/billing/BillingPage.test.tsx src/features/meters/MetersPage.test.tsx`.
- [ ] **Step 3: Implement responsive feature pages** with route-level error boundaries, accessible forms/dialogs, explicit file preview/removal, safe normalized support content, separate official-link contexts using `noopener,noreferrer`, decimal input as strings, official payment links in a new secure context, and no unsupported module dead ends.
- [ ] **Step 4: Run frontend checks:** `npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build`.
- [ ] **Step 5: Commit:** `git add frontend && git commit -m "feat: add resident services web UI"`.

### Task 13: Resident-Service E2E and Live Read-Only Gate

**Files:**
- Create: `tests/mock_provider/resident_services.rs`
- Create: `frontend/e2e/resident-services.spec.ts`
- Create: `docs/runbooks/live-resident-services.md`
- Modify: `README.md`

- [ ] **Step 1: Extend the deterministic mock provider** with supported and unavailable company capabilities, resident onboarding, news, contacts, support HTTP/WebSocket/history/files/appeals, issue conversations/uploads, management chat, invoices/payment redirects, meters, preferences, validation errors, rate limits, and ambiguous mutation outcomes.
- [ ] **Step 2: Write Playwright coverage** for all phase-4 UI flows, including support reconnect/send/upload/reaction/appeal close and hostile content, MTS ID official-link boundaries, mobile/keyboard, file validation, explicit mutation summaries/cancellation, safe payment new-window metadata, SSE refresh, and browser-visible token/card-data scans.
- [ ] **Step 3: Run the phase gate:** `cargo test && npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build && npm --prefix frontend exec -- playwright test frontend/e2e/resident-services.spec.ts`.
- [ ] **Step 4: Run only live read-only checks** for profile, flats, residents, news, companies, support config/history/appeals, issue categories/list/details, management chat list, services/invoices/details, meters/history, and notification preferences. Stop and ask for immediate user confirmation before any real create/update/delete/send/upload/react/close/submit/payment/offline-status operation.
- [ ] **Step 5: Commit:** `git add tests/mock_provider frontend/e2e docs/runbooks/live-resident-services.md README.md && git commit -m "test: cover resident service flows"`.

## Phase Acceptance Gate

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
npm --prefix frontend run typecheck
npm --prefix frontend test
npm --prefix frontend run build
npm --prefix frontend exec -- playwright test frontend/e2e/resident-services.spec.ts
git diff --check
```

The phase is complete only when every included home-service and support capability has a typed API and complete UI state coverage, optional modules degrade independently, support/issue uploads and payment redirects satisfy their security tests, decimal data round-trips exactly, no ID token reaches the browser, and real mutations remain paused for immediate user confirmation.
