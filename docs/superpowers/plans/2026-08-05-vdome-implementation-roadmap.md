# VDome Core Intercom Rust API and Web UI Implementation Roadmap

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this roadmap phase-by-phase. Execute each linked plan task-by-task and pass its acceptance gate before starting the next phase.

**Goal:** Deliver a production-ready independent Rust/Axum client and complete Web UI for the core intercom, camera, and incoming-call functions of MTS VDome 3.0.1.

**Architecture:** One Rust service embeds a React SPA and owns encrypted WebSSO authentication, typed VDome core providers, stable flat/device/media/call services, manual one-use mutation intents, a media proxy, persistent SIP/WebRTC handling, SSE/webhooks, and a versioned local API. Four sequential phases preserve narrow boundaries and require deterministic mock proof before any live-account action.

**Tech Stack:** Rust 1.96.0 edition 2024, Axum 0.8.9, Tokio, Reqwest 0.13.4, Tower HTTP 0.7.0, Rustls, AES-GCM, Utoipa, Rust Embed, Tokio Tungstenite, RSIP, WebRTC, React 19.2.8, Vite 8.2.0, TypeScript 6.0.3, Vitest 4.1.10, Playwright 1.62.1, and Docker BuildKit.

## Global Constraints

- Repository: the current standalone checkout.
- Keep the architecture standalone and do not reuse another provider's protocol or repository-specific structure.
- Current interoperability reference: VDome distribution package, package `ru.mts.vdome.resident`, version `3.0.1` (`300001001`), SHA-256 `1a857b46a2e7aec6d9e01458a9c8e8961e2a1d96a861bd4e1bdcd1ccf368c92b`.
- Implement independently from observed contracts; do not copy proprietary APK code, UI, analytics, or assets.
- Production authentication is challenge-driven MTS ID/WebSSO for client `VDome`; never fall back to obsolete direct SMS.
- Provider DTOs never cross the service boundary; local JSON remains stable and versioned under `/api/v1`.
- All provider/media/SIP secrets remain server-side, encrypted at rest, and redacted from logs/errors/browser-visible data.
- Every door and call-control mutation uses a deliberate server summary and short-lived one-use intent; ambiguous dispatch is never retried. Call-setting/blocking writes use CSRF and expected revision.
- Real SMS login is allowed. Ask immediately before each live manual door, accept, reject, ignore, hangup, open-during-call, call-block, or call-setting mutation.
- Excluded features have no dormant provider, domain, API, task, persistence, or UI scaffold; phase 4 asserts their absence.
- Every task uses TDD, focused verification, and a small commit without any Codex/Anthropic co-author trailer.

---

## Phase Order

```text
Phase 1: Rust/Axum foundation + MTS ID authentication
   |
   v
Phase 2: Linked flats + devices/manual opening + cameras/archive/events
   |
   v
Phase 3: Persistent incoming calls + browser WebRTC + SSE/webhooks
   |
   v
Phase 4: Local security + diagnostics + packaging + CI + release
```

| Phase | Plan | Principal output | Gate |
|---|---|---|---|
| 1 | [Foundation and authentication](./2026-08-05-01-foundation-auth.md) | Runnable Axum/React shell, encrypted store, provider bootstrap, typed client, WebSSO challenges, refresh/logout | Rust auth/API tests, frontend auth tests/build, live SMS runbook |
| 2 | [Intercom and media](./2026-08-05-02-intercom-media.md) | Linked flats, core dashboard, access points, manual opening, cameras, snapshots, HLS, archive, read-only events, call settings | Provider/media/security/API/UI tests, device/media Playwright, read-only live smoke |
| 3 | [Calls and realtime](./2026-08-05-03-calls-realtime.md) | Persistent SIP registration, browser WebRTC, manual call controls, SSE, Browser Notifications, signed webhooks | Mock SIP/RTP/WebRTC, call API/UI/Playwright, passive live self-ring |
| 4 | [Security and release](./2026-08-05-04-hardening-release.md) | Local auth, OpenAPI/health/config, supervision, embedded UI, container, scope enforcement, CI/docs | Full Rust/frontend/E2E/security/container gate and controlled live acceptance |

Phases are sequential because later phases consume stable interfaces from earlier phases. Tasks within a phase may run independently only when their locked file maps and interface declarations do not overlap.

## Cross-Phase Interface Contract

| Interface/type | Owner | Consumers | Contract |
|---|---|---|---|
| `AppConfig` | phase 1 `src/config.rs` | all phases | Validated startup configuration; secret fields use secret wrappers |
| `AppError` / error envelope | phase 1 `src/error.rs` | every service/API | Stable code/message/retry/request ID; raw provider body stays private |
| `ProviderBootstrap` | phase 1 `src/provider/mts/bootstrap.rs` | auth, health/capabilities | Anonymous `/config`/`/feature`, explicit `WEB_SSO`, unknown names preserved safely |
| `MtsClient` | phase 1 `src/provider/mts/client.rs` | all core providers | Typed authorized requests, one serialized refresh/retry rule, request IDs |
| `AuthCoordinator` / `TokenSource` | phase 1 | `MtsClient`, call engine | Encrypted session and serialized refresh; unused ID token is not persisted |
| `LocalSessionId` | phase 1 | every API/mutation/media/call | HttpOnly local identity; never the upstream VDome/call session ID |
| `FlatId` / `CapabilitySet` | phase 2 | phases 2–4 | Owned linked-flat scope and optional core capability availability |
| `DeviceInventory` | phase 2 | media, calls, API/UI | Deterministically merged access points, locks, cameras, and core metadata |
| `MutationService` / `MutationIntent` | phase 2 | phases 2–3 | Typed, local-session-bound, 60-second, one-use payload hash; consume before dispatch |
| `MediaPolicy` / `MediaLeaseId` | phase 2 | camera/archive API/UI | Opaque owned leases and strict HTTPS/host/DNS/redirect/size/time policy |
| `Page<T>` | phase 2 | events/call history | Opaque cursor, optional total, stable list envelope |
| `RealtimeBus` / `DomainEvent` | phase 3 | calls, SSE, webhooks, phase 4 health | Bounded monotonic redacted events; slow consumers cannot block producers |
| `CallRevision` | phase 3 | manual call intents/API/UI | Optimistic race protection for one observed call state |
| `PublicConfig` | phase 4 | connection settings UI | Redacted revisioned local settings; webhook secret is write-only |

No phase may add a generic raw-provider route, arbitrary URL/media proxy, stringly typed mutation executor, automatic-action scheduler, or client-supplied ownership/session field.

## Complete Included Feature Matrix

| Core capability | Server/local API | Web UI | Automated proof | Phase/task |
|---|---|---|---|---|
| Provider `/config` and `/feature` bootstrap | Internal typed bootstrap; normalized capabilities | Login compatibility and health states | Anonymous allowlist/schema/unknown-flag tests | 1 / task 4; 4 / task 2 |
| MTS ID phone/SMS/challenge login | `/api/v1/auth/*`, `/api/v1/session` | Challenge-driven login forms | SSO request/transport/coordinator/API/component/E2E tests | 1 / tasks 5–10 |
| Token refresh, restore, logout | Auth coordinator and encrypted store | Session/bootstrap/logout states | Concurrency, expiry, corruption, restart, redaction tests | 1 / tasks 3, 7–10 |
| Linked flat list and selection | `/api/v1/flats`, `/api/v1/flats/selected` | Global flat selector | Ownership, persistence, partial-capability, UI/E2E tests | 2 / tasks 1–3, 8–10 |
| Core dashboard/capabilities | `/api/v1/dashboard`, `/api/v1/capabilities` | Device/camera/call connection and recent indicators | Aggregation/API/component/partial-outage tests | 2 / tasks 1–3, 8–10; 4 / task 2 |
| Call settings and blocking | `/api/v1/call-settings` | Revisioned per-flat controls | Provider/revision/CSRF/API/UI/E2E tests | 2 / tasks 2, 8–10; 4 / task 1 |
| Intercoms, doors, gates, barriers, elevators, locks | `/api/v1/devices` | Filtered device cards and multi-lock selector | DTO merge, unknown kinds, ownership, UI/E2E tests | 2 / tasks 1–3, 8–10 |
| Manual opening | `/api/v1/access/intents/*`; `/api/open-door` alias | Server summary and explicit execute dialog | Expiry/race/ownership/ambiguous/API/UI/E2E tests | 2 / tasks 7–10 |
| Camera snapshots | `/api/v1/cameras/:id/snapshot` | Camera grid/preview | MIME/size/SSRF/ownership/API/UI tests | 2 / tasks 3–4, 8–10 |
| Live camera video | Live-session and opaque media-lease routes | HLS player | Playlist/nested URL/range/backpressure/playback tests | 2 / tasks 5, 8–10 |
| Archive playback and clip download | Archive-session/download routes | UTC range/timeline/download controls | Range/DST/bounds/streaming/API/UI/E2E tests | 2 / tasks 6, 8–10 |
| Read-only camera/intercom events | `/api/v1/events` | Paginated timeline and archive markers | Pagination/filter/ownership/API/UI/E2E tests | 2 / tasks 6, 8–10 |
| Persistent incoming audio/video calls | SIP engine and local signaling WebSocket | Incoming banner and call surface | SIP registration/INVITE/SDP/RTP/WebRTC tests | 3 / tasks 1–5, 8–10 |
| Accept/reject/ignore/hangup | Typed call intents and engine commands | Explicit manual controls/summaries | Reducer/race/SIP/action/API/UI/E2E tests | 3 / tasks 6, 8–10 |
| Manual open during call | Access intent bound to call ID/revision/session | Call-surface open confirmation | Session-header/ownership/race/E2E tests | 3 / tasks 2, 6, 8–10 |
| Microphone/camera/received-audio/output controls | Browser peer/media state | Feature-detected local controls | RTP bridge/component/Playwright tests | 3 / tasks 5, 9–10 |
| Active/recent and optional call/guest event history | `/api/v1/calls`, `/api/v1/calls/:id` | Calls page/history | Reducer retention plus phase-2 event mapping/API/UI tests | 2 / task 6; 3 / tasks 1, 8–10 |
| In-app realtime updates | `/api/v1/realtime` SSE | Banners/status refresh | Resume/replay/lag/heartbeat/reconnect tests | 3 / tasks 1, 7–10 |
| Browser Notifications | Local browser adapter | Explicit permission and incoming-call notification | Component/Playwright adapter tests | 3 / tasks 9–10; 4 / task 2 |
| Signed outbound webhooks | Bounded dispatcher and revisioned config | Connection settings/status | Signature/host/retry/no-tab/config tests | 3 / tasks 7, 10; 4 / tasks 2–3 |
| Health/config/OpenAPI | `/health/*`, `/api/v1/config`, `/api/v1/capabilities`, `/api/v1/openapi.json` | Redacted connection settings/health | Schema/snapshot/no-PII/revision/UI tests | 4 / task 2 |
| Loopback/remote local security | Middleware and local session API | Local login/session behavior | Argon2/CSRF/origin/proxy/header tests | 4 / task 1 |
| Embedded UI and Docker | Single Rust artifact/container | Same core SPA | Asset/cache/CSP/container smoke tests | 4 / task 4 |

## Explicit Exclusion Matrix

Every row is enforced by phase-4 route, source-path, OpenAPI, navigation, task-registry, and mock-provider absence tests.

| Excluded APK/product area | Decision |
|---|---|
| Support, FAQ, conversations, files, reactions, appeals | No provider/client/API/UI/background transport |
| Automatic opening: always, next call, timed | No type, config, persistence, scheduler, incoming-call hook, API, or UI |
| Favorites/main-settings favorite selection | No provider request, local fallback, API, or control |
| Terms/privacy/end-user documents/PDF viewer | No user-facing document route or proxy; repository engineering docs remain |
| Theme selection, share, rating, Play Store, About | One fixed accessible visual theme; no related metadata/actions/screens |
| Resident profile, MTS Profile SDK, residents, flat onboarding/add/remove | Only linked-flat listing/selection remains |
| News, contacts, feedback, issues/files, management chat | Entirely absent |
| Services, invoices, payment, meters/readings, notification preferences | Entirely absent; Browser Notification permission remains local |
| Guest/vehicle passes and access requests | Entirely absent; direct manual door control remains |
| Marketplace/catalog/basket/orders | Entirely absent |
| Event deletion | Event timeline is read-only |
| Wear, widgets, NFC/HCE, BLE/local hardware, FCM, Android intents/update | Replaced only where core server/browser behavior is specified |
| TNPS, analytics, crash reporting, ESMART/debug menu, copied assets | Local redacted health/tracing and original UI only |
| Bank-card handling | Never accepted, proxied, or stored |

## Mutation Confirmation Matrix

| Mutation | Intent/revision binding | Immediate live approval |
|---|---|---|
| Door/gate/barrier/elevator/lock open | local session, flat, access point, lock, action | Yes |
| Accept/reject/ignore/hangup | local session, call ID, call revision, action | Yes |
| Open during call | access fields plus call ID/revision and server-side session reference | Yes |
| Call block/setting update | local session, flat/apartment, expected provider revision, redacted before/after hash | Yes |

Mock-provider actions need no out-of-band approval, though the same in-product confirmation UI is exercised. Browser-local media/notification controls and selected-flat changes are not upstream mutations.

## Repository-Wide Definition of Done

- All four linked phase gates pass from a clean checkout.
- `cargo fmt`, clippy with warnings denied, all Rust tests, frontend typecheck/unit/build, full Playwright, dependency/license scans, container build, and container smoke pass.
- Every included feature row has a stable local API, complete UI states, provider/unit/API/component tests, and a deterministic end-to-end mock journey.
- Every exclusion row is proven absent from routes, OpenAPI, source modules, registered tasks, persistence, provider requests, and UI navigation.
- Authentication errors, capability absence, rate limits, schema additions, partial outages, session expiry, reconnect, races, and ambiguous manual mutations have explicit tested behavior.
- No secret or real identifying data appears in logs, errors, API/SSE/WebSocket output, HTML/bundles, OpenAPI, screenshots, test artifacts, metrics, or container layers.
- Media, provider, WebSSO, and webhook destinations pass strict scheme/host/redirect/DNS/address policy; no arbitrary proxy exists.
- The service remains useful with no open browser: token refresh, SIP registration, incoming-call observation, event polling, and webhook delivery continue.
- README, API/configuration/deployment/security/troubleshooting/live-acceptance docs, license, changelog, and compatibility notes describe only the core product and explicit exclusions.
- Final live acceptance starts read-only and asks immediately before each real mutation listed above.
- `git status --short` is clean after the final documented commit, and no commit contains a forbidden co-author trailer.
