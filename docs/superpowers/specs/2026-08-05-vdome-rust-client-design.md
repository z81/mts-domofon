# VDome (MTS) Core Intercom Rust API and Web UI — Design

Date: 2026-08-06

Status: approved core-only scope

## 1. Goal

Build an independent Rust implementation of the current MTS VDome resident client in `/Users/alex/dev/domru-mts`, using the Rust implementation from `domru-z81` only as a structural reference.

The product is deliberately limited to the main intercom experience:

- current MTS ID/SMS authorization;
- flats and intercom-capability selection;
- intercoms, access points, manual door control, cameras, archive, and events;
- persistent incoming audio/video call reception and browser call handling;
- Browser Notifications, SSE, and signed webhooks;
- a stable local Axum API and complete responsive Web UI for those features;
- one deployable Rust service and Docker image.

This is a personal interoperability client. It reproduces independently observed protocol behavior and independently defined data models. It does not copy proprietary APK code, UI, artwork, analytics, or bundled assets.

## 2. Sources of truth

### Current product

- XAPK: `/Users/alex/Downloads/VDome_3.0.1_APKPure.xapk`
- SHA-256: `1a857b46a2e7aec6d9e01458a9c8e8961e2a1d96a861bd4e1bdcd1ccf368c92b`
- Package: `ru.mts.vdome.resident`
- Version: `3.0.1` (`300001001`)
- Primary gateway: `https://gateway.vdome.mts.ru/`
- Resident API used only where a core flat/device contract requires it: `https://resident-rest.vdome.mts.ru`
- Call transport: `https://webrtc.domofon.mts.ru/ws`, normalized to `wss`

The APK and safe unauthenticated probes show that production authorization uses gateway `WEB_SSO`, not the obsolete direct-SMS API.

### Structural reference

`/Users/alex/dev/domru-z81`, Rust `origin/main`, supplies the desired service shape:

- async Rust and Axum;
- embedded Web UI;
- encrypted local session/config persistence;
- secure media proxying;
- persistent SIP/call monitoring;
- SSE and webhooks;
- Docker packaging.

Its Dom.ru provider protocol is not reused.

### Historical reference

`/Users/alex/Downloads/cs12d7a.4pda.ws` (MTS Domofon 1.1.7) is historical evidence only. Its hosts and direct token/SMS flow are not production defaults.

## 3. Product boundary

### Included

1. Anonymous gateway `/config` and `/feature` bootstrap with explicit `AUTH_TYPE=WEB_SSO` validation.
2. MTS ID/WebSSO login, phone/SMS and other requested challenges, token refresh, logout, and restart recovery.
3. List and select flats already linked to the authenticated account; show per-flat intercom capabilities.
4. A compact dashboard for core device/camera/call status, without favorites or general home services.
5. Intercoms, doors, gates, barriers, elevators/access points, and multiple locks where provisioned.
6. Manual door/access-point opening through an explicit, short-lived, one-use confirmation intent.
7. Cameras: snapshots, live video, archive playback, event timeline, and bounded archive clip download.
8. Persistent incoming audio/video call detection without requiring an open browser tab.
9. Call actions: accept, reject, ignore, hang up, and manual opening during an active call.
10. Browser call controls for microphone, outgoing camera, received audio, and output-device selection when supported.
11. Active/recent call state, provider call history where available, per-flat call settings, and call blocking where exposed.
12. In-app realtime updates, Browser Notifications, and optional signed outbound webhooks.
13. Stable versioned local API, responsive Russian Web UI, health diagnostics, OpenAPI, security controls, deterministic mock tests, and Docker release packaging.

### Explicitly excluded

- Support, FAQ, support conversations, files, reactions, and appeals;
- automatic door opening in every form, including always/next-call/timed modes;
- favorites and the APK main-settings favorite selection;
- user-facing legal documents, PDF/document viewer, terms/privacy screens, and arbitrary attachment proxying;
- theme selection, sharing, rating, Play Store actions, and About screen;
- resident profile editing, MTS Profile/All Services SDK pages, residents/additional users, and resident management;
- flat onboarding, address search, and add/remove-flat workflows; the client operates on flats already linked to the account;
- news, publications, management contacts, feedback, issues/tickets, issue attachments, and management chat;
- services, invoices, payment links/status, meters, readings, and notification-preference management;
- individual/vehicle access requests and passes;
- marketplace catalog, basket, checkout, and orders;
- event deletion;
- Wear OS, Android widgets, NFC/HCE, BLE/local-hardware access, FCM mobile-device registration, Android intents, and native update APIs;
- TNPS, proprietary telemetry, analytics, crash reporting, ESMART/developer/debug menus, and copied branding/assets;
- storage or processing of bank-card data.

Developer documentation such as README, OpenAPI, deployment, security, and troubleshooting guides remains part of the repository; “documents” above means end-user document/PDF functionality from the APK.

## 4. Chosen approach

Implement a strongly typed core VDome provider with a stable local domain API. Do not expose an unrestricted raw upstream proxy and do not retain dormant provider/UI modules for excluded features.

Reasons:

- WebSSO needs coordinated challenge, cookie, callback, and token state;
- media URLs require ownership checks, safe proxying, and HLS rewriting;
- calls combine persistent signaling, SIP state, REST actions, RTP/WebRTC, and realtime events;
- manual opening and call control need explicit mutation boundaries and ambiguous-outcome handling;
- removing unused resident/commerce/support abstractions keeps the service understandable and testable.

The result remains one deployable service, with narrow provider, domain, API, background-task, media/call, and UI layers.

## 5. High-level architecture

```text
Browser / local integrations
        |
        | JSON API, SSE, call WebSocket, bounded media proxy
        v
Axum application
  |-- Auth/session coordinator
  |-- Flat/device/media services
  |-- Manual mutation-intent service
  |-- Persistent call engine and WebRTC bridge
  |-- Realtime bus and signed webhook worker
  |-- Health/capability diagnostics
  `-- Encrypted secret/config store
        |
        | typed provider requests only
        v
VDome provider layer
  |-- MTS ID / WebSSO
  |-- gateway.vdome.mts.ru
  |-- resident-rest.vdome.mts.ru for required core routes
  |-- webrtc.domofon.mts.ru
  `-- authorized camera/stream hosts returned by VDome
```

## 6. Repository structure

```text
domru-mts/
├── Cargo.toml
├── Dockerfile
├── docker-compose.yml
├── README.md
├── docs/
├── frontend/
│   ├── package.json
│   └── src/
│       ├── app/
│       ├── features/
│       │   ├── auth/
│       │   ├── home/
│       │   ├── devices/
│       │   ├── cameras/
│       │   ├── events/
│       │   └── calls/
│       └── lib/
├── src/
│   ├── main.rs
│   ├── app.rs
│   ├── config.rs
│   ├── error.rs
│   ├── state.rs
│   ├── storage.rs
│   ├── api/
│   │   ├── auth.rs
│   │   ├── dashboard.rs
│   │   ├── devices.rs
│   │   ├── media.rs
│   │   ├── events.rs
│   │   ├── calls.rs
│   │   ├── realtime.rs
│   │   ├── config.rs
│   │   └── health.rs
│   ├── domain/
│   ├── provider/mts/
│   │   ├── bootstrap.rs
│   │   ├── client.rs
│   │   ├── device.rs
│   │   ├── sso.rs
│   │   ├── dashboard.rs
│   │   ├── intercom.rs
│   │   ├── media.rs
│   │   ├── events.rs
│   │   └── calls.rs
│   ├── service/
│   ├── media/
│   ├── calls/
│   └── realtime/
└── tests/
    ├── fixtures/
    ├── contract/
    ├── api/
    ├── mock_provider/
    └── mock_sip/
```

Excluded feature modules are not scaffolded. Large core areas remain separate so an upstream DTO change cannot grow a single provider or route file indefinitely.

## 7. Backend components

### 7.1 Bootstrap and provider HTTP client

Before login, a restricted anonymous client reads only gateway `GET /config` and `GET /feature`. It normalizes known `{configName, platform, value}` and `{featureName, value}` entries, preserves unknown names for redacted compatibility diagnostics, and requires `AUTH_TYPE=WEB_SSO`. It never falls back to legacy direct SMS.

Authorized requests add only the headers required by the observed VDome contract:

- `Authorization: Bearer <access token>`;
- `X-Device-Token` when issued;
- `X-Device-Type: ANDROID` where required by the gateway;
- a fresh UUID in `Request-Id`;
- bounded JSON content negotiation.

Tokens, cookies, phone numbers, OTP values, SIP credentials, media credentials, and authorization headers are always redacted. Provider DTOs map immediately into stable domain types. Additive unknown fields are tolerated; required ownership/action identifiers remain strict.

### 7.2 Authentication coordinator

Login is challenge-driven rather than hard-coded as two calls.

The coordinator will:

1. Generate and persist a stable encoded device descriptor compatible with MTS Login SDK 5.8.0.
2. Start authorization for client `VDome` with the registered gateway callback.
3. Keep the MTS ID cookie jar and callback state server-side.
4. Expose only the current phone, SMS OTP, captcha, password, account-selection, or consent challenge requested by MTS ID.
5. Validate state and callback destinations strictly.
6. Parse and encrypt only the WebSSO, access, refresh, and device token material required by core VDome routes; zeroize an unused returned ID token instead of persisting it.
7. Serialize refresh, retry one proven-safe read after `401`, restore the session after restart, and erase it on logout.

Secret challenge values live only for one request and are never persisted or logged.

### 7.3 Flats, dashboard, and devices

The flat service lists flats already linked to the account, remembers one selected flat, and derives only core capabilities: intercom/access points, cameras/archive/events, calls, and provider call settings.

The device service merges smart-facade and legacy intercom responses deterministically:

- smart-facade identity and availability win;
- legacy fields may enrich labels, lock numbers, call metadata, and media identifiers;
- unknown access-point kinds remain visible as unavailable/unknown rather than being guessed;
- every device, camera, event, and mutation is revalidated against the selected owned flat.

There is no favorite read/write path or local favorite fallback.

### 7.4 Manual opening

Every door, gate, barrier, elevator, multi-lock, and open-during-call action is manual.

The browser first requests a short-lived intent containing a server-generated Russian summary. The intent is bound to the local session, flat, access point, lock, action, and optional active-call revision/session reference. Execution atomically consumes the intent before network dispatch.

The service never retries an opening after dispatch becomes ambiguous. It returns one of confirmed, rejected, or unknown and publishes a redacted result event. There is no scheduler, rule engine, automatic-open state, or automatic response to an incoming call.

### 7.5 Camera media, archive, and events

The browser never receives reusable upstream authorization credentials.

The media layer will:

- fetch authorized snapshots with MIME/signature and byte limits;
- discover live/archive URLs only for an owned camera;
- proxy HLS manifests/segments when authentication or CORS requires it;
- rewrite nested playlists and relative URLs to opaque same-origin leases;
- validate HTTPS, host, DNS resolution, redirects, ranges, sizes, timeouts, and concurrency;
- provide archive playback and bounded clip download for an explicit UTC range;
- expose a read-only paginated event timeline and event markers.

There is no arbitrary URL proxy and no event-deletion endpoint.

### 7.6 Calls and browser media

The call subsystem remains active when no browser is open.

- A supervised engine obtains current SIP credentials and connects to the configured VDome WebSocket transport over `wss`.
- It registers, refreshes credentials, responds to keepalives, reconnects with capped jittered backoff, and treats SIP `INVITE` as the authoritative incoming-call signal.
- A pure reducer owns ringing, connecting, active, ending, and ended transitions plus revision/race validation.
- REST call actions preserve the observed opaque `action_route_id`; open-during-call uses the actual access point plus `X-Call-Session-ID` where required.
- The browser uses a local signaling WebSocket for offer/answer/ICE and WebRTC for audio/video.
- Direct browser-compatible media is preferred; otherwise an internal SIP/RTP-to-WebRTC bridge relays negotiated codecs without recording or persisting media.
- Accept, reject, ignore, hangup, and open actions require typed one-use intents and are never blindly retried after an ambiguous dispatch.
- Microphone, outgoing camera, received audio, and supported output-device selection are local browser controls.

No raw SIP message, SDP credential, media key, audio, or video is persisted.

### 7.7 Realtime and webhooks

Normalized secret-free events include:

- authentication/connection state;
- device/camera availability changes discovered by bounded polling;
- new intercom/camera event metadata;
- incoming, ringing, answered, and ended calls;
- manual door-open results.

The local UI receives events over resumable SSE; the call surface also receives state through its signaling WebSocket. Browser Notifications require explicit browser permission.

Optional webhooks are signed with HMAC-SHA-256, queued independently from call registration, use short timeouts and bounded retries only for safe delivery attempts, and reject unsafe destinations/DNS rebinding. They never include tokens, SDP, SIP payloads, upstream media URLs, or unnecessary personal data.

## 8. Stable local API

Canonical routes are versioned under `/api/v1`:

```text
/api/v1/auth/*
/api/v1/session
/api/v1/flats
/api/v1/flats/selected
/api/v1/dashboard
/api/v1/devices
/api/v1/cameras/:id/snapshot
/api/v1/cameras/:id/live
/api/v1/cameras/:id/archive
/api/v1/cameras/:id/archive/download
/api/v1/media/:lease/:resource
/api/v1/events
/api/v1/call-settings
/api/v1/calls
/api/v1/calls/:id
/api/v1/calls/:id/intents
/api/v1/calls/:id/intents/:intent/execute
/api/v1/access/intents
/api/v1/access/intents/:intent/execute
/api/v1/realtime                 # SSE
/api/v1/calls/signaling          # WebSocket
/api/v1/config
/api/v1/capabilities
/api/v1/openapi.json
/health/live
/health/ready
```

Compatibility aliases matching `domru-z81` are provided only where semantics stay safe: `/api/login`, `/api/confirm`, `/api/session`, `/api/logout`, `/api/places`, `/api/snapshot/:camera_id`, `/api/stream/:camera_id`, `/api/archive/:camera_id`, and `/api/open-door`. `/api/open-door` cannot bypass the canonical confirmation-intent flow.

There is no generic provider request, URL proxy, automatic-open, favorite, support, resident-service, payment, pass, marketplace, document, or event-deletion route.

Errors use a stable envelope with code, Russian-facing message, retryability, local request ID, and optional retry delay. Raw upstream bodies, stack traces, tokens, credentials, and personal data are never returned.

## 9. Web UI

The frontend is an original responsive TypeScript/React application built by Vite and embedded in the Rust release artifact. Node is a build-time dependency only.

Top-level navigation:

1. Home: selected flat, core capability/connection state, recent call/event indicators.
2. Devices: intercoms, doors, gates, barriers, elevators, locks, and manual opening.
3. Cameras: grid, snapshot, live video, archive controls, event timeline, and clip download.
4. Calls: incoming banner, active call surface, recent/history view, and call settings.
5. Connection settings: Browser Notification permission, webhook configuration/status, session/logout, and redacted health diagnostics.

Interaction rules:

- every applicable screen is scoped by the selected owned flat;
- absent provider capabilities display a precise unavailable state without breaking other core screens;
- all door and call mutations show a final server-generated summary and require deliberate execution;
- incoming calls always show an in-app banner and may show a Browser Notification after permission;
- camera URLs and reusable credentials never appear in DOM, browser storage, or errors;
- call media permissions are requested only when previewing/accepting;
- accessibility covers keyboard navigation, focus management, labeled controls, reduced motion, and adequate contrast;
- the UI has one fixed accessible visual theme and no favorite, sharing, document, support, or resident-service screens.

## 10. Persistence and local security

Persisted data is limited to:

- encrypted provider session/token bundle;
- stable MTS device identity;
- selected flat;
- provider call settings and bounded non-sensitive call/event cache metadata where needed;
- local session, webhook, polling, and deployment configuration.

Secrets are encrypted with AES-GCM. `VDOME_SECRET_KEY` may provide the key; otherwise an owner-only random key is generated in the data directory. Secret/config writes are atomic and owner-only.

Defaults:

- bind to `127.0.0.1`;
- require a local password or trusted reverse-proxy identity before non-loopback exposure;
- same-origin UI/API, HttpOnly SameSite cookies, CSRF and Origin checks;
- strict body/upload/frame/media limits;
- no secrets in frontend storage, logs, panic output, metrics, OpenAPI, or browser-visible payloads;
- no arbitrary provider/media/webhook proxying;
- no persistent call recordings.

## 11. Error handling and resilience

Provider errors map to stable categories: challenge required, expired/revoked session, unavailable capability, permission denied, validation failure, rate limit, transient outage, media/signaling failure, upstream schema incompatibility, and local storage/configuration failure.

Rules:

- retry only idempotent reads with capped exponential backoff/jitter and honor `Retry-After`;
- serialize token refresh and retry a failed read once;
- never blindly retry manual opening, call actions, or call-setting mutations after ambiguous dispatch;
- reconnect call/realtime transports without blocking request handling;
- retain bounded last-known read-only state during transient outages;
- fail one optional core capability independently;
- expose actionable Russian messages and redacted structured diagnostics.

## 12. Testing strategy

Automated coverage includes:

- unit tests for config, URL/header policy, redaction, storage, refresh serialization, DTO mapping, ownership, mutation intents, playlist rewriting, SIP parsing/digest/dialogs, call transitions, SDP/RTP/WebRTC, SSE, and webhook signing;
- fully synthetic provider/SIP/media fixtures with no real account data;
- mock upstream contract tests for bootstrap, authentication, flats, dashboard, devices, cameras, archive, events, call settings, SIP credentials, and call actions;
- Axum tests for every canonical route/alias, validation, local auth, CSRF/Origin, secret non-disclosure, media limits, and absence of excluded routes;
- React component tests for all loading, empty, unavailable, reconnect, error, permission, confirmation, and ambiguous-outcome states;
- Playwright flows for login, flat selection, devices/manual opening, live/archive/events, incoming audio/video calls, all call controls, notifications, reconnect, mobile/keyboard use, and excluded-navigation absence;
- Docker build/startup and no-build-tools runtime smoke tests.

No fixture or artifact may contain real phone numbers, addresses, device IDs, camera URLs, SIP credentials, call IDs, tokens, or media.

## 13. Controlled live verification

After all mock suites pass:

1. Ask the user to perform MTS ID login and enter the real SMS code through the local UI.
2. Run read-only checks for provider bootstrap, session, flats, dashboard, devices, snapshots, live/archive discovery, event listing, call settings, and health.
3. With the user present, observe one self-initiated incoming call without controlling it; verify the in-app event, Browser Notification, and optional webhook while the call expires naturally.
4. Ask for immediate confirmation before each specific real manual opening, accept, reject, ignore, hangup, open-during-call, call-block, or call-setting update.
5. Never combine approvals: one confirmation authorizes only the described next action. Record only pass/fail plus redacted request/version IDs.

Live responses are never committed unless manually reduced to synthetic, non-identifying shapes.

## 14. Deployment

The release is one Rust service with embedded frontend assets.

```text
VDOME_BIND=127.0.0.1:3000
VDOME_DATA_DIR=./data
VDOME_SECRET_KEY=<optional external key>
VDOME_LOCAL_PASSWORD_HASH=<required off loopback unless trusted proxy auth is configured>
WEBHOOK_URL=<optional>
WEBHOOK_SECRET=<optional>
RUST_LOG=info
TZ=Europe/Moscow
```

Docker uses a Node build stage, Rust build stage, and minimal non-root runtime. `./data` is a persistent owner-only volume. Liveness reports process state; readiness reports configured core dependencies without treating unavailable account features as a process failure.

## 15. Main risks and mitigations

### WebSSO changes

Keep MTS Login isolated behind an auth transport and cover every observed challenge with synthetic fixtures. Reject unknown challenge forms explicitly instead of guessing or falling back to legacy SMS.

### Account-specific devices

Build inventory from returned flat capabilities. Missing cameras, archive, elevators, or calls are valid per-flat states and must not break available intercom controls.

### Media URL diversity

Use opaque leases, trusted-response-derived host policy, DNS/redirect validation, bounds, and separate HLS/source adapters. Never expose a caller-controlled URL fetch.

### Call protocol uncertainty

Keep SIP transport, call reducer, provider REST actions, SDP/media bridge, browser signaling, and UI separate. Validate passive registration/ringing before any live action.

### Upstream schema drift

Use tolerant additive parsing, strict safety identifiers, redacted contract fixtures, explicit compatibility errors, and read-only smoke checks.

## 16. Acceptance criteria

The project is complete when:

1. A fresh instance can complete current MTS ID/SMS authorization, restore the encrypted session, refresh it, and log out.
2. Already linked flats, core capabilities, devices, access points, locks, and call settings map into stable local models.
3. Every manual opening requires a one-use confirmation intent and reports confirmed/rejected/unknown without unsafe retry.
4. Cameras support secure snapshot, live playback, archive, read-only event timeline, and bounded clip download where provisioned.
5. The background service detects real incoming calls and delivers SSE, Browser Notification, and optional signed webhook events with no open tab.
6. Browser audio/video and accept/reject/ignore/hangup/open-during-call controls work against the deterministic mock system and, when separately approved, the live account.
7. Tokens, cookies, SIP/media credentials, personal data, and upstream URLs remain server-side, encrypted/redacted, and absent from browser-visible output.
8. No excluded feature has an API route, provider module, UI navigation entry, background task, persistence model, or dormant scaffold.
9. Rust, API, frontend, E2E, security, dependency, container build, and container smoke gates pass from a clean checkout.
10. README documents setup, core feature scope, excluded features, security, API usage, Docker, troubleshooting, live-action confirmation policy, and unofficial/non-affiliated status.
