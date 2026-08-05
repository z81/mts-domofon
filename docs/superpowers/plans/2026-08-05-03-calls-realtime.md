# VDome Calls, Guarded Auto-Open, Realtime, Browser Media, and Webhooks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep VDome call registration active without an open browser, normalize incoming calls, support browser audio/video, manual call actions, and explicitly armed automatic door opening, expose realtime SSE/WebSocket state, and deliver signed webhooks.

**Architecture:** A supervised call engine obtains SIP credentials from VDome, registers through the known VDome WebSocket transport, and drives a pure state machine. SIP signaling and upstream REST actions remain behind provider traits. Browser WebRTC terminates in Rust; the bridge relays negotiated RTP between the VDome leg and a local peer connection. A fail-closed auto-open policy may consume one validated incoming session only after deliberate arming. A bounded broadcast bus fans normalized events to SSE, the call WebSocket, browser notifications, and a non-blocking signed webhook worker.

**Tech Stack:** The first two phases; Tokio Tungstenite 0.30.0, RSIP 0.4.0, WebRTC 0.20.0, Rustls, HMAC/SHA-256, Tokio broadcast/watch channels, Axum WebSocket/SSE, React 19.2.8, TypeScript 6.0.3, Vitest 4.1.10, and Playwright 1.62.1.

## Global Constraints

- Finish phases 1 and 2 first; reuse their session, device inventory, confirmation-intent, redaction, request-ID, and local-session boundaries.
- Use `https://webrtc.domofon.mts.ru/ws` as the configured APK endpoint and normalize only its scheme to `wss`; any server address learned from an authenticated VDome response must pass a dedicated host policy.
- Fetch current SIP identity from `/domofon/api/security/v1/users/self`; username, secret, realm/domain, device token, session ID, SDP ICE credentials, and media keys are secret.
- Observed REST action route is `POST /domofon/api/intercom/v1/calls/{actionRouteId}/action/{action}`; the APK annotation names that path value `intercomId`, but current call code passes the incoming call session ID. Preserve it as an opaque `action_route_id` learned with the call and verify it against mock/live observation rather than relabeling it. The observed remote-ignore action is `ignore`. Opening during a call uses the actual intercom ID on the normal lock route with `X-Call-Session-ID`.
- Treat a SIP `INVITE` as the authoritative incoming-call signal. Mobile FCM payload handling is excluded; the service maintains SIP registration continuously instead.
- Support one active media call and multiple observed ringing sessions; resolve glare/races deterministically and reject stale browser revisions.
- Never record call audio/video or persist raw SDP/SIP messages. Redacted structured diagnostics may include message kind, call ID hash, codec names, timing, status codes, and byte counts.
- Do not automatically retry accept, reject, ignore, hangup, or open-during-call after an ambiguous dispatch.
- Ask the user immediately before each real call-control or open-during-call mutation during live verification. Merely observing an incoming call and registration state is read-only.
- Automatic opening defaults to off. Arming/changing it is a mutation requiring a one-use intent and immediate live approval; persistent `always` mode additionally requires `VDOME_ALLOW_PERSISTENT_AUTO_OPEN=true`. Never test it against a real call without a separately confirmed controlled self-ring, and disarm it after the test.
- Webhooks must not block the call engine, must never contain credentials/SDP/raw SIP, and must refuse private/link-local destinations unless explicitly allowed by a separate unsafe development flag.
- Do not include `Co-Authored-By` trailers or any Codex/Anthropic co-author line in commits.
- When a task creates a Rust module, update its nearest existing `mod.rs` (and `src/lib.rs` only for a new top-level module) in the same task and include that registry file in the task commit.

---

## Locked File Map

- `src/domain/call.rs`: call identity/state/action/event domain types and transition reducer only.
- `src/provider/mts/calls.rs`: SIP credential discovery and VDome REST call actions only.
- `src/calls/sip/*`: SIP message, digest, dialog, registration, transaction, and WebSocket transport only.
- `src/calls/engine.rs`: supervision, session ownership, timeouts, and domain transitions only.
- `src/calls/auto_open.rs`: scoped arming policy, expiry/consumption, deduplication, and audit decisions only.
- `src/calls/bridge.rs`: RTP/WebRTC media bridging only.
- `src/calls/signaling.rs`: browser offer/answer/ICE protocol only.
- `src/realtime/bus.rs`: normalized in-memory event publication/subscription only.
- `src/realtime/sse.rs`: SSE formatting/resume/heartbeat only.
- `src/realtime/webhook.rs`: delivery validation, signing, retry, and metrics only.
- `src/api/calls.rs`: call list/action/signaling HTTP transport only.
- `src/api/realtime.rs`: SSE HTTP transport only.
- `frontend/src/features/calls/*`: call history/current call UI only.
- `frontend/src/lib/realtime/*`: SSE, WebSocket, and Browser Notification adapters only.

### Task 1: Pure Call State Machine and Bounded Realtime Bus

**Files:**
- Create: `src/domain/call.rs`
- Create: `src/realtime/mod.rs`
- Create: `src/realtime/bus.rs`
- Modify: `src/domain/mod.rs`
- Modify: `src/lib.rs`
- Test: `tests/call_state_machine.rs`
- Test: `tests/realtime_bus.rs`

**Interfaces:**

```rust
pub struct CallRevision(pub u64);
pub enum CallPhase { Ringing, Connecting, Active, Ending, Ended(CallEndReason) }
pub enum CallCommand { ObserveInvite(IncomingInvite), Accept { revision: CallRevision }, Reject { revision: CallRevision }, Ignore { revision: CallRevision }, Hangup { revision: CallRevision }, RemoteEnded(CallEndReason), Timeout }
pub enum AutoOpenMode { Off, Always, NextCall, Until(DateTime<Utc>) }
pub fn reduce(state: &CallSession, command: CallCommand, now: DateTime<Utc>) -> Result<Transition, CallTransitionError>;

#[derive(Clone)]
pub struct RealtimeBus { /* bounded broadcast sender and monotonic event ID */ }
impl RealtimeBus { pub fn publish(&self, event: DomainEvent) -> EventId; pub fn subscribe(&self) -> EventSubscription; }
```

- [ ] **Step 1: Write table/property tests** for all legal transitions plus duplicate INVITE, stale revision, accept-vs-timeout, reject-vs-remote-BYE, multiple ringers, one-active-call enforcement, monotonic timestamps, terminal idempotency, tolerant auto-open mode serialization, and sensitive-field-free event serialization.
- [ ] **Step 2: Write bus tests** for bounded lag reporting, monotonic event IDs, independent slow subscribers, shutdown notification, and no panic when there are no receivers.
- [ ] **Step 3: Run and observe failure:** `cargo test --test call_state_machine --test realtime_bus -- --nocapture`.
- [ ] **Step 4: Implement the reducer and bus** with no network or clock calls in the reducer; inject `now`, keep terminal sessions for a bounded in-memory history, and publish a redacted snapshot after each accepted transition.
- [ ] **Step 5: Run checks and commit:** `cargo test --test call_state_machine --test realtime_bus && cargo clippy --all-targets --all-features -- -D warnings && git add src/domain src/realtime src/lib.rs tests && git commit -m "feat: add call state machine and realtime bus"`.

### Task 2: SIP Credential Discovery and Secret Types

**Files:**
- Create: `src/provider/mts/calls.rs`
- Create: `src/provider/mts/dto/calls.rs`
- Create: `src/calls/mod.rs`
- Create: `src/calls/credentials.rs`
- Create: `tests/fixtures/call_user.json`
- Create: `tests/contract/call_credentials.rs`

**Interfaces:**

```rust
pub struct SipCredentials {
    pub username: SecretString,
    pub password: SecretString,
    pub domain: String,
    pub user_id: String,
}

#[async_trait]
pub trait CallProvider: Send + Sync {
    async fn credentials(&self) -> Result<SipCredentials, AppError>;
    async fn post_remote_action(&self, action_route_id: &str, action: RemoteCallAction) -> Result<(), AppError>;
    async fn open_during_call(&self, intercom_id: &str, lock: Option<&str>, call_session_id: &SecretString) -> Result<(), AppError>;
}
```

- [ ] **Step 1: Write a Wiremock contract** for `GET /domofon/api/security/v1/users/self` that extracts the first usable `auth[].credential` (`sip`, `secret`, `domain`) while handling empty/malformed/rotated credentials, and proves Debug/Display/Serialize never disclose username/password.
- [ ] **Step 2: Write REST-action contracts** for `ignore`, fail-closed handling of unsupported action strings, `X-Call-Session-ID`, lock-number routing, and ambiguous mutation responses.
- [ ] **Step 3: Run and observe failure:** `cargo test --test call_credentials -- --nocapture`.
- [ ] **Step 4: Implement the provider** with credential zeroization on replacement and exact typed action mapping; do not add a public stringly typed call-action method.
- [ ] **Step 5: Run checks and commit:** `cargo test --test call_credentials && cargo test && git add src/provider/mts src/calls tests && git commit -m "feat: add VDome call credentials and actions"`.

### Task 3: SIP Parser, Digest Authentication, Transactions, and Dialogs

**Files:**
- Modify: `Cargo.toml`
- Create: `src/calls/sip/mod.rs`
- Create: `src/calls/sip/message.rs`
- Create: `src/calls/sip/digest.rs`
- Create: `src/calls/sip/transaction.rs`
- Create: `src/calls/sip/dialog.rs`
- Create: `tests/fixtures/sip/register_challenge.txt`
- Create: `tests/fixtures/sip/invite_audio_video.txt`
- Create: `tests/sip_protocol.rs`

**Interfaces:**

```rust
pub enum SipInbound { Response(SipResponse), Invite(SipInvite), Ack(SipRequest), Cancel(SipRequest), Bye(SipRequest), Options(SipRequest) }
pub struct SipDialogId { pub call_id: String, pub local_tag: String, pub remote_tag: String }
pub fn authorization(challenge: &DigestChallenge, request: &DigestRequest, credentials: &SipCredentials, cnonce: &[u8; 16]) -> Result<HeaderValue, SipError>;
pub fn parse_frame(bytes: &[u8], limits: SipLimits) -> Result<SipInbound, SipError>;
```

Add `rsip = "0.4.0"` and `md-5 = { version = "0.11.0", features = ["zeroize"] }`; MD5 is restricted to interoperable SIP Digest authentication and is never used for local password or signature security.

- [ ] **Step 1: Write RFC-derived unit tests** for CRLF framing, compact headers, folded-header rejection, content length, Via branch, CSeq, tags/dialog ID, 401/407 digest (`MD5`, `MD5-sess`, `qop=auth`), nonce count, retransmitted requests, CANCEL/487/ACK, BYE, OPTIONS, malformed/oversized messages, and redacted errors.
- [ ] **Step 2: Run and observe failure:** `cargo test --test sip_protocol -- --nocapture`.
- [ ] **Step 3: Implement the narrow protocol subset** on top of RSIP parsing, explicit body/header/count limits, cryptographically random branches/tags/cnonce, and deterministic transaction timers driven by an injected timer in tests.
- [ ] **Step 4: Run focused checks:** `cargo test --test sip_protocol && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit:** `git add Cargo.toml Cargo.lock src/calls/sip tests/fixtures/sip tests/sip_protocol.rs && git commit -m "feat: implement SIP call protocol core"`.

### Task 4: VDome WebSocket SIP Registration and Reconnection Supervisor

**Files:**
- Modify: `Cargo.toml`
- Create: `src/calls/sip/transport.rs`
- Create: `src/calls/sip/registration.rs`
- Create: `src/calls/engine.rs`
- Modify: `src/app.rs`
- Modify: `src/state.rs`
- Create: `tests/call_supervisor.rs`
- Create: `tests/contract/sip_websocket.rs`

**Interfaces:**

```rust
#[async_trait]
pub trait SipTransport: Send {
    async fn send(&mut self, message: SipMessage) -> Result<(), SipTransportError>;
    async fn receive(&mut self) -> Result<SipInbound, SipTransportError>;
}

impl CallEngine {
    pub async fn run(self, shutdown: CancellationToken);
    pub async fn snapshot(&self) -> CallEngineSnapshot;
}
```

Add `tokio-tungstenite = { version = "0.30.0", default-features = false, features = ["connect", "handshake", "rustls-tls-native-roots", "url"] }` and extend `tokio-util` features to `["io", "rt"]` for cancellation tokens.

- [ ] **Step 1: Build a local scripted WebSocket test server** that exercises REGISTER, 401 digest challenge, 200 expiry/contact, keepalive ping/pong, re-registration before expiry, credential rotation, INVITE while no browser is connected, clean logout, connection loss, and capped jittered reconnect.
- [ ] **Step 2: Add transport policy tests** proving only configured `wss` hosts are used, redirects are rejected, bearer/cookie headers are not added to SIP WebSocket handshakes unless the observed contract requires them, frames are bounded, and TLS validation cannot be disabled in production config.
- [ ] **Step 3: Run and observe failure:** `cargo test --test call_supervisor --test sip_websocket -- --nocapture`.
- [ ] **Step 4: Implement supervised registration** using credentials from task 2, mapping INVITE/CANCEL/BYE into the reducer and publishing connection/authentication state; refresh credentials after authorization failures and stop cleanly on logout.
- [ ] **Step 5: Run checks and commit:** `cargo test --test call_supervisor --test sip_websocket && cargo test && git add Cargo.toml Cargo.lock src/calls src/app.rs src/state.rs tests && git commit -m "feat: register for VDome SIP calls"`.

### Task 5: SDP Negotiation and RTP-to-WebRTC Bridge

**Files:**
- Modify: `Cargo.toml`
- Create: `src/calls/sdp.rs`
- Create: `src/calls/rtp.rs`
- Create: `src/calls/bridge.rs`
- Create: `tests/fixtures/sip/sdp_audio_video.txt`
- Create: `tests/sdp_negotiation.rs`
- Create: `tests/rtp_bridge.rs`

**Interfaces:**

```rust
pub struct NegotiatedMedia { pub audio: Option<CodecBinding>, pub video: Option<CodecBinding>, pub upstream: Vec<RtpEndpoint> }
pub struct BrowserPeerId(pub Uuid);
impl MediaBridge {
    pub async fn attach_browser(&self, call: &CallId, offer: RTCSessionDescription) -> Result<(BrowserPeerId, RTCSessionDescription), AppError>;
    pub async fn add_ice_candidate(&self, peer: &BrowserPeerId, candidate: RTCIceCandidateInit) -> Result<(), AppError>;
    pub async fn close_call(&self, call: &CallId);
}
```

Add `webrtc = "0.20.0"` with its default Tokio runtime before compiling the bridge.

- [ ] **Step 1: Write SDP tests** for audio-only/video, codec preference (Opus then PCMU/PCMA; H264 constrained-baseline then VP8), direction attributes, rtcp-mux, payload remapping, ICE/DTLS stripping on the plain SIP leg, unsupported codec rejection, malformed addresses, and maximum body size.
- [ ] **Step 2: Write bridge tests** using loopback UDP RTP to prove payload/sequence/timestamp/SSRC mapping, RTCP propagation, keyframe request handling, packet-size/rate limits, inactive browser behavior, microphone and received-audio mute toggles, browser disconnect without ending SIP, and cleanup on terminal calls.
- [ ] **Step 3: Run and observe failure:** `cargo test --test sdp_negotiation --test rtp_bridge -- --nocapture`.
- [ ] **Step 4: Implement browser termination with `webrtc`**, `TrackLocalStaticRTP`, bounded RTP queues, negotiated common codecs (no lossy transcoding), local-only UDP allocation, and explicit capability error when the live upstream offers no browser-compatible codec.
- [ ] **Step 5: Run checks and commit:** `cargo test --test sdp_negotiation --test rtp_bridge && cargo clippy --all-targets --all-features -- -D warnings && git add Cargo.toml Cargo.lock src/calls tests && git commit -m "feat: bridge VDome calls to browser WebRTC"`.

### Task 6: Call Commands, SIP Responses, and One-Use Action Intents

**Files:**
- Modify: `src/calls/engine.rs`
- Modify: `src/calls/sip/dialog.rs`
- Modify: `src/calls/bridge.rs`
- Create: `src/calls/auto_open.rs`
- Modify: `src/service/mutations.rs`
- Modify: `src/config.rs`
- Modify: `src/storage.rs`
- Create: `tests/call_commands.rs`
- Create: `tests/auto_open_policy.rs`
- Create: `tests/contract/call_actions.rs`

**Command behavior:**

```text
accept  -> consume intent, send SIP 200 with negotiated SDP, await ACK, activate bridge
reject  -> consume intent, send SIP 603, publish rejected terminal state
ignore  -> consume intent, POST observed remote action `ignore`, leave SIP ringing until CANCEL/timeout
hangup  -> consume intent, send BYE in-dialog, await response, terminate bridge
open    -> consume access intent bound to active call revision, add X-Call-Session-ID
arm-auto-open -> consume policy intent, validate owned flat/access point/source, persist Off/Always/NextCall/Until
incoming-auto-open -> atomically consume NextCall or validate active mode, reserve call-session hash, dispatch one open only
```

- [ ] **Step 1: Write race-heavy tests** for all commands, intent/session/revision binding, double-clicks, ACK timeout, CANCEL during accept, remote BYE during hangup, no media before ACK, ignore REST failure, ambiguous results, and automatic cleanup.
- [ ] **Step 2: Write auto-open policy tests** for default-off, owned flat/access-point/source binding, one-use arming intent, 30-minute UTC expiry, `NextCall` consumption before network dispatch, `Always` feature-gate refusal, duplicate/replayed INVITE suppression, one attempt per call session, ambiguous-result no-retry, logout disarm, restart persistence, clock skew, and redacted audit events.
- [ ] **Step 3: Run and observe failure:** `cargo test --test call_commands --test call_actions --test auto_open_policy -- --nocapture`.
- [ ] **Step 4: Implement command and auto-open orchestration** so the reducer decides legality, the engine atomically reserves an action before I/O, and outcomes distinguish confirmed, rejected, and unknown without unsafe retry. Persist only the scoped policy and hashed recent-session deduplication metadata; consume `NextCall` before dispatch and publish policy/result events without addresses or call credentials.
- [ ] **Step 5: Run focused/full checks:** `cargo test --test call_commands --test call_actions --test auto_open_policy && cargo test && cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] **Step 6: Commit:** `git add src/calls src/service/mutations.rs src/config.rs src/storage.rs tests && git commit -m "feat: add call controls and guarded auto-open"`.

### Task 7: SSE Resume, Heartbeats, and Signed Webhooks

**Files:**
- Modify: `Cargo.toml`
- Create: `src/realtime/sse.rs`
- Create: `src/realtime/webhook.rs`
- Create: `src/domain/realtime.rs`
- Modify: `src/config.rs`
- Modify: `src/storage.rs`
- Create: `tests/sse.rs`
- Create: `tests/webhooks.rs`

**Interfaces:**

```rust
pub struct WebhookEnvelope { pub id: EventId, pub occurred_at: DateTime<Utc>, pub event: PublicEvent }
// Headers: X-VDome-Event-Id, X-VDome-Timestamp, X-VDome-Signature: v1=<hex hmac-sha256(timestamp + "." + body)>
pub async fn sse_stream(bus: RealtimeBus, last_event_id: Option<EventId>) -> Sse<impl Stream<Item = Result<Event, Infallible>>>;
```

Add `hmac = { version = "0.13.0", features = ["zeroize"] }` and `sha2 = { version = "0.11.0", features = ["zeroize"] }` for webhook signatures.

- [ ] **Step 1: Write SSE tests** for event IDs/types, JSON data, 15-second heartbeat, disconnect cleanup, lag event, Last-Event-ID replay from the bounded ring, expired replay window, auth/logout events, and no secret-bearing fields.
- [ ] **Step 2: Write webhook tests** for canonical signature bytes, destination validation/DNS rebinding defense, 2-second timeout, maximum three bounded retries for retryable delivery only, `Retry-After`, queue overflow metrics, ordered delivery per destination, shutdown drain, and call-engine independence.
- [ ] **Step 3: Run and observe failure:** `cargo test --test sse --test webhooks -- --nocapture`.
- [ ] **Step 4: Implement SSE and the webhook worker**, persisting only encrypted webhook configuration and bounded non-sensitive delivery metadata.
- [ ] **Step 5: Run checks and commit:** `cargo test --test sse --test webhooks && cargo test && git add Cargo.toml Cargo.lock src/realtime src/domain/realtime.rs src/config.rs src/storage.rs tests && git commit -m "feat: add realtime events and signed webhooks"`.

### Task 8: Call and Realtime APIs plus Browser Signaling WebSocket

**Files:**
- Create: `src/calls/signaling.rs`
- Create: `src/api/calls.rs`
- Create: `src/api/realtime.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/app.rs`
- Create: `tests/api/calls.rs`
- Create: `tests/api/realtime.rs`
- Create: `tests/api/signaling.rs`

**Routes and messages:**

```text
GET  /api/v1/calls
GET  /api/v1/calls/:id
GET  /api/v1/calls/auto-open
POST /api/v1/calls/auto-open/intents
POST /api/v1/calls/auto-open/intents/:intent/execute
POST /api/v1/calls/:id/intents
POST /api/v1/calls/:id/intents/:intent/execute
GET  /api/v1/realtime
GET  /api/v1/calls/signaling        (WebSocket upgrade)

client WS: {"type":"attach","callId":"call-test-1","revision":3,"offer":{"type":"offer","sdp":"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n"}}
client WS: {"type":"ice","peerId":"55f81f2f-0886-4748-bf23-f18f318354cd","candidate":{"candidate":"candidate:1 1 UDP 2122260223 192.0.2.1 50000 typ host","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":null}}
server WS: {"type":"answer","peerId":"55f81f2f-0886-4748-bf23-f18f318354cd","answer":{"type":"answer","sdp":"v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n"}}
server WS: {"type":"call_state","call":{"id":"call-test-1","revision":4,"phase":"active"}}
server WS: {"type":"error","code":"stale_call_revision","message":"Состояние звонка изменилось"}
```

- [ ] **Step 1: Write Axum/WebSocket tests** for authentication, stable call JSON, action and auto-open policy schemas, scope/revision/intent validation, persistent-`always` feature gate, SSE resume, origin validation, frame/message limits, offer/candidate limits, one attached browser peer per local session, ping/pong, disconnect cleanup, and secret-free errors.
- [ ] **Step 2: Run and observe failure:** `cargo test --test calls --test realtime --test signaling -- --nocapture`.
- [ ] **Step 3: Implement thin APIs** and a signaling session that owns browser peer IDs, subscribes to call events, never forwards SIP/SDP credentials, and closes with explicit WebSocket codes on auth/session expiry.
- [ ] **Step 4: Run API/full checks:** `cargo test --test calls --test realtime --test signaling && cargo test`.
- [ ] **Step 5: Commit:** `git add src/calls/signaling.rs src/api src/app.rs tests/api && git commit -m "feat: expose calls and realtime API"`.

### Task 9: Calls UI, In-App Alerts, and Browser Notifications

**Files:**
- Create: `frontend/src/lib/realtime/sse.ts`
- Create: `frontend/src/lib/realtime/callSocket.ts`
- Create: `frontend/src/lib/realtime/notifications.ts`
- Create: `frontend/src/features/calls/CallsPage.tsx`
- Create: `frontend/src/features/calls/IncomingCallBanner.tsx`
- Create: `frontend/src/features/calls/CallSurface.tsx`
- Create: `frontend/src/features/calls/CallControls.tsx`
- Create: `frontend/src/features/calls/CallHistory.tsx`
- Create: `frontend/src/features/calls/AutoOpenSettings.tsx`
- Modify: `frontend/src/app/App.tsx`
- Test: `frontend/src/features/calls/CallSurface.test.tsx`
- Test: `frontend/src/lib/realtime/notifications.test.ts`

- [ ] **Step 1: Write UI tests** for incoming audio/video calls, permission denied, notification opt-in, duplicate events after reconnect, accept/reject/ignore/hangup/open summaries, stale revisions, camera preview, microphone/camera/received-audio mute state, speaker/output-device selection when `setSinkId` is supported, graceful fallback when it is not, auto-open off/always/next-call/30-minute selection, scope warning/confirmation, expiry/disarm/result state, persistent-always disabled state, lost signaling, remote hangup, keyboard-only use, focus trapping, and reduced motion.
- [ ] **Step 2: Run and observe failure:** `npm --prefix frontend test -- src/features/calls/CallSurface.test.tsx src/lib/realtime/notifications.test.ts`.
- [ ] **Step 3: Implement the UI** with an always-visible in-app banner, Browser Notification only after explicit permission, `getUserMedia` only when accepting/previewing, local track indicators, one-use action/policy intents, a persistent visible armed-policy indicator and one-click disarm, and cleanup of tracks/peer connection on every terminal path.
- [ ] **Step 4: Run frontend checks:** `npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build`.
- [ ] **Step 5: Commit:** `git add frontend && git commit -m "feat: add browser incoming-call UI"`.

### Task 10: Deterministic Call E2E and Live Observation Gate

**Files:**
- Create: `tests/mock_provider/calls.rs`
- Create: `tests/mock_sip/mod.rs`
- Create: `frontend/e2e/calls.spec.ts`
- Create: `docs/runbooks/live-calls.md`
- Modify: `README.md`

- [ ] **Step 1: Implement a deterministic SIP/WebSocket/RTP peer** that can ring audio-only/video, challenge registration, cancel, accept, exchange synthetic RTP, reject, ignore, hang up, reconnect, rotate credentials, and return ambiguous action results without touching a real account.
- [ ] **Step 2: Write Playwright coverage** for background incoming banner, Browser Notification adapter, audio/video accept, reject, ignore, hangup, open confirmation, auto-open arming/expiry/next-call consumption/deduplication/disarm with a mock door, reconnect, no-tab webhook delivery, mobile layout, keyboard controls, and browser-visible secret scans.
- [ ] **Step 3: Run the phase gate:** `cargo test && npm --prefix frontend run typecheck && npm --prefix frontend test && npm --prefix frontend run build && npm --prefix frontend exec -- playwright test frontend/e2e/calls.spec.ts`.
- [ ] **Step 4: With the user present, run only live registration and incoming-call observation.** Ask the user to ring their own intercom; verify a redacted INVITE summary, normalized ringing event, browser banner, and webhook while allowing the call to expire naturally. Do not answer/reject/ignore/hang up/open or arm auto-open until asking for and receiving immediate confirmation for that specific action. If the user separately approves an auto-open test, use one controlled self-ring with `NextCall`, observe at most one attempt, then verify the policy is off before continuing.
- [ ] **Step 5: Commit:** `git add tests/mock_provider tests/mock_sip frontend/e2e docs/runbooks/live-calls.md README.md && git commit -m "test: cover realtime call flows"`.

## Phase Acceptance Gate

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
npm --prefix frontend run typecheck
npm --prefix frontend test
npm --prefix frontend run build
npm --prefix frontend exec -- playwright test frontend/e2e/calls.spec.ts
git diff --check
```

The phase is complete only when the service stays registered with no browser tab, mock SIP/WebRTC media, all call commands, and guarded auto-open pass end to end, SSE/webhook delivery cannot stall registration, secrets never cross the local API, and live control or auto-open arming remains gated by immediate user confirmation.
