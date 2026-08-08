# Default Camera Stream Endpoint Design

## Goal

Add one stable local URL, `GET /api/v1/cameras/stream`, that a media player can open without sending query parameters, cookies, an `Authorization` header, or provider tokens.

The endpoint starts or reuses the first live-capable camera for the current property scope and redirects the client to the existing short-lived local HLS playlist. Provider authentication remains entirely inside the backend.

## HTTP Contract

- Method and path: `GET /api/v1/cameras/stream`.
- Request body: none.
- Request parameters: none.
- Local client authentication: none, by explicit design.
- Side effect: the GET intentionally starts or reuses a bounded local media session.
- Success: `307 Temporary Redirect` with a same-origin `Location` of `/api/v1/live/<random-uuid>/index.m3u8`.
- Cache policy: `Cache-Control: private, no-store` on the redirect; the existing playlist and segment responses retain their private, no-store policy.
- Response contains no MTS access token, device token, RTSP URL, provider host, flat ID, or camera ID.

The caller does not supply authorization, but the backend must already have a valid encrypted MTS/VDome session. If that provider session is absent or expired, the existing stable API error is returned instead of a redirect.

## Camera Selection

1. Load the ownership-checked property dashboard.
2. Use `selectedFlatId` when it identifies a linked flat.
3. Otherwise use the first flat in the dashboard order, matching the frontend's existing fallback.
4. Load that flat's ownership-checked device inventory.
5. Select the first camera, in the existing deterministic camera-ID order, whose live capability is `available`.
6. Start or reuse its bounded local live session through the existing live-source provider and `LiveSessionManager`.

The endpoint does not silently switch to another flat when the selected flat has no live-capable camera.

## Errors

- No linked flat: `404 not_found`.
- No live-capable camera in the chosen flat: `404 not_found`.
- Missing or expired provider session: preserve the existing authentication error and status.
- Provider or local media startup failure: preserve the existing redacted stable error envelope.
- Unsupported HTTP methods: use the existing method-not-allowed envelope.

Errors must not reveal property, camera, provider, token, or media-source identifiers.

## Implementation Shape

- Register the new GET route in `src/api/intercom.rs` before the parameterized camera routes for clarity.
- Extract the existing camera-specific start logic into a private helper that accepts an already selected `FlatId` and camera ID. Both `POST /api/v1/cameras/{camera_id}/live` and the new default stream handler call that helper.
- Build the redirect response from the validated `LiveSessionView::playlist_url`; do not accept or construct external redirect targets.
- Keep selection and orchestration in the API layer. Provider tokens remain injected only by `MtsClient`, and private source URLs remain `SecretString` values passed directly to the local media runner.
- Update the README endpoint table and add a minimal usage example such as `ffplay http://127.0.0.1:3000/api/v1/cameras/stream`.

## Security Boundary

This endpoint intentionally provides no local client-authentication challenge. Any local process or browser origin able to reach the loopback port can invoke it and receive the playlist redirect; that exposure is an explicit tradeoff of the stable unauthenticated URL. The loopback-only bind limits reachability but is not equivalent to authentication. The endpoint must not be documented as safe to expose through a public bind or unauthenticated reverse proxy.

The redirect's random UUID is a short-lived local bearer capability. Existing behavior limits a live session to ten minutes, reuses a session for the same camera, rejects unknown assets, bounds playlist and segment sizes, and redacts live-session identifiers from request logs.

## Verification

- A request with no headers or query parameters receives a `307` and same-origin HLS `Location` when a stored provider session and live camera are available.
- The selected flat takes precedence over the first dashboard flat.
- With no selected flat, the first dashboard flat is used.
- Non-live cameras are skipped within the chosen flat using deterministic camera order.
- No linked flat and no live-capable camera produce redacted `404` envelopes.
- The existing camera-specific live endpoint retains its current JSON contract.
- The redirect and final media responses are private and non-cacheable.
- Tests prove that provider tokens and private source URLs never enter the response or `Location`.
- `cargo test --all-features`, frontend tests, lint, and production build remain green.
- The branch remains one commit ahead of `main` through amend, with no co-author trailer.
