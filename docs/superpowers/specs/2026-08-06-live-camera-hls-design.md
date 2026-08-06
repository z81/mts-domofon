# Live camera playback with audio

## Goal

Add on-demand live camera playback with sound to the existing local web UI. Playback starts only after the user presses **Смотреть live**, stays inside the current page, and never exposes an MTS stream URL or media credential to the browser, logs, process arguments, or public API models.

## Recovered provider contract

The MTS camera inventory already contains the live source in `streamsToWatch` or `streams`; the APK prefers a non-blank `rtsp` entry, then the first non-blank value, and falls back from `streamsToWatch` to `streams`. The local provider will follow that order, accept only bounded `rtsp`/`rtsps` URLs, retain the selected URL as secret data, and continue exposing only the provider-neutral `live` capability in normal inventory responses.

## User flow

1. The camera card continues showing the current snapshot.
2. When live capability is available, the user presses **Смотреть live**.
3. The frontend creates one short-lived local live session for the selected flat and camera.
4. The backend proves flat and camera ownership, resolves the private MTS stream, starts the local media worker, and returns only an opaque local session identifier and same-origin playlist URL.
5. The camera card switches to an HTML video player with controls and sound. If browser autoplay policy blocks playback with sound, the player stays ready and asks the user to press play; audio remains available.
6. **Остановить**, switching the camera or flat, leaving the page, logout, or session expiry stops playback. A bounded server-side TTL cleans up abandoned sessions.

## Backend

- Extend the private camera DTO boundary to retain one bounded selected stream URL as a `SecretString`; never add it to `Camera`, `DeviceInventory`, debug output, errors, tracing, or JSON.
- Add a live-source provider method scoped by both flat and camera. `MediaService` must perform the existing linked-flat and camera ownership checks before a live session can start.
- Add canonical local endpoints to start a live session, read its HLS playlist/segments, and stop it. Session identifiers are random capability values; tracing redacts them and asset names are accepted only from a fixed allowlist.
- A `LiveSessionManager` enforces a small global concurrency bound, one active session per selected camera, a startup deadline, a fixed maximum lifetime, and idempotent cleanup. It owns every child process and temporary directory and kills/removes both on stop, failure, expiry, logout, or shutdown.
- Invoke `ffmpeg` directly without a shell. Feed the private RTSP URL through a bounded concat description on the child's stdin so it never appears in the process command line. Drain stderr without logging its content. Reject control characters and all URL schemes except `rtsp` and `rtsps`.
- Produce same-origin HLS with H.264 video and AAC audio, short segments, a bounded rolling playlist, no persistent recording, and private/no-store response headers. The browser never contacts MTS directly.
- If `ffmpeg` is missing, the source is unavailable, startup times out, or the worker exits, return a stable generic local error without provider content.

## Frontend

- Add typed `startLive` and `stopLive` calls. The API exposes only the local playlist URL and never accepts or renders arbitrary upstream URLs.
- Keep snapshot rendering as the idle and fallback state. Add **Смотреть live**, starting, playing, stopped, and retry states.
- Use native HLS when available and `hls.js` otherwise. Render a normal `<video controls playsInline>` element with audio enabled, a clear stop button, and a generic playback error.
- Abort startup and best-effort stop the live session whenever camera, flat, page, authentication state, or component lifetime changes. Never open another window or external player.

## Verification

- Provider tests cover exact stream selection order, URL bounds/schemes, secret redaction, and ownership-before-resolution.
- Session-manager tests use an injected fake process runner and cover start readiness, concurrency, timeout, stop, child exit, TTL, and directory cleanup without contacting MTS.
- API tests cover canonical routes, opaque identifiers, safe asset paths, HLS MIME/cache headers, and absence of upstream data in responses and logs.
- Frontend tests cover the button-driven lifecycle, native-HLS and `hls.js` paths, sound-capable video controls, retry, stop, and cleanup on navigation.
- Full Rust/frontend tests, formatting, clippy, lint, typecheck, production build, and a local real-camera smoke test must pass. The smoke test checks only status, MIME types, playlist readiness, and media segment presence; it does not print or save provider URLs or tokens.

## Non-goals

No archive playback, recording/download, screenshots beyond the existing snapshot, direct RTSP exposure, external player launch, WebRTC gateway, public-network binding, or more than a small number of concurrent local streams.
