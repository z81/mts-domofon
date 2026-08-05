# VDome (MTS) Rust API and Web UI — Design

Date: 2026-08-05

Status: approved

## 1. Goal

Build an independent Rust implementation of the current MTS VDome resident client in `/Users/alex/dev/domru-mts`, using the Rust implementation from `domru-z81` as the structural reference.

The result must provide:

- a typed client for the current VDome backend;
- an Axum HTTP API for local integrations;
- a complete responsive Web UI for every APK capability that can meaningfully exist in a server/browser product;
- live video, archive, events, door control, and incoming audio/video calls;
- SSE, browser notifications, and webhooks;
- one deployable service and a Docker image.

This is a personal interoperability client. It must not copy proprietary application code, artwork, analytics, or bundled assets. Only observed protocol behavior and independently defined data models are reproduced.

## 2. Sources of truth

### Current product

- XAPK: `/Users/alex/Downloads/VDome_3.0.1_APKPure.xapk`
- SHA-256: `1a857b46a2e7aec6d9e01458a9c8e8961e2a1d96a861bd4e1bdcd1ccf368c92b`
- Package: `ru.mts.vdome.resident`
- Version: `3.0.1` (`300001001`)
- Primary gateway: `https://gateway.vdome.mts.ru/`
- Resident API: `https://resident-rest.vdome.mts.ru`
- Call signaling: `https://webrtc.domofon.mts.ru/ws`
- MTS support chat: `wss://online-chat.mts.ru/ws/chat`
- MTS support HTTP: `https://online-chat.mts.ru` and `https://support.mts.ru`

The APK and safe unauthenticated probes confirm that the production feature configuration uses `WEB_SSO`, not the legacy direct SMS API.

### Structural reference

`/Users/alex/dev/domru-z81`, branch `origin/main`, supplies the desired Rust/Axum shape:

- a small async Rust backend;
- embedded Web UI;
- persistent local session/config files;
- media proxying;
- SIP/call monitoring;
- SSE and webhook delivery;
- Docker packaging.

Its Dom.ru provider protocol is not reused for MTS.

### Legacy reference

`/Users/alex/Downloads/cs12d7a.4pda.ws` (MTS Domofon 1.1.7) is useful only for historical behavior and call-action terminology. Its hosts and direct token/SMS flow are obsolete and must not be the default implementation.

## 3. Product boundary

### Included

1. MTS ID/WebSSO authorization, including phone/SMS challenges and token refresh.
2. Profile, flats, onboarding, residents, and additional users.
3. Dashboard, services, favorites, and per-flat selection.
4. Intercoms, barriers, elevators/access points, doors, and multi-lock control.
5. Cameras, snapshots, live streams, archive playback, event timeline/deletion, and clip download.
6. Incoming call monitoring and browser audio/video call handling.
7. Call actions: accept, reject, ignore, hang up, and open during a call.
8. Call settings, explicitly armed automatic opening (`always`, `next call`, or 30-minute window), and call/guest event history.
9. Browser notifications, SSE, and outbound webhooks.
10. Individual and vehicle access requests/passes.
11. News and publication details.
12. Management companies, intercom companies, emergency contacts, and feedback.
13. Issues/tickets: categories, list, details, creation, supplement/edit, cancellation, messages, and photos/files.
14. Resident-to-management chat and read state.
15. Services, invoices, invoice details, payment links, and offline payment status where exposed by VDome.
16. Meters, readings, history/statistics, and submission of readings.
17. Marketplace catalog/search, product details, local basket, checkout/order creation, and order history.
18. Notification preferences and application settings that have a browser equivalent.
19. MTS support: FAQ, support conversation/history, live messages, safe file upload/download, operator/typing state, reactions/bot actions where returned, and appeal list/details/close.
20. VDome profile editing plus safe handoff to official MTS ID account, personal-data, safety, and linked-account pages.
21. About/legal/share/theme: version and compatibility status, official terms/privacy documents, current Play Store link, copy/share action, and a local light/dark/system theme.

### Explicitly excluded

- Wear OS application behavior;
- Android home-screen widgets;
- Android NFC virtual card/HCE behavior;
- Android Bluetooth/BLE or other local-hardware access flows;
- Android-only background components and intents;
- FCM device registration as a mobile device;
- proprietary telemetry, MTS analytics, crash reporting, and the TNPS survey SDK;
- hidden ESMART/developer/debug menus and internal diagnostics;
- native MTS Profile/All Services SDK screens, Android update UI, and native rating APIs;
- literal copies of APK UI/assets;
- storage or processing of bank-card details.

Browser notifications, SSE, webhooks, responsive shortcuts, local health/compatibility diagnostics, official MTS ID/account links, the current Play Store link, and official payment redirects replace the relevant mobile-only presentation features. The service never proxies an MTS ID account page or handles card data.

## 4. Chosen approach

Implement a strongly typed VDome provider client with a stable local domain API. Do not expose the upstream API as an unrestricted raw proxy.

Reasons:

- upstream authentication requires coordinated WebSSO state, cookies, callbacks, and three token classes;
- camera and archive URLs require secure proxying and playlist rewriting;
- calls combine signaling, REST actions, session IDs, and real-time state;
- typed boundaries permit fixture tests and make upstream schema changes visible;
- the Web UI should depend on a stable local contract, not APK-specific DTOs.

The implementation remains one deployable service, while source code is split into provider, domain, API, background-task, and UI layers.

## 5. High-level architecture

```text
Browser / integrations
        |
        | local JSON API, SSE, WebSocket, media proxy
        v
Axum application
  |-- Auth/session coordinator
  |-- Domain services
  |-- Realtime event bus and webhook dispatcher
  |-- Media proxy/playlist rewriter
  |-- Call signaling/media bridge
  `-- Persistent secret/config store
        |
        | typed provider requests
        v
VDome provider layer
  |-- MTS ID / WebSSO
  |-- gateway.vdome.mts.ru
  |-- resident-rest.vdome.mts.ru
  |-- webrtc.domofon.mts.ru
  |-- online-chat.mts.ru / support.mts.ru
  `-- authorized camera/stream hosts returned by VDome
```

## 6. Repository structure

Proposed structure:

```text
domru-mts/
├── Cargo.toml
├── Dockerfile
├── docker-compose.yml
├── README.md
├── docs/
├── frontend/
│   ├── package.json
│   ├── src/
│   └── dist/                 # generated, not committed unless build requires it
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
│   │   ├── residents.rs
│   │   ├── issues.rs
│   │   ├── chat.rs
│   │   ├── support.rs
│   │   ├── billing.rs
│   │   ├── meters.rs
│   │   ├── access.rs
│   │   ├── marketplace.rs
│   │   ├── notifications.rs
│   │   └── config.rs
│   ├── domain/
│   ├── provider/
│   │   └── mts/
│   │       ├── client.rs
│   │       ├── auth.rs
│   │       ├── dto.rs
│   │       ├── dashboard.rs
│   │       ├── intercom.rs
│   │       ├── media.rs
│   │       ├── events.rs
│   │       ├── calls.rs
│   │       ├── resident.rs
│   │       ├── support.rs
│   │       ├── access.rs
│   │       └── marketplace.rs
│   ├── realtime/
│   └── media/
└── tests/
    ├── fixtures/
    ├── contract/
    └── api/
```

Large feature areas remain separate so that provider DTO changes do not grow a single client or route file indefinitely.

## 7. Backend components

### 7.1 Provider HTTP client

The shared client adds the headers observed in VDome 3.0.1:

- `Authorization: Bearer <access token>` when authorized;
- `X-Device-Token` when issued;
- `X-Device-Type: ANDROID` where the gateway expects the mobile product identity;
- a fresh UUID in `Request-Id` per upstream request;
- JSON content negotiation.

Tokens and authorization headers must always be redacted from logs. Upstream `Request-Id` values are retained in structured error metadata.

Provider DTOs are converted immediately into local domain models. Tolerant deserialization is used for additive upstream changes, while required safety-critical identifiers remain strict.

Before authentication, a restricted anonymous bootstrap client reads only gateway `GET /config` and `GET /feature`. It normalizes known `{configName, platform, value}` and `{featureName, value}` entries, preserves unknown names for diagnostics, and requires `AUTH_TYPE=WEB_SSO`. Android minimum/latest-version values are compatibility observations rather than forced-update rules for this independent Rust/Web release; no result may trigger fallback to the legacy direct-SMS API.

### 7.2 Authentication coordinator

Production login is challenge-driven rather than assuming exactly two calls.

The MTS ID client will:

1. Generate and persist a stable, encoded device descriptor compatible with MTS Login SDK 5.8.0.
2. Start authorization for client `VDome` with the registered callback at `/user-service/api/auth/callback` on the VDome gateway.
3. Maintain the MTS ID cookie/session jar server-side.
4. Follow the WebSSO callback state machine and expose only the current required challenge to the local UI.
5. Accept phone, SMS OTP, captcha, password, account selection, or consent only when requested by MTS ID.
6. Follow the successful callback URL server-side and parse:
   - `webSsoAccessToken`;
   - Keycloak `accessToken` and `refreshToken`;
   - `idToken`.
7. Persist the resulting session and refresh it through the correct VDome refresh endpoints.

Phone numbers, OTP values, passwords, and captcha answers are never logged. OTP/password values exist only for the duration of a request and are never persisted.

Only one refresh operation may run at a time. Concurrent requests wait for that result. A request receiving `401` may be retried once after refresh; mutating actions are retried only when the upstream response proves they were not accepted.

### 7.3 Domain services

Domain services provide stable types for:

- account/profile/flat/resident;
- device/access point/intercom/lock/camera;
- live/archive media and events;
- active and historical calls;
- company/contact/news;
- issue/message/attachment;
- chat message;
- service/invoice/payment link;
- meter/reading;
- access request;
- marketplace product/basket/order;
- favorite and notification preference;
- support conversation/message/attachment/operator state/appeal;
- legal document, provider compatibility, and safe official-account link.

Account- or company-specific missing modules are represented as unavailable capabilities, not as global application failures.

### 7.4 Media proxy

The browser never receives reusable upstream authorization tokens.

The media layer will:

- fetch authorized snapshots;
- obtain live/archive URLs from VDome;
- proxy HLS manifests and segments when authorization or CORS requires it;
- rewrite nested playlists and relative segment URLs to same-origin local routes;
- bound response sizes and timeouts;
- stream bytes without buffering full video files;
- support archive clip download with an explicit time range.

Short-lived upstream media URLs may be returned directly only when they contain no reusable credential and are safe for browser exposure.

### 7.5 Calls and realtime events

The call subsystem has a stable local interface independent of the final upstream transport details.

- A background task connects to `webrtc.domofon.mts.ru/ws`, authenticates using the active VDome session/device identity, and reconnects with bounded exponential backoff.
- Incoming call sessions are normalized and published to the internal broadcast bus.
- REST actions use the discovered VDome call-action routes and carry `X-Call-Session-ID` when required.
- The browser connects to a local signaling WebSocket for offer/answer/ICE exchange and to SSE for call metadata/notifications.
- Direct browser WebRTC is preferred. If the live account reveals a mobile SIP/RTP leg that cannot be negotiated by a browser, the same component provides a server-side SIP/RTP-to-WebRTC bridge without changing the public API or UI.
- Opening a lock during a call uses the active call session ID.

Automatic opening is modeled as an explicitly armed policy, never as a hidden side effect:

- `off` (default), `always`, `next_call`, or `until` with a 30-minute preset;
- scoped to an owned flat and selected access point/call source;
- arming or changing the policy requires a short-lived one-use mutation intent and a prominent warning;
- `next_call` is consumed before dispatch, `until` expires by server UTC, and at most one open attempt is made for a validated call session;
- ambiguous results are not retried, every attempt is audited as a redacted event, and logout disarms the policy;
- persistent `always` mode is disabled unless `VDOME_ALLOW_PERSISTENT_AUTO_OPEN=true` is set deliberately.

The server continues detecting incoming calls and invoking webhooks when no browser tab is open.

### 7.6 Realtime bus and webhooks

Normalized events include:

- incoming/ringing/answered/ended call;
- door-open result;
- new camera/intercom event;
- issue/chat/order update when polling or an upstream signal makes it available;
- support message/operator/appeal update and automatic-open policy/result;
- connection/authentication state changes.

Events are exposed through SSE. Incoming calls also appear through the call WebSocket. Webhooks are signed with an optional shared secret, have short timeouts, bounded retries, and never block the upstream listener.

### 7.7 MTS support and documents

Support is implemented as a clean-room typed client, not by embedding the proprietary Android SDK:

- support configuration, bidirectional history, and uploads use `online-chat.mts.ru` with product/channel `vdome` and compatibility version `3.23.0`;
- live messaging uses `wss://online-chat.mts.ru/ws/chat`; the first typed frame carries the server-held `idToken`, generated event ID, bounded application/device data, selected official company referrer, optional appeal ID, settings, and enabled features;
- appeals use `support.mts.ru/my-appeals/api`, and the fixed FAQ is `https://m.support.mts.ru/mpvdome`;
- provider frames map to safe text, file/image, operator/typing, delivery/read, reaction, bot-action, and appeal domain events; unknown frames are retained only as redacted diagnostics;
- send, upload, reaction, and close operations use typed one-use intents and are not retried after an ambiguous dispatch.

The fixed terms document is `https://resident-rest.vdome.mts.ru/docs/terms-of-use-resident.pdf`; privacy is `https://domofon.mts.ru/privacy-policy.html`. Official PDFs and authenticated invoice/issue/support attachments may be exposed only through fixed routes or opaque, owned, expiring document leases with strict HTTPS/host/DNS/redirect, type, size, and timeout checks. There is no caller-supplied URL proxy.

## 8. Local API design

The public API is versioned under `/api/v1`. Convenient aliases matching `domru-z81` are provided where they are unambiguous.

Principal groups:

```text
/api/v1/auth/*
/api/v1/session
/api/v1/flats/*
/api/v1/dashboard
/api/v1/devices/*
/api/v1/cameras/*
/api/v1/events/*
/api/v1/calls/*
/api/v1/residents/*
/api/v1/news/*
/api/v1/companies/*
/api/v1/issues/*
/api/v1/chat/*
/api/v1/support/*
/api/v1/billing/*
/api/v1/meters/*
/api/v1/access-requests/*
/api/v1/marketplace/*
/api/v1/notifications/*
/api/v1/config
/api/v1/about
/api/v1/legal/*
/api/v1/documents/*
/api/v1/realtime                 # SSE
/api/v1/calls/signaling          # WebSocket
/api/v1/calls/auto-open
```

Compatibility aliases include `/api/login`, `/api/confirm`, `/api/session`, `/api/logout`, `/api/places`, `/api/snapshot/:camera_id`, `/api/stream/:camera_id`, `/api/archive/:camera_id`, and `/api/open-door`.

Authorization is represented uniformly:

```json
{
  "status": "challenge_required",
  "flowId": "opaque-local-id",
  "challenge": {
    "type": "sms_otp",
    "fields": [{ "name": "code", "secret": true }],
    "expiresInSeconds": 120
  }
}
```

The same shape can represent phone, captcha, password, consent, or account-selection challenges without changing routes.

Errors use a stable envelope:

```json
{
  "code": "upstream_rate_limited",
  "message": "Too many authorization attempts",
  "retryable": true,
  "requestId": "local-request-id",
  "retryAfterSeconds": 60
}
```

Raw upstream bodies, tokens, stack traces, and personal data are never returned by default.

## 9. Web UI

The frontend is a responsive TypeScript/React application built by Vite and embedded in the Rust artifact. Node is a build-time dependency only.

Top-level navigation:

1. Home/favorites.
2. Intercoms and access points.
3. Cameras and event timeline.
4. Calls/history.
5. Access requests.
6. Home services: news, issues, chat, invoices, meters.
7. Marketplace.
8. Profile/residents/settings.
9. Support/FAQ and appeals.
10. About/legal/provider compatibility.

Important interaction rules:

- A flat selector scopes every applicable screen.
- Unsupported services are hidden or shown as unavailable with the upstream reason.
- Manual door/open-barrier controls always require a deliberate confirmation interaction. Automatic opening runs only after the user deliberately arms a narrowly scoped policy through the same one-use confirmation mechanism.
- Orders, issue creation/cancellation, readings submission, and payment transitions show a final summary before mutation.
- Payment opens only an official upstream payment URL in a new secure browser context.
- Camera playback supports live/archive state, timeline navigation, event markers, and clip download.
- Incoming calls display an in-app banner and Browser Notification, then open the call surface with answer/reject/open controls.
- Support sends, uploads, reactions, and appeal closing show a final summary; official micro-app/survey links open only through a strict HTTPS allowlist without local token injection.
- MTS ID account pages open on the official `id.mts.ru` host in a separate secure context; the service does not proxy them or expose its server-side ID token/cookies.
- Account links are limited to `/account`, `/account/personal`, `/account/safety`, `/account/links`, and an explicitly marked official `/account/payments` handoff when offered; card/account-payment pages are never proxied or reproduced locally.
- Official PDFs and attachments use a bounded allowlisted document proxy or direct safe download and the browser's native PDF viewer.
- Accessibility includes keyboard navigation, focus management, labeled controls, reduced-motion support, and sufficient contrast.

The design is original and does not reproduce VDome branding assets.

## 10. Persistence and local security

This is a single-account personal service by default.

Persisted data:

- encrypted session/token bundle;
- stable device identity;
- selected flat and non-sensitive UI preferences;
- automatic-open policy, selected device favorites, theme, and dismissed UI notices;
- webhook and polling configuration;
- optional bounded cache metadata.

Secrets are encrypted with AES-GCM. `VDOME_SECRET_KEY` may supply the key; otherwise a random local key is generated in the data directory. Secret and key files are created with owner-only permissions and written atomically.

Security defaults:

- bind to `127.0.0.1` unless explicitly configured otherwise;
- same-origin UI/API;
- HttpOnly, SameSite session cookie;
- CSRF protection for cookie-authenticated mutations;
- body and upload size limits;
- validated outbound webhook URL and redirect URL handling;
- no arbitrary upstream proxy route;
- no general-purpose HTML/document proxy and no token injection into external account/support pages;
- no secrets in frontend storage;
- redaction in tracing and panic/error output.

When exposed beyond loopback, a local access password or trusted reverse-proxy authentication is mandatory.

## 11. Error handling and resilience

Provider errors are classified as:

- authentication challenge required;
- expired/revoked session;
- permission or unavailable service;
- validation failure;
- rate limit;
- transient network/upstream outage;
- media/signaling failure;
- upstream schema incompatibility;
- local storage/configuration failure.

Rules:

- retry idempotent reads with capped exponential backoff and jitter;
- honor `Retry-After`;
- do not blindly retry door, auto-open, call, support, payment, reading, issue, access-request, or order mutations;
- reconnect realtime transports with state notifications;
- preserve the last known read-only view during temporary upstream outages;
- invalidate only the affected capability when an optional service is absent;
- surface actionable Russian messages in the UI while retaining structured technical context in redacted logs.

## 12. Testing strategy

### Automated tests

- Unit tests for URL building, headers, token redaction, refresh serialization, DTO mapping, playlist rewriting, and call state transitions.
- Fixture tests based on fully redacted observed response shapes.
- Mock upstream contract tests for every provider module, including support WebSocket/HTTP, bootstrap configuration, and error/schema-variation cases.
- Axum API tests for status codes, validation, CSRF, and secret non-disclosure.
- Frontend component tests for all loading/empty/error/challenge states.
- Browser E2E tests against a deterministic mock provider for the complete user flows.
- Docker build and startup smoke test.

### Live-account verification

After the mock suite passes:

1. Perform MTS ID login and real SMS verification with the user.
2. Run read-only smoke tests for provider configuration, profile, flats, devices, cameras, events, archive, services, news, issues, billing, meters, passes, marketplace, support history, and appeals.
3. Verify incoming-call observation without accepting or controlling it.
4. Ask for explicit confirmation immediately before each real door-open, automatic-open arm/test, call accept/reject/hangup, issue/order/access-request creation, meter submission, support send/upload/reaction/appeal close, or payment-related mutation.

Live responses are never committed unless manually reduced to redacted synthetic fixtures.

## 13. Deployment

The release artifact is one Rust service with embedded frontend assets.

Configuration:

```text
VDOME_BIND=127.0.0.1:3000
VDOME_DATA_DIR=./data
VDOME_SECRET_KEY=<optional external key>
WEBHOOK_URL=<optional>
WEBHOOK_SECRET=<optional>
VDOME_ALLOW_PERSISTENT_AUTO_OPEN=false
RUST_LOG=info
TZ=Europe/Moscow
```

Docker uses a Node frontend build stage, Rust build stage, and minimal runtime image. `./data` is mounted as a persistent volume. Health endpoints distinguish process liveness from upstream/auth readiness.

## 14. Main risks and mitigations

### WebSSO protocol changes

Keep MTS Login state handling isolated behind an `AuthProvider` interface and cover every observed challenge with fixtures. Report unsupported challenge types explicitly instead of guessing.

### Account-specific feature sets

Drive the UI from gateway feature flags and returned flat services. Absence of marketplace, meters, billing, or intercom service is a valid state.

### Media URL diversity

Use a host allowlist derived from trusted VDome responses, strict redirects, bounded proxying, and separate adapters for HLS/RTSP/Ivideon-style sources.

### Call protocol uncertainty

Keep signaling, call actions, and media transport separate. Validate read-only signaling first; support either direct browser WebRTC or an internal SIP/RTP bridge without changing the local contract.

### Support protocol and externally rendered content

Keep MTS support HTTP/WebSocket transport behind a typed provider with fixtures and strict size/host policies. Render normalized message primitives locally; hand dynamic official micro-apps, polls, and surveys off to allowlisted HTTPS pages without forwarding local cookies or tokens.

### Upstream schema drift

Use tolerant additive parsing, captured redacted fixtures, explicit versioned domain models, and contract smoke tests.

## 15. Acceptance criteria

The project is complete when:

1. A fresh instance can complete current MTS ID/SMS authorization and automatically refresh/recover its session.
2. Every included feature area has a typed Rust provider implementation, local API routes, and a usable Web UI state.
3. Cameras support snapshot, live playback, archive timeline/events, and clip download for the live account where the service is provisioned.
4. The service detects real incoming calls, delivers SSE/browser/webhook notifications, and supports browser audio/video plus all applicable call actions.
5. Manual door and barrier controls work only through explicit user actions; automatic opening works only through a deliberately armed, scoped, expiring/audited policy, and all outcomes remain explicit.
6. Optional resident services degrade independently according to account capabilities.
7. Tokens and personal secrets remain server-side, encrypted at rest, and absent from logs/API errors.
8. Mock/unit/API/UI suites pass, Docker starts successfully, and read-only live smoke tests pass.
9. Mutating live tests are performed only after immediate user confirmation.
10. Support/FAQ/appeals, legal/about/theme/share, official MTS ID handoff, and provider compatibility diagnostics have typed safe browser flows without leaking credentials.
11. README documents setup, security, API usage, Docker, supported features, limitations, and the unofficial/non-affiliated status.
