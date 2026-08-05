# VDome Rust API and Web UI Implementation Roadmap

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this roadmap phase-by-phase. Each linked phase plan must be executed task-by-task and its acceptance gate must pass before the next phase begins.

**Goal:** Deliver a production-ready independent Rust/Axum client and complete Web UI for every MTS VDome 3.0.1 APK capability that meaningfully transfers to a server/browser product.

**Architecture:** One Rust service embeds a React SPA and owns encrypted authentication, typed VDome providers, stable domain services, media/SIP/WebRTC bridges, mutation confirmation intents, SSE/webhooks, and a versioned local API. Five implementation phases preserve narrow provider/domain/API/UI boundaries while allowing each risky subsystem to pass a deterministic mock gate before live-account verification.

**Tech Stack:** Rust 1.96.0 edition 2024, Axum 0.8.9, Tokio, Reqwest 0.13.4, Tower HTTP 0.7.0, Rustls, AES-GCM, Utoipa, Rust Embed, Tokio Tungstenite, RSIP, WebRTC, React 19.2.8, Vite 8.2.0, TypeScript 6.0.3, Vitest 4.1.10, Playwright 1.62.1, Docker BuildKit.

## Global Constraints

- Repository: `/Users/alex/dev/domru-mts`.
- Structural reference only: `/Users/alex/dev/domru-z81` Rust `origin/main`; do not reuse its Dom.ru protocol.
- Current interoperability reference: `/Users/alex/Downloads/VDome_3.0.1_APKPure.xapk`, package `ru.mts.vdome.resident`, version `3.0.1` (`300001001`), SHA-256 `1a857b46a2e7aec6d9e01458a9c8e8961e2a1d96a861bd4e1bdcd1ccf368c92b`.
- Implement independently from observed network contracts; do not copy proprietary APK source, UI, analytics, or assets.
- Production authentication is challenge-driven MTS ID/WebSSO for client `VDome`; direct legacy SMS endpoints are not the default.
- Provider DTOs never cross the service boundary; local JSON models remain stable and versioned under `/api/v1`.
- All secrets remain server-side, encrypted at rest and redacted from logs/errors/browser-visible data.
- Safety-sensitive, destructive, user-content, financial, access, automation-policy, support, and order mutations use a deliberate UI summary and short-lived one-use server intent; ambiguous upstream outcomes are never blindly retried. Low-risk favorite/call-setting changes still require CSRF, an expected revision, and an explicit control.
- Real SMS login is allowed. Ask immediately before every live write, including favorites/call settings and each door/auto-open/call/issue/management-chat/support/feedback/profile/resident/order/pass/meter/notification/offline-payment/payment mutation.
- Every task follows TDD, includes focused verification, and ends in a small commit without any Codex/Anthropic `Co-Authored-By` trailer.

---

## Phase Order and Dependencies

```text
Phase 1: Foundation + MTS ID authentication
   |
   v
Phase 2: Flats + intercoms + cameras + archive/events + lock intents
   |
   v
Phase 3: SIP calls + WebRTC bridge + guarded auto-open + SSE + webhooks
   |
   v
Phase 4: Residents + news/issues/management chat + MTS support + billing/meters/preferences
   |
   v
Phase 5: Access requests + marketplace + hardening + release
```

Phases are sequential because each later phase consumes stable services from the earlier phases. Tasks within one phase may only be parallelized when the linked plan's file map and interface dependencies show that they do not share state.

| Phase | Plan | Principal output | Gate |
|---|---|---|---|
| 1 | [Foundation and authentication](./2026-08-05-01-foundation-auth.md) | Runnable Axum/React service, encrypted store, typed `MtsClient`, anonymous provider bootstrap, WebSSO challenge flow, refresh/logout | Rust bootstrap/auth/API tests + frontend auth tests/build + live SMS runbook |
| 2 | [Intercom and media](./2026-08-05-02-intercom-media.md) | Flats/dashboard, devices/locks, cameras, snapshots, HLS, archive, events, confirmed open | Rust media/security/contracts + frontend unit/build + Playwright device/media flow |
| 3 | [Calls and realtime](./2026-08-05-03-calls-realtime.md) | Persistent SIP registration, browser WebRTC, call controls, guarded auto-open, SSE, Browser Notifications, webhooks | Mock SIP/RTP/WebRTC/auto-open + call API/UI/Playwright + passive live ringing |
| 4 | [Resident services](./2026-08-05-04-resident-services.md) | Profile/residents, news/contacts, MTS support/appeals, issues/files, management chat, invoices/pay links, meters, preferences | Provider/API/WebSocket/upload/payment tests + complete home-services/support UI/E2E + read-only live smoke |
| 5 | [Access, marketplace, and release](./2026-08-05-05-access-marketplace-release.md) | PASS/DRIVE requests, catalog/basket/orders, settings/about/legal, diagnostics, security, OpenAPI, Docker/CI/docs | Full Rust/frontend/E2E/security/container gate + controlled live acceptance |

## Cross-Phase Interface Contract

The following names and ownership rules are fixed so phase plans compose without circular dependencies:

| Interface/type | Owner | Consumers | Contract |
|---|---|---|---|
| `AppConfig` | phase 1 `src/config.rs` | all phases | Validated startup-only configuration; secrets use secret wrappers |
| `AppError` / error envelope | phase 1 `src/error.rs` | all APIs/services/providers | Stable code/message/retry metadata; raw provider body is private |
| `MtsClient` | phase 1 `src/provider/mts/client.rs` | all provider modules | Authorized typed request construction, one refresh/retry rule, request IDs |
| `ProviderBootstrap` | phase 1 `src/provider/mts/bootstrap.rs` | auth, capability diagnostics | Anonymous `/config`/`/feature`, explicit `WEB_SSO`, known flags plus preserved unknown names |
| `AuthCoordinator` / `TokenSource` | phase 1 | `MtsClient`, call engine | Serialized refresh and encrypted active session; tokens are never cloneable strings in APIs |
| `LocalSessionId` | phase 1 | all APIs/mutations/media/calls | HttpOnly local session identity; not the VDome token/session ID |
| `FlatId` / `CapabilitySet` | phase 2 | phases 2–5 | Provider-neutral flat scope and optional-module availability |
| `MutationService` / `MutationIntent` | phase 2 | phases 2–5 | One-use, 60-second, local-session-bound, typed payload hash; consume before dispatch |
| `MediaPolicy` / media lease | phase 2 | cameras, news, marketplace, calls where applicable | HTTPS/DNS/host validation and opaque bounded leases; no arbitrary proxy |
| `RealtimeBus` / `DomainEvent` | phase 3 | phases 3–5 | Bounded monotonic redacted events; slow subscribers cannot block producers |
| `AutoOpenPolicy` | phase 3 `src/calls/auto_open.rs` | call engine/API/UI | Off by default; scoped/expiring/one-session deduplicated; armed only through a one-use intent |
| `Page<T>` | phase 2 | phases 2–5 | Opaque cursors, optional total, and stable list envelope |
| `Money` | phase 4 | phases 4–5 | Exact decimal/currency representation |
| `SupportService` | phase 4 | support API/UI/realtime | Server-held ID token, typed safe content/actions, bounded HTTP/WebSocket transport; no raw proxy |
| `BasketRevision` | phase 5 | marketplace API/UI | Optimistic concurrency; prices and totals are server/provider-derived |

No phase may add a generic raw-provider request route, arbitrary media URL route, stringly typed mutation executor, or client-supplied price/authorization field.

## Complete APK Feature Coverage Matrix

| APK capability | Server/local API | Web UI | Automated proof | Phase/task |
|---|---|---|---|---|
| MTS ID phone/SMS/challenge login | `/api/v1/auth/*`, `/api/v1/session` | Challenge-driven auth forms | WebSSO transport/coordinator/API/component/E2E tests | 1 / tasks 5–10 |
| Provider `/config` and `/feature` bootstrap | Internal typed bootstrap + normalized compatibility API | Login compatibility and diagnostics states | Anonymous allowlist/schema/unknown-flag tests | 1 / task 4; 5 / task 8 |
| Token refresh, logout, session restore | Auth coordinator and encrypted store | Session/bootstrap/logout states | Concurrency, expiry, restart, redaction tests | 1 / tasks 3–4, 7–10 |
| Profile editing | `/api/v1/profile` + mutation intents | Profile view/edit/summary | Provider, API, component/E2E tests | 4 / tasks 2, 11–13 |
| Official MTS ID profile/security/links | Fixed allowlisted metadata only | Clearly marked external `id.mts.ru` handoff | Host/no-token/new-context tests | 4 / tasks 2, 12–13; 5 / tasks 6, 11 |
| Flats, add/remove flat, onboarding/address search | `/api/v1/flats/*` | Flat selector and onboarding wizard | Address/company/flat contract and UI flow tests | 4 / tasks 2, 11–13 |
| Residents/additional users | `/api/v1/residents/*` | Residents panel/invite/remove | Registration/security-user contracts and UI flows | 4 / tasks 3, 11–13 |
| Dashboard, flat services, capability availability | `/api/v1/dashboard`, `/api/v1/capabilities` | Home cards, unsupported reasons | Partial-service aggregation/API/UI tests | 2 / tasks 1–3, 8–10; 5 / task 8 |
| Device favorites/main settings | Dashboard provider plus revisioned per-flat fallback | Select/remove camera/intercom/elevator favorites | Provider, isolation, badge, component/E2E tests | 2 / tasks 1–2, 8–10 |
| Call settings | Dashboard provider/local config | Per-flat call settings controls | Provider/API/UI tests | 2 / tasks 2, 8–10 |
| Intercoms, doors, barriers, gates, elevators, access points | `/api/v1/devices` | Filtered device cards/multi-lock UI | DTO merge, unknown category, capability, E2E tests | 2 / tasks 1–3, 8–10 |
| Door/barrier/elevator opening | Access intent issue/execute routes; `/api/open-door` alias | Explicit one-use confirmation dialog | Intent races, ambiguous outcome, API/UI/E2E tests | 2 / tasks 7–10 |
| Cameras and snapshots | Camera/snapshot routes and safe proxy | Camera grid/preview | Ownership, MIME/size/SSRF, UI/E2E tests | 2 / tasks 3–4, 8–10 |
| Live camera video | Live session/media lease routes | HLS player | Manifest/segment/redirect/host and playback flow tests | 2 / tasks 5, 8–10 |
| Archive playback and clip download | Archive session/download routes | Timeline range controls/download | Range/time/DST/streaming and UI/E2E tests | 2 / tasks 6, 8–10 |
| Camera/intercom events | `/api/v1/events` | Paginated event timeline/markers | Provider pagination/filter and UI tests | 2 / tasks 6, 8–10 |
| Delete camera/intercom events | Event deletion intent routes | Timeline selection/final confirmation | Body-bearing DELETE, intent, ambiguous-outcome, UI/E2E tests | 2 / tasks 6–10 |
| Incoming audio/video calls | SIP engine + local signaling WebSocket | Incoming banner and call surface | SIP registration/INVITE/SDP/RTP/WebRTC E2E | 3 / tasks 1–5, 8–10 |
| Accept/reject/ignore/hangup | Typed call intents and engine commands | Confirmed call controls | State/race/SIP/action/API/UI/E2E tests | 3 / task 6, 8–10 |
| Microphone, received-audio, camera, and speaker controls | Browser signaling/media state | Mute/video/output-device controls with feature detection | RTP bridge and component/Playwright tests | 3 / tasks 5, 9–10 |
| Open during a call | Access intent bound to call revision/session | Call-surface open confirmation | Session header/intent/race/E2E tests | 3 / task 6, 9–10 |
| Automatic opening | `/api/v1/calls/auto-open` plus policy intents | Off/always/next-call/30-minute settings, warning, armed indicator/disarm | Scope/expiry/consume-before-send/dedup/logout/ambiguous/mock E2E tests | 3 / tasks 1, 4, 6, 8–10 |
| Call history/state | `/api/v1/calls` | Calls page/history | Reducer retention/API/UI tests | 3 / tasks 1, 8–10 |
| In-app realtime notifications | `/api/v1/realtime` SSE | Live banners/toasts/state refresh | Resume/lag/heartbeat/reconnect tests | 3 / tasks 1, 7–10 |
| Browser Notifications | Local UI adapter | Explicit permission and incoming-call notification | Component/Playwright adapter tests | 3 / tasks 9–10 |
| Outbound webhooks | Signed bounded dispatcher | Webhook settings/status | Signing/destination/retry/no-browser tests | 3 / tasks 7, 10; 5 / task 7 |
| Individual guest access request | Access-request intent routes, type `PASS` | Guest form/summary/local result ledger | Mapping/validation/intent/API/UI/E2E tests | 5 / tasks 1, 5–6, 11–12 |
| Vehicle access request | Access-request intent routes, type `DRIVE` | Vehicle/plate form/summary | Mapping/validation/intent/API/UI/E2E tests | 5 / tasks 1, 5–6, 11–12 |
| News/publication list and details | `/api/v1/news/*` | News list/details | Pagination/sanitization/image/API/UI tests | 4 / tasks 4, 11–13 |
| Management/intercom companies | `/api/v1/companies/*` | Contacts page | Company scope/missing capability/API/UI tests | 4 / tasks 4, 11–13 |
| Emergency/phone contacts | Company/contact domain routes | Accessible call/contact actions | Phone/link sanitization and UI tests | 4 / tasks 4, 11–13 |
| Intercom feedback | Feedback mutation intents | Feedback form/final summary | Validation/non-retry/API/UI/E2E tests | 4 / tasks 4, 11–13 |
| MTS support FAQ and live conversation | `/api/v1/support/config|faq|conversations|messages|files` | FAQ/chat/history/operator/typing/files/reactions/bot actions | HTTP/WebSocket/content/upload/API/UI/E2E tests | 4 / tasks 5, 11–13 |
| MTS support appeals | `/api/v1/support/appeals/*` + close intent | Appeal list/details/close summary | Ownership/operation/intent/API/UI/E2E tests | 4 / tasks 5, 11–13 |
| Issue categories/list/details/status | `/api/v1/issues/*` | Issues list/details | Smart-facade contracts/API/UI tests | 4 / tasks 6, 11–13 |
| Issue creation/supplement/edit/cancel | Typed issue mutation intents | Editors and final summary dialogs | Intent/race/ambiguous/API/UI/E2E tests | 4 / tasks 6, 11–13 |
| Issue messages/photos/files | Issue message/upload routes | Conversation, previews, upload controls | Streaming upload/MIME/limits/cleanup tests | 4 / tasks 6, 11–13 |
| Management chat/send/read | `/api/v1/chat/*` | Chat page/unread state | Pagination/send/read/API/UI/E2E tests | 4 / tasks 7, 11–13 |
| Services/invoices/archive/details | `/api/v1/billing/*` | Billing/service views | Decimal/status/ownership/API/UI tests | 4 / tasks 8, 11–13 |
| Payment link/session | Safe payment intent/redirect metadata | Official new-window transition | Host/redirect/card-data/intent tests | 4 / tasks 8, 11–13 |
| Offline payment status | Typed mutation intent | Final summary/status refresh | Contract/ambiguous/API/UI tests | 4 / tasks 8, 11–13 |
| Meters/list/latest/history/statistics | `/api/v1/meters/*` | Meter cards/charts/history | Decimal/tariff/range/provider/API/UI tests | 4 / tasks 9, 11–13 |
| Submit meter readings | Meter mutation intents | String-decimal form/final summary | Precision/window/intent/ambiguous/E2E tests | 4 / tasks 9, 11–13 |
| Marketplace catalog/categories/search | `/api/v1/marketplace/catalog|search` | Catalog tree/search/results | Provider/cache/company/API/UI tests | 5 / tasks 2, 5–6 |
| Marketplace product details | `/api/v1/marketplace/products/:id` | Product page/options/images | Price/availability/safe-image tests | 5 / tasks 2, 5–6 |
| Marketplace basket | Revisioned local basket routes | Basket drawer | Concurrency/persistence/no-client-price tests | 5 / tasks 3, 5–6 |
| Marketplace checkout/order creation | One-use checkout intent routes | Revalidated summary/checkout | Price change/ambiguous/duplicate E2E tests | 5 / tasks 3–6 |
| Marketplace order history/details | `/api/v1/marketplace/orders/*` | Orders page | Pagination/status/ownership/API/UI tests | 5 / tasks 4–6 |
| Provider notification preferences | `/api/v1/notifications/preferences` | Settings controls | Unknown preservation/revision/API/UI tests | 4 / tasks 10–13 |
| Local Web UI/application settings | `/api/v1/config`, encrypted config | Profile/settings and light/dark/system theme | Persistence/security/config/E2E tests | 1 / tasks 3, 8–9; 5 / tasks 6–8 |
| About/version/update compatibility | `/api/v1/about` + normalized provider diagnostics | About and provider-status views; no false Android forced update | Exact-link/version/unknown-flag/OpenAPI/UI tests | 1 / task 4; 5 / tasks 6, 8, 11–12 |
| Terms/privacy/PDF and document viewing | Fixed legal routes plus opaque bounded document leases | Browser-native PDF/download and official privacy link | Host/DNS/MIME/size/lease/no-HTML tests | 4 / tasks 5–8, 11–13; 5 / task 8 |
| Share/rate application | Fixed current Play Store metadata | Copy/share/open official Play link | Exact package URL and safe-context UI tests | 5 / tasks 6, 8, 11 |
| Push/deep-link vote or poll target | Safe normalized internal route or allowlisted official HTTPS handoff | Route to owned screen or clearly marked external context | Scheme/host/no-token tests | 4 / tasks 5, 12–13; 5 / task 11 |

## Mobile-Only Features and Browser Equivalents

| APK-only feature | Decision | Browser/server equivalent |
|---|---|---|
| Wear OS application | Excluded | Responsive Web UI on small screens |
| Android home-screen widgets | Excluded | Home favorites and browser bookmarks/installed PWA-style shortcut if the browser offers it; no Android widget API emulation |
| NFC HCE virtual access card | Excluded | No equivalent; lock actions remain authenticated confirmed API operations |
| FCM mobile-device registration | Excluded | Persistent SIP registration for calls, SSE, Browser Notifications, and webhooks |
| Android services/intents/background UI | Excluded | Supervised server tasks and Web UI lifecycle |
| Android Bluetooth/BLE/local hardware routing | Excluded | Browser feature detection for call output; authenticated API actions for remote doors only |
| MTS analytics/crash reporting | Excluded | Local redacted tracing/health/metrics only |
| TNPS survey/telemetry SDK | Excluded | Support, feedback, issues, or allowlisted official survey link when returned by a safe provider action |
| Hidden ESMART/developer/debug menu | Excluded | Local redacted health, capability, and compatibility diagnostics |
| Native MTS Profile/All Services SDK | Excluded | Typed VDome profile plus official external MTS ID/account links; no token-bearing proxy |
| Android in-app update/rating/share APIs | Excluded | Rust release/provider compatibility status and current official Play copy/share link |
| Android push/deep-link navigation | Excluded | SSE/browser routes; allowlisted official external vote/poll handoff |
| Proprietary branding/assets | Excluded | Original accessible interface and neutral product naming |
| Native bank-card entry | Excluded | Official HTTPS VDome payment redirect only |

## Mutation Confirmation Matrix

Every row uses the same server rule: issue a typed summary intent, show it, execute once within 60 seconds, consume before dispatch, and never retry an ambiguous result.

| Mutation class | Binding fields in intent | Immediate live approval required |
|---|---|---|
| Door/barrier/elevator | local session, flat, access point, lock, action, optional call revision | Yes |
| Event deletion | local session, flat, non-empty owned event IDs | Yes |
| Call accept/reject/ignore/hangup | local session, call ID, revision, action | Yes |
| Open during call | all access fields plus call ID/revision and secret session reference | Yes |
| Arm/change automatic opening | local session, owned flat/access point/source, mode/expiry, policy revision | Yes |
| Profile/flat/resident | local session, flat/account, operation, redacted before/after hash | Yes |
| Feedback/chat | local session, flat/company, operation, message hash/length | Yes |
| MTS support action | local session, appeal/conversation/message, send/upload/reaction/close kind, content/file hash | Yes |
| Issue/file action | local session, flat, issue/category, operation, payload/file hashes | Yes |
| Payment/offline status | local session, flat, invoice, exact amount/currency, operation | Yes |
| Meter reading | local session, flat, meter/tariffs, exact decimal values | Yes |
| Notification preferences | local session, provider revision, changed setting IDs/values | Yes |
| Access request | local session, flat/company, PASS/DRIVE, time range, redacted guest/plate | Yes |
| Marketplace order | local session, company, basket revision, live product/price/total/address hash | Yes |

Mock-provider executions and local basket edits are not real external mutations and need no out-of-band user approval, though the in-product confirmation flow is still tested. A mock auto-open policy may be exercised freely; any real policy arming is an external mutation and always pauses for immediate approval.

Favorites and call/block settings are intentionally not in the intent table: they use explicit controls plus optimistic revision/CSRF protection, but the live acceptance run still pauses for immediate user approval before exercising them.

## Repository-Wide Definition of Done

- All five linked plan acceptance gates pass from a clean checkout.
- `cargo fmt`, clippy with warnings denied, all Rust tests, frontend typecheck/unit/build, full Playwright suite, dependency/license checks, container build, and container smoke pass.
- Every included feature row above has a stable local API, complete responsive UI states, provider contract tests, API tests, frontend tests, and an end-to-end mock flow.
- Every APK feature discovered in the 3.0.1 audit is either represented in the coverage matrix or explicitly mapped to an exclusion/browser equivalent; support, auto-open, MTS ID handoff, legal/about/theme/share, documents, badges, and compatibility diagnostics are not deferred as generic links.
- Login, provider errors, capability absence, rate limits, partial outages, schema additions, session expiry, reconnect, and ambiguous mutations have explicit tested behavior.
- No secret appears in logs, errors, API/SSE/WebSocket payloads, HTML/bundles, OpenAPI, screenshots, test artifacts, or container layers.
- Media/payment/webhook/WebSSO outbound destinations pass HTTPS, host, redirect, and resolved-address policies; there is no arbitrary proxy.
- The service remains useful with no open browser: token refresh, SIP registration, call observation, webhook delivery, and configured polling continue.
- `README.md`, API/config/deployment/security/privacy/troubleshooting/live-acceptance docs, license, changelog, and compatibility guide are complete.
- The final live acceptance begins read-only and requests immediate user confirmation before each actual mutation listed above.
- `git status --short` is clean after the final documented commit; commit messages contain no forbidden co-author trailer.
