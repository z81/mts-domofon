# Events, Camera Archive, Call History, and Auto-Open Design

## Goal

Replace the remaining core intercom placeholders with real MTS-backed features:

- a read-only event journal;
- recent call history derived from that journal;
- camera archive playback through the local service;
- the bounded auto-open modes present in VDome 3.0.1.

Support, FAQ, requests, documents, favorites, themes, sharing, billing, and event deletion remain out of scope.

## Supersession

Earlier project documents deliberately excluded automatic opening. The user has now explicitly requested it. This design supersedes that exclusion only for these VDome-compatible modes:

- disabled;
- open on the next incoming intercom call;
- open on incoming intercom calls for the next 30 minutes.

There is no permanent, recurring, calendar-based, location-based, or arbitrary-rule auto-open mode.

## Source Contracts

The implementation follows the contracts recovered from the user-provided VDome 3.0.1 XAPK:

- `GET /vdome-smart-facade/api/v1/event` with bounded page, size, from, and to parameters;
- `GET /vdome-smart-facade/api/v1/event/{eventId}` when event detail is required;
- `GET /domofon/api/intercom/v1/cameras/{cameraId}/archive/distribution` for recorded ranges;
- `GET /domofon/api/intercom/v1/cameras/{cameraId}/archive` with start and end parameters for an archive RTSP source;
- the existing call-scoped unlock operation for automatic opening.

Provider DTOs and media URLs stay private to the Rust process. Browser responses expose only stable local models and opaque local session identifiers.

## Event Journal and Call History

### Domain model

Add a provider-neutral, bounded event page. Each public event contains:

- an opaque event ID;
- occurrence time;
- a normalized kind: call, access, camera, or other;
- a safe Russian title and optional source label;
- normalized call outcome when applicable: accepted, missed, rejected, ignored, or unknown;
- optional duration derived from bounded, valid provider timestamps;
- optional owned camera ID and archive timestamp for navigation.

Provider aliases, payloads, phone numbers, peer details, SIP data, picture URLs, and unknown nested objects never cross the local API.

### Ownership and paging

The event service validates the requested flat against the authenticated property service, loads the flat's device inventory, and builds a bounded set of owned flat, access-point, intercom, and camera identifiers. Account-wide provider events are retained only when their context or payload can be matched to that owned set. Events without sufficient ownership evidence fail closed and are omitted.

The canonical local route is:

`GET /api/v1/events?flatId={flatId}&page={page}&size={size}&from={instant}&to={instant}&kind={kind}`

Page and size are bounded; date ranges cannot exceed 31 days. The default request returns the newest 20 events from the last seven days. Unknown provider event shapes degrade to a safe `other` event only when ownership and a valid timestamp are still proven.

Because MTS returns account-wide totals, the service does not expose or trust those totals after ownership filtering. It scans a bounded number of upstream pages in newest-first order, accumulates only owned matching events, and returns a local `hasMore` flag. A request that reaches the scan bound returns the safe partial page and `hasMore: true`; it never widens ownership rules just to fill a page.

### UI

The Events page replaces its placeholder with loading, error, empty, and paginated states. It offers All, Calls, Access, and Camera filters. The home page shows the latest three events. The Calls page uses the same API with `kind=call` to render recent call history below the active-call surface.

Selecting a camera event with an owned camera and valid timestamp opens that camera in archive mode at the event time. Call and access events navigate to the corresponding Calls or Access screen. There is no delete control.

## Camera Archive

### Provider and service

Archive discovery is allowed only after flat and camera ownership are proven. Distribution data is normalized into bounded recorded intervals. An archive start request accepts an owned camera and a start instant, clamps playback to a maximum 30-minute window, and requests the provider archive source for that interval.

The returned RTSP URL is held as a secret and passed directly to the existing ffmpeg-backed HLS session infrastructure. The archive session key includes flat, camera, start, and end so it can never reuse a live or different-time source. Archive sessions retain the existing capacity, byte, filename, lifetime, cancellation, and local-only asset controls. Audio is preserved whenever the provider source contains it.

Canonical local routes are:

- `GET /api/v1/cameras/{cameraId}/archive/distribution?flatId={flatId}`;
- `POST /api/v1/cameras/{cameraId}/archive?flatId={flatId}` with `{ "start": "..." }`;
- the existing opaque local HLS asset and stop routes for playback.

### UI

The camera screen offers Live and Archive modes only when their capabilities are available. Archive mode has a date/time selector, visible recorded intervals, Play, and Stop controls. A journal navigation supplies the selected camera and timestamp. Invalid time, unavailable recording, session-capacity exhaustion, and provider failure use explicit retryable UI states without falling back to synthetic media.

Clip export and download are not part of this increment because the request is archive viewing, not export.

## Auto-Open

### Policy

The policy is disabled by default and has exactly three modes:

- `disabled`;
- `next_call`;
- `until`, whose deadline is set by the server to at most 30 minutes from arming.

An independent `dismiss_after_open` option mirrors the APK label “Automatically dismiss the call after opening.” The option controls call presentation after the open attempt; it does not broaden which calls may trigger opening.

The current policy is shown prominently on the Calls page and as an armed banner throughout the app. Disabling is immediate. Arming `next_call` or `until` requires a summary modal and a short-lived, one-use server intent before the policy is installed.

### Trigger and race safety

The policy runs in the Rust service so it remains effective when no browser tab is open. It reacts only to a newly authenticated, owned incoming intercom call that already passed the existing FCM and SIP validation pipeline.

For `next_call`, the policy is atomically consumed before any network dispatch. For `until`, the call ID is atomically recorded before dispatch. Duplicate FCM delivery, SIP replay, reconnect, browser refresh, or server-side event replay therefore cannot open twice. An ambiguous or failed unlock is never retried automatically.

Automatic opening uses the call-associated intercom and call-session data already retained server-side by the call runtime. It does not choose all inventory locks and does not accept a browser-supplied intercom, call session, or upstream URL. The provider's call-scoped default lock behavior therefore matches the APK.

If `dismiss_after_open` is enabled, the incoming call is dismissed and suppressed from further local ringing only after the provider returns a definitive successful opening result. A failed or ambiguous attempt leaves the call available for normal manual handling. If the option is disabled, a successfully opened call also remains available for normal manual handling. The auto-open result is recorded as a safe local status without exposing identifiers or allowing a retry button.

### Persistence and clearing

Policy state is encrypted with a dedicated storage file and AAD. It is bound to the current persisted authentication generation. Logout, session replacement, corrupt state, or an expired deadline clears it. A valid unexpired policy may survive a process restart with the same authenticated session; `next_call` remains one-use across restarts.

Canonical local routes are:

- `GET /api/v1/calls/auto-open`;
- `POST /api/v1/calls/auto-open/intents`;
- `POST /api/v1/calls/auto-open/intents/{intentId}/execute`;
- `DELETE /api/v1/calls/auto-open`.

No live auto-open test is performed. Enabling a real rule is left to the user after delivery.

## Failure Isolation

Events, archive, active calls, devices, and auto-open state fail independently. A failed event request does not hide cameras or door controls. A failed archive session does not stop live viewing. A failed auto-open attempt does not retry, answer, or mutate another call. Authentication failure disables all provider-backed reads and clears armed automatic actions.

## Testing

Implementation follows red-green-refactor for each boundary:

- provider contract tests for event queries, DTO bounds, archive distribution, archive source, redirects, and secret redaction;
- ownership, filtering, pagination, timestamp, and partial-schema service tests;
- auto-open state-machine tests for disabled, next-call, 30-minute expiry, restart, logout, duplicates, races, ambiguous outcomes, and dismiss behavior;
- Axum route and method-mismatch tests for every new local endpoint;
- frontend API and component tests for journal filters, paging, call history, archive playback, navigation from an event, arming confirmation, armed status, and immediate disable;
- full Rust and frontend quality gates plus read-only live smoke for events and archive discovery.

Tests use synthetic provider data. They never open a real door, arm a real policy, answer a real call, or expose account identifiers and media URLs.
