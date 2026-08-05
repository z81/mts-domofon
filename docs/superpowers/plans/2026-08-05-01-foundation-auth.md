# VDome Foundation and MTS ID Authentication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce a runnable Axum application with a React shell, encrypted local state, a typed VDome HTTP client, current MTS ID/WebSSO challenge authentication, token refresh, and tested auth/session APIs; phase 5 converts the verified SPA into the final embedded single-binary artifact.

**Architecture:** The browser talks only to the local Axum API. `AuthCoordinator` owns short-lived WebSSO flows and the active VDome token bundle; `MtsSsoTransport` owns MTS cookies and exchanges WebSSO callback JSON. `MtsClient` reads tokens through a narrow `TokenSource` interface, so later provider modules cannot expose secrets to the UI.

**Tech Stack:** Rust 1.96.0 (edition 2024), Axum 0.8.9, Tokio 1, Reqwest 0.13.4 with rustls/cookies/json, Tower HTTP 0.7.0, Serde 1, AES-GCM 0.11.0, Secrecy 0.10.3, Utoipa 5.5.0, Wiremock 0.6.5; Node 26.3.1, npm 11.16.0, React 19.2.8, Vite 8.2.0, TypeScript 6.0.3, Vitest 4.1.10.

## Global Constraints

- Work only in `/Users/alex/dev/domru-mts`; use `/Users/alex/dev/domru-z81` `origin/main` as a structural reference, not as the MTS protocol implementation.
- Core phase-1 provider hosts are `https://gateway.vdome.mts.ru/`, `https://resident-rest.vdome.mts.ru`, and `https://webrtc.domofon.mts.ru/ws`; later phases add only the separately allowlisted official support hosts documented in the roadmap.
- Production authentication is MTS ID/WebSSO for client `VDome`; do not make the obsolete direct SMS endpoints the default.
- Upstream access, refresh, WebSSO, ID, device, phone, OTP, password, captcha, and media credentials must never appear in logs or frontend storage.
- Bind to `127.0.0.1:3000` by default; a non-loopback bind requires local access protection in phase 5.
- Persist secret files atomically with owner-only permissions; accept `VDOME_SECRET_KEY`, otherwise generate `data/master.key`.
- Use TDD for every behavior, keep provider DTOs separate from stable domain/API models, and commit after every task.
- Do not include `Co-Authored-By` trailers or any Codex/Anthropic co-author line in commits.
- When a task creates a Rust module, update its nearest existing `mod.rs` (and `src/lib.rs` only for a new top-level module) in the same task and include that registry file in the task commit.
- Do not copy proprietary APK code or assets; synthetic fixtures must contain no real phone, address, token, account, camera, or resident data.
- Real SMS login is allowed. Ask immediately before any later live door, call-control, issue, order, pass, meter, or payment mutation.

---

## Locked File Map

- `src/config.rs`: environment/default configuration only.
- `src/state.rs`: clonable application dependency container only.
- `src/error.rs`: stable local error codes and Axum responses only.
- `src/request_id.rs`: local request-ID middleware only.
- `src/storage.rs`: encrypted atomic secret/config persistence only.
- `src/domain/auth.rs`: provider-neutral auth/session types only.
- `src/provider/mts/device.rs`: encoded MTS device descriptor only.
- `src/provider/mts/sso.rs`: MTS Login URL/state/challenge HTTP protocol only.
- `src/provider/mts/client.rs`: authorized VDome request construction and response decoding only.
- `src/provider/mts/bootstrap.rs`: anonymous gateway `/config` and `/feature` discovery only.
- `src/auth/coordinator.rs`: flow lifecycle, token installation, refresh serialization, and logout only.
- `src/api/auth.rs`: HTTP transport for auth/session operations only.
- `src/api/health.rs`: liveness/readiness endpoints only.
- `src/web.rs`: SPA serving/fallback only; phase 1 serves a built directory and phase 5 embeds the same artifacts.
- `frontend/src/features/auth/*`: auth UI only.
- `frontend/src/app/*`: application shell/router/API bootstrap only.

### Task 1: Runnable Rust Server Foundation

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/app.rs`
- Create: `src/config.rs`
- Create: `src/state.rs`
- Create: `src/api/mod.rs`
- Create: `src/api/health.rs`
- Test: `tests/config.rs`
- Test: `tests/health.rs`

**Interfaces:**
- Produces: `config::AppConfig::from_env() -> Result<AppConfig, ConfigError>`.
- Produces: `app::build_router(AppState) -> axum::Router`.
- Produces: `GET /health/live -> 200 {"status":"ok"}`.
- Consumes: no earlier application code.

- [ ] **Step 1: Write failing configuration tests**

```rust
// tests/config.rs
use domru_mts::config::AppConfig;

#[test]
fn defaults_are_loopback_and_local_data() {
    let cfg = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    assert_eq!(cfg.bind.to_string(), "127.0.0.1:3000");
    assert_eq!(cfg.data_dir.to_string_lossy(), "./data");
    assert_eq!(cfg.web_dist_dir.to_string_lossy(), "./frontend/dist");
    assert_eq!(cfg.gateway.as_str(), "https://gateway.vdome.mts.ru/");
    assert_eq!(cfg.resident.as_str(), "https://resident-rest.vdome.mts.ru/");
    assert_eq!(cfg.call_endpoint.as_str(), "https://webrtc.domofon.mts.ru/ws");
}

#[test]
fn rejects_non_https_provider_url() {
    let err = AppConfig::from_iter([("VDOME_GATEWAY_URL", "http://example.test")]).unwrap_err();
    assert!(err.to_string().contains("https"));
}
```

- [ ] **Step 2: Run the test and verify the missing crate/API failure**

Run: `cargo test --test config`

Expected: FAIL because `Cargo.toml` and `domru_mts::config` do not exist.

- [ ] **Step 3: Create the manifest and minimal configuration implementation**

Use this package identity and dependency baseline:

```toml
[package]
name = "domru-mts"
version = "0.1.0"
edition = "2024"
rust-version = "1.96"

[lib]
name = "domru_mts"
path = "src/lib.rs"

[[bin]]
name = "domru-mts"
path = "src/main.rs"

[dependencies]
async-trait = "0.1"
axum = { version = "0.8.9", features = ["macros", "ws"] }
base64 = "0.22"
bytes = "1"
chrono = { version = "0.4.45", features = ["serde"] }
futures-util = "0.3"
http = "1"
reqwest = { version = "0.13.4", default-features = false, features = ["cookies", "gzip", "json", "multipart", "query", "rustls", "stream"] }
secrecy = { version = "0.10.3", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.7.0", features = ["catch-panic", "compression-gzip", "cors", "fs", "limit", "request-id", "sensitive-headers", "set-header", "trace"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
subtle = "2.6.1"
url = { version = "2", features = ["serde"] }
utoipa = { version = "5.5.0", features = ["axum_extras", "uuid", "url"] }
uuid = { version = "1", features = ["serde", "v4"] }

[dev-dependencies]
http-body-util = "0.1"
tempfile = "3"
wiremock = "0.6.5"
```

Implement `AppConfig` with `from_iter` for deterministic tests and `from_env` as a thin adapter. Parse URLs with `url::Url`, require HTTPS for provider URLs, normalize gateway/resident base URLs to include a trailing slash, and require the call endpoint to have the exact `/ws` path with no query/fragment.

- [ ] **Step 4: Run configuration tests**

Run: `cargo test --test config`

Expected: PASS, 2 tests.

- [ ] **Step 5: Write the failing liveness test**

```rust
// tests/health.rs
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn liveness_is_available_without_auth() {
    let app = domru_mts::app::test_router().await;
    let response = app.oneshot(Request::get("/health/live").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], br#"{"status":"ok"}"#);
}
```

- [ ] **Step 6: Implement the server state and liveness route**

Define an initially empty clonable state in `src/state.rs` and the exact health response in `src/api/health.rs`:

```rust
#[derive(Clone, Default)]
pub struct AppState;

#[derive(serde::Serialize)]
struct HealthResponse { status: &'static str }

async fn live() -> axum::Json<HealthResponse> {
    axum::Json(HealthResponse { status: "ok" })
}
```

`main.rs` loads config, initializes redacted structured tracing, binds `TcpListener`, and calls `axum::serve` with graceful Ctrl-C shutdown.

- [ ] **Step 7: Run foundation checks**

Run: `cargo fmt --check`

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Run: `cargo test --test health --test config`

Expected: all commands exit 0.

- [ ] **Step 8: Commit the foundation**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml .gitignore src tests
git commit -m "chore: bootstrap VDome Rust service"
```

### Task 2: Stable Errors, Request IDs, and Secret Redaction

**Files:**
- Create: `src/error.rs`
- Create: `src/request_id.rs`
- Modify: `src/lib.rs`
- Modify: `src/app.rs`
- Test: `tests/errors.rs`

**Interfaces:**
- Produces: `AppError { code: ErrorCode, message: String, retryable: bool, upstream_request_id: Option<String>, retry_after_seconds: Option<u64> }`.
- Produces: `ApiError { request_id: RequestId, source: AppError }` and `impl IntoResponse for ApiError` with JSON `ErrorEnvelope`; provider/domain `AppError` remains transport-neutral.
- Produces: `request_id::layer() -> RequestIdLayer`, a small Tower layer that validates/generates one canonical UUID and mirrors it into request extensions and response headers.
- Consumes: `app::build_router` from Task 1.

- [ ] **Step 1: Write failing error-envelope tests**

```rust
#[tokio::test]
async fn error_response_is_stable_and_hides_cause() {
    let err = ApiError::new(
        RequestId::parse("e7411f58-d5e9-4a43-b25d-88ddf2014713").unwrap(),
        AppError::upstream_auth("Bearer secret-token", Some("mts-42".into())),
    );
    let response = err.into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let json: Value = body_json(response).await;
    assert_eq!(json["code"], "upstream_authentication_failed");
    assert_eq!(json["retryable"], false);
    assert_eq!(json["requestId"], "e7411f58-d5e9-4a43-b25d-88ddf2014713");
    assert!(json.get("upstreamRequestId").is_none());
    assert!(!json.to_string().contains("secret-token"));
}
```

- [ ] **Step 2: Run and verify failure**

Run: `cargo test --test errors`

Expected: FAIL because `AppError` is undefined.

- [ ] **Step 3: Implement error codes and response mapping**

Use a serialized snake-case enum with explicit status mapping:

```rust
pub enum ErrorCode {
    InvalidRequest,
    AuthenticationChallengeRequired,
    UpstreamAuthenticationFailed,
    SessionExpired,
    Forbidden,
    NotFound,
    RateLimited,
    UpstreamUnavailable,
    UpstreamSchemaChanged,
    StorageFailure,
    Internal,
}
```

Store internal causes and upstream request IDs in non-serialized fields that are formatted only with `tracing` after passing through a redactor. `ApiError` serializes the local request ID, never the raw cause or upstream body. Mark `Authorization`, `Cookie`, `Set-Cookie`, `X-Device-Token`, and media query secrets as sensitive headers in `app.rs`.

- [ ] **Step 4: Add deterministic request IDs**

Accept an inbound `X-Request-Id` only when it is a valid UUID; otherwise generate `Uuid::new_v4()`. Propagate the canonical value to response `X-Request-Id`, attach it to request extensions, and use it when API handlers convert `AppError` into `ApiError`. Add tests that a valid supplied ID is retained, malformed/oversized IDs are replaced, and the JSON `requestId` matches the response header.

- [ ] **Step 5: Run error and middleware tests**

Run: `cargo test --test errors`

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: PASS and no secret value in captured JSON/log formatter tests.

- [ ] **Step 6: Commit**

```bash
git add src/error.rs src/request_id.rs src/lib.rs src/app.rs tests/errors.rs
git commit -m "feat: add safe API errors and request IDs"
```

### Task 3: Encrypted Atomic Session Storage

**Files:**
- Modify: `Cargo.toml`
- Create: `src/storage.rs`
- Create: `src/domain/mod.rs`
- Create: `src/domain/auth.rs`
- Modify: `src/lib.rs`
- Test: `tests/storage.rs`

**Interfaces:**
- Produces: `TokenBundle`, `DeviceIdentity`, and `PersistedSession` domain structs.
- Produces: `SecretStore::open(data_dir, external_key) -> Result<SecretStore, AppError>`.
- Produces: `SecretStore::{load_session, save_session, clear_session, load_or_create_device}`.
- Consumes: `AppError` from Task 2.

- [ ] **Step 1: Add crypto dependencies and write failing round-trip tests**

Add:

```toml
aes-gcm = { version = "0.11.0", features = ["zeroize"] }
rand = "0.9"
zeroize = { version = "1.9.0", features = ["derive", "serde"] }
```

Test exact invariants:

```rust
#[tokio::test]
async fn stored_session_is_encrypted_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([7_u8; 32])).await.unwrap();
    let session = PersistedSession::synthetic_for_test();
    store.save_session(&session).await.unwrap();
    let raw = tokio::fs::read(dir.path().join("session.enc")).await.unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("access-test"));
    let loaded = store.load_session().await.unwrap().unwrap();
    assert_eq!(loaded.authenticated_at, session.authenticated_at);
    assert_eq!(loaded.tokens.expose_for_test(), session.tokens.expose_for_test());
}
```

- [ ] **Step 2: Run and verify failure**

Run: `cargo test --test storage`

Expected: FAIL because storage/domain modules do not exist.

- [ ] **Step 3: Define secret domain types without `Debug` or `Display`**

```rust
#[derive(Clone)]
pub struct TokenBundle {
    pub access_token: secrecy::SecretString,
    pub refresh_token: secrecy::SecretString,
    pub web_sso_access_token: secrecy::SecretString,
    pub id_token: secrecy::SecretString,
    pub device_token: Option<secrecy::SecretString>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeviceIdentity {
    pub uuid: uuid::Uuid,
}
```

`TokenBundle` must not implement `Debug`, `Display`, `Serialize`, or `Deserialize`. Implement a private `StoredSession` made of zeroizing strings inside `storage.rs`; conversion to and from that DTO is the only place persistence exposes secret values to Serde. Implement a manual redacted `Debug` for `PersistedSession` showing only whether tokens exist and the authenticated timestamp. The `expose_for_test` helper is compiled only under `cfg(test)`.

- [ ] **Step 4: Implement AES-256-GCM storage and atomic writes**

Use a random 12-byte nonce per write and file format `VDM1 || nonce || ciphertext`. Seal `session.enc` with AAD `domru-mts/session/v1` and `device.enc` with AAD `domru-mts/device/v1`; an encrypted file cannot be replayed as the other type. Write to a same-directory uniquely named temporary file with `create_new`, `sync_all`, set mode `0600` on Unix, rename atomically, then sync the parent directory. Generate `master.key` as exactly 32 random bytes with mode `0600` when no external key is supplied.

- [ ] **Step 5: Add corruption, clear, permission, and stable-device tests**

Required assertions:

- a changed ciphertext returns `ErrorCode::StorageFailure`;
- `clear_session` removes only `session.enc` and retains the stable encrypted `device.enc`;
- `load_or_create_device` returns the same UUID on a second open;
- Unix file modes are `0600`;
- an external key never creates `master.key`.

- [ ] **Step 6: Run storage checks**

Run: `cargo test --test storage`

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/storage.rs src/domain src/lib.rs tests/storage.rs
git commit -m "feat: persist VDome sessions securely"
```

### Task 4: Provider Bootstrap and Authorized VDome HTTP Client

**Files:**
- Create: `src/provider/mod.rs`
- Create: `src/provider/mts/mod.rs`
- Create: `src/provider/mts/client.rs`
- Create: `src/provider/mts/bootstrap.rs`
- Create: `src/provider/mts/dto/mod.rs`
- Create: `src/provider/mts/dto/bootstrap.rs`
- Modify: `src/lib.rs`
- Test: `tests/mts_client.rs`
- Test: `tests/contract/provider_bootstrap.rs`
- Create: `tests/fixtures/provider_config.json`
- Create: `tests/fixtures/provider_features.json`

**Interfaces:**
- Produces: `trait TokenSource { async fn access_token(&self) -> Option<SecretString>; async fn device_token(&self) -> Option<SecretString>; async fn id_token(&self) -> Option<SecretString>; async fn refresh_after_unauthorized(&self) -> Result<(), AppError>; }`; `id_token` exists for the later isolated MTS support client and is never attached by the main VDome request builder.
- Produces: `MtsClient::authorized(Method, ProviderBase, &str) -> RequestBuilder`.
- Produces: `MtsClient::anonymous_gateway(Method, &str) -> RequestBuilder` restricted to the bootstrap allowlist.
- Produces: `MtsClient::send_json<T>()`, `send_empty()`, and `send_stream()`.
- Produces: `BootstrapProvider::fetch() -> ProviderBootstrap`, including typed known configuration/feature flags and preserved unknown names.
- Consumes: `AppConfig`, `AppError`, and secret domain types.

- [ ] **Step 1: Write failing header and redaction contract tests**

Mount Wiremock at a test base URL and assert that a request contains:

```text
Authorization: Bearer access-test
X-Device-Token: device-test
X-Device-Type: ANDROID
Request-Id: <valid UUID>
Accept: application/json
```

Also return `401` once and `200` once; assert `refresh_after_unauthorized` is called exactly once and the read is retried exactly once.

Add synthetic contracts for anonymous `GET /config` and `GET /feature` responses. Assert exact mapping of `{configName, platform, value}` and `{featureName, value}`, tolerant unknown entries, duplicate-name rejection, response bounds, and that anonymous requests cannot target any other path.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test --test mts_client --test provider_bootstrap`

Expected: FAIL because `MtsClient` is undefined.

- [ ] **Step 3: Implement base selection and safe path joining**

```rust
pub enum ProviderBase { Gateway, Resident }

impl MtsClient {
    pub fn endpoint(&self, base: ProviderBase, path: &str) -> Result<Url, AppError> {
        if !path.starts_with('/') || path.contains("..") {
            return Err(AppError::invalid_request("invalid provider path"));
        }
        self.base(base).join(path.trim_start_matches('/')).map_err(AppError::url)
    }
}
```

Reject absolute caller-supplied URLs in JSON methods. A separate allowlisted media method is added in phase 2.

Implement bootstrap discovery without authorization headers, cookies, or device tokens. Normalize known values into `ProviderBootstrap`, retain unknown names for compatibility diagnostics, and require `AUTH_TYPE=WEB_SSO` before offering login. Any missing, contradictory, or legacy auth mode produces an explicit compatibility error; it must never fall back to the obsolete direct-SMS API. Provider Android version/update values are recorded as diagnostics only because this independent Rust/Web client has its own release version.

- [ ] **Step 4: Implement response classification**

Map `401`, `403`, `404`, `409`, `422`, `429`, and `5xx` into stable error codes. Read `Request-Id`/`X-Request-Id` and `Retry-After`; bound JSON error bodies to 16 KiB; never include their raw contents in public errors.

- [ ] **Step 5: Run client tests**

Run: `cargo test --test mts_client --test provider_bootstrap`

Expected: header, one-refresh, no-second-refresh, rate-limit, body-bound, anonymous-path allowlist, `WEB_SSO`, known/unknown flag, and schema-drift tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/provider src/lib.rs tests/mts_client.rs tests/contract/provider_bootstrap.rs tests/fixtures/provider_config.json tests/fixtures/provider_features.json
git commit -m "feat: add VDome bootstrap and typed HTTP client"
```

### Task 5: MTS Device Descriptor and WebSSO Authorization Request

**Files:**
- Create: `src/provider/mts/device.rs`
- Create: `src/provider/mts/sso.rs`
- Modify: `src/provider/mts/mod.rs`
- Test: `tests/mts_sso_request.rs`

**Interfaces:**
- Produces: `MtsDeviceDescriptor::from_identity(&DeviceIdentity) -> MtsDeviceDescriptor`.
- Produces: `MtsDeviceDescriptor::encode() -> Result<String, AppError>`.
- Produces: `SsoRequest::new(device_id, state) -> SsoRequest` and `authorize_url() -> Url`.
- Consumes: persisted `DeviceIdentity` from Task 3.

- [ ] **Step 1: Write the encoded-device fixture test**

Use a fixed UUID and assert decoded Base64 JSON equals:

```json
{
  "uuid": "32f6ad2b-9f3e-4a5d-8d2c-2ac4dcdbb3e7",
  "platform": "Android",
  "vendor": "VDome Web",
  "model": "Rust Service",
  "name": "domru-mts",
  "ip": "",
  "timezone": "Europe/Moscow",
  "locale": "ru_RU",
  "useragent": "domru-mts/0.1",
  "version": "5.8.0",
  "screenSize": "",
  "systemVersion": "server"
}
```

- [ ] **Step 2: Write the authorization URL test**

Assert exact scheme/host/path and query values:

```text
https://login.mts.ru/amserver/oauth2/authorize
client_id=VDome
redirect_uri=https://gateway.vdome.mts.ru/user-service/api/auth/callback
response_type=code
scope=account address sso profile ssom openid phone
state=<generated state>
device_id=<encoded JSON>
```

- [ ] **Step 3: Run and verify failure**

Run: `cargo test --test mts_sso_request`

Expected: FAIL because device and SSO builders do not exist.

- [ ] **Step 4: Implement descriptors and request headers**

The WebSSO client must send:

```text
X-SDK-APPVersion: 3.0.1
X-SDK-Name: Login
X-SDK-ClientId: VDome
X-SDK-Platform: Android
X-SDK-Version: 5.8.0
```

Generate a fresh 256-bit URL-safe `state` per flow and compare callback state in constant time.

- [ ] **Step 5: Run request-builder tests**

Run: `cargo test --test mts_sso_request`

Expected: PASS, including malformed callback and state-mismatch cases.

- [ ] **Step 6: Commit**

```bash
git add src/provider/mts/device.rs src/provider/mts/sso.rs src/provider/mts/mod.rs tests/mts_sso_request.rs
git commit -m "feat: build current MTS ID authorization requests"
```

### Task 6: Challenge-Driven WebSSO Transport

**Files:**
- Modify: `src/domain/auth.rs`
- Modify: `src/provider/mts/sso.rs`
- Create: `tests/fixtures/sso/enter-phone.json`
- Create: `tests/fixtures/sso/verify-otp.json`
- Create: `tests/fixtures/sso/success.json`
- Create: `tests/fixtures/sso/captcha.json`
- Test: `tests/mts_sso_flow.rs`

**Interfaces:**
- Produces: `AuthChallenge`, `ChallengeInput`, `AuthFlowId`, and `AuthProgress`.
- Produces: `MtsSsoTransport::{begin, submit}` returning `AuthProgress`.
- Produces on success: `TokenBundle` parsed from `webSsoAccessToken`, `keycloakTokenPair`, and `idToken`.
- Consumes: `SsoRequest` from Task 5 and `TokenBundle` from Task 3.

- [ ] **Step 1: Define synthetic fixture bodies and failing parser tests**

The fixtures use the field names observed in the 3.0.1 login bundle:

```json
{"displayForm":"enter-phone","callback":"cb-phone","dataConfig":{}}
```

```json
{"displayForm":"verify-otp","callback":"cb-otp","dataConfig":{"resendOtpTimeout":30}}
```

```json
{"result":{"successUrl":"https://gateway.vdome.mts.ru/user-service/api/auth/callback?code=synthetic&state=fixed-state&idt_token=synthetic"}}
```

The callback response fixture is:

```json
{"webSsoAccessToken":"sso-test","keycloakTokenPair":{"accessToken":"access-test","refreshToken":"refresh-test"},"idToken":"id-test"}
```

Assert phone maps to `AuthChallenge::Phone`, OTP maps to `SmsOtp { resend_after_seconds: 30 }`, and success produces a redacted `TokenBundle`.

- [ ] **Step 2: Run and verify parser failure**

Run: `cargo test --test mts_sso_flow parser`

Expected: FAIL because `AuthProgress` and parser are undefined.

- [ ] **Step 3: Implement provider-neutral challenges**

```rust
pub enum AuthChallenge {
    Phone,
    SmsOtp { resend_after_seconds: u64 },
    Captcha { image_data_url: String },
    Password,
    AccountSelection { options: Vec<AuthAccountOption> },
    Consent { title: String, body: String },
}

pub enum ChallengeInput {
    Phone { phone: String },
    SmsOtp { code: SecretString },
    Captcha { answer: SecretString },
    Password { password: SecretString },
    Account { id: String },
    Consent { accepted: bool },
}
```

Normalize Russian phone input to `7XXXXXXXXXX`, validate exact digit count, and reject an input kind that does not match the active challenge.

- [ ] **Step 4: Implement `/wsso/authenticate` exchange**

Use the per-flow Reqwest cookie jar. Initial body contains `{"callback":null,"language":"ru"}`. Submissions use:

```json
{"callback":"opaque-from-MTS","language":"ru","formData":{"msisdn":"79990000000"}}
```

or `formData.otp`, `formData.captcha`, `formData.password`, `formData.accountId`, and `formData.accepted` according to `ChallengeInput`. Never accept an arbitrary caller-provided callback or URL.

- [ ] **Step 5: Implement success callback following**

Allow callback hosts only `gateway.vdome.mts.ru`, require path `/user-service/api/auth/callback`, validate `state`, follow once using the same cookie jar, require HTTPS, and deserialize the three token classes.

- [ ] **Step 6: Add Wiremock flow tests**

Test exact sequences:

- begin → phone → OTP → success;
- begin → captcha → phone → OTP → success;
- wrong input kind rejected locally with no HTTP request;
- state mismatch rejected;
- callback host mismatch rejected;
- unsupported `displayForm` returns `upstream_schema_changed` and preserves only the form name.

- [ ] **Step 7: Run WebSSO tests**

Run: `cargo test --test mts_sso_flow`

Expected: all synthetic flow tests pass and snapshots contain no fixture secrets.

- [ ] **Step 8: Commit**

```bash
git add src/domain/auth.rs src/provider/mts/sso.rs tests/fixtures/sso tests/mts_sso_flow.rs
git commit -m "feat: implement MTS ID challenge flow"
```

### Task 7: Auth Coordinator, Refresh Serialization, and Logout

**Files:**
- Create: `src/auth/mod.rs`
- Create: `src/auth/coordinator.rs`
- Create: `src/provider/mts/auth_api.rs`
- Modify: `src/provider/mts/mod.rs`
- Modify: `src/app.rs`
- Test: `tests/auth_coordinator.rs`

**Interfaces:**
- Produces: `AuthCoordinator::{start, submit, session, refresh, logout}`.
- Implements: `TokenSource for AuthCoordinator`.
- Consumes: `MtsSsoTransport`, `RefreshApi`, `SecretStore`, and the shared Reqwest/TLS configuration; it must not own `MtsClient`, because `MtsClient` is constructed afterward with `AuthCoordinator` as its `TokenSource`.
- Provider refresh: `PUT /user-service/api/v2/auth/refresh` with JSON schema `{ "refreshToken": string }`.
- WebSSO refresh: `PUT /user-service/api/auth/websso/refresh` using authorized client state.

- [ ] **Step 1: Write failing flow-expiry and refresh-deduplication tests**

Use a fake clock and fake transport. Assert:

- auth flows expire after 10 minutes;
- at most five flows exist and the oldest is evicted;
- 20 simultaneous `refresh()` calls produce one provider refresh;
- a failed refresh leaves the prior session available only as expired metadata;
- logout clears memory and `session.enc`.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test --test auth_coordinator`

Expected: FAIL because coordinator is undefined.

- [ ] **Step 3: Implement coordinator state**

```rust
struct AuthState {
    session: Option<PersistedSession>,
    flows: HashMap<AuthFlowId, ActiveFlow>,
}

pub struct AuthCoordinator {
    state: tokio::sync::RwLock<AuthState>,
    refresh_lock: tokio::sync::Mutex<()>,
    transport: Arc<dyn SsoTransport>,
    refresh_api: Arc<dyn RefreshApi>,
    store: SecretStore,
    clock: Arc<dyn Clock>,
}
```

Do not hold the state lock across HTTP calls. Copy only redacted flow metadata outside the lock; keep cookie jars inside `ActiveFlow` behind their own mutex.

- [ ] **Step 4: Implement refresh and startup restoration**

On startup load `session.enc`. Refresh first with the Keycloak refresh token; refresh the WebSSO token through the gateway when required; save the complete replacement atomically. Never save a partially refreshed bundle.

- [ ] **Step 5: Implement logout**

Call `/user-service/api/v2/auth/logout` with the refresh token when available, but clear local session even if the upstream logout is unavailable. Return a warning field for an upstream logout failure rather than retaining credentials.

- [ ] **Step 6: Run coordinator tests**

Run: `cargo test --test auth_coordinator`

Run: `cargo test --all-targets`

Expected: PASS, including concurrency tests under Tokio multi-thread runtime.

- [ ] **Step 7: Commit**

```bash
git add src/auth src/provider/mts/auth_api.rs src/provider/mts/mod.rs src/app.rs tests/auth_coordinator.rs
git commit -m "feat: coordinate VDome sessions and refresh"
```

### Task 8: Auth and Session HTTP API

**Files:**
- Create: `src/api/auth.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/app.rs`
- Test: `tests/auth_api.rs`

**Interfaces:**
- Produces: `POST /api/v1/auth/start`.
- Produces: `POST /api/v1/auth/flows/{flow_id}/challenge`.
- Produces: `GET /api/v1/session`, `POST /api/v1/session/refresh`, `DELETE /api/v1/session`.
- Produces aliases: `POST /api/login`, `POST /api/confirm`, `GET /api/session`, `POST /api/refresh`, `POST /api/logout`.
- Consumes: `AuthCoordinator` from Task 7 and `AppError` from Task 2.

- [ ] **Step 1: Write failing route tests**

Test these exact local payloads:

```json
POST /api/v1/auth/start
{}
```

```json
POST /api/v1/auth/flows/<id>/challenge
{"type":"phone","phone":"+7 (999) 000-00-00"}
```

```json
POST /api/v1/auth/flows/<id>/challenge
{"type":"sms_otp","code":"1234"}
```

Assert no response contains token values and invalid/expired flow IDs return `404 auth_flow_not_found`.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test --test auth_api`

Expected: FAIL because routes are missing.

- [ ] **Step 3: Implement DTOs and route handlers**

Return:

```rust
pub struct SessionView {
    pub authenticated: bool,
    pub status: SessionStatus,
    pub authenticated_at: Option<String>,
    pub expires_at: Option<String>,
}
```

The compatibility `/api/login` starts a flow when needed and submits a phone challenge. `/api/confirm` requires the server-maintained current compatibility flow and submits OTP; it never accepts an upstream callback URL.

- [ ] **Step 4: Add request-size, content-type, and secret tests**

Enforce 16 KiB JSON request bodies, require JSON on POST, mark OTP/password JSON fields sensitive in tracing, and test that a 17 KiB body returns `413`.

- [ ] **Step 5: Run API tests**

Run: `cargo test --test auth_api`

Expected: all route, alias, error, and secret non-disclosure tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/api/auth.rs src/api/mod.rs src/app.rs tests/auth_api.rs
git commit -m "feat: expose MTS ID authentication API"
```

### Task 9: React Shell and Auth UI

**Files:**
- Create: `frontend/package.json`
- Create: `frontend/package-lock.json`
- Create: `frontend/tsconfig.json`
- Create: `frontend/vite.config.ts`
- Create: `frontend/eslint.config.js`
- Create: `frontend/index.html`
- Create: `frontend/src/main.tsx`
- Create: `frontend/src/app/App.tsx`
- Create: `frontend/src/app/api.ts`
- Create: `frontend/src/app/styles.css`
- Create: `frontend/src/features/auth/AuthPage.tsx`
- Create: `frontend/src/features/auth/authApi.ts`
- Create: `frontend/src/features/auth/AuthPage.test.tsx`
- Create: `src/web.rs`
- Modify: `src/app.rs`
- Modify: `src/lib.rs`
- Test: `tests/web_assets.rs`

**Interfaces:**
- Produces: responsive SPA shell and safe `/assets/*` serving from the Vite build directory.
- Produces: `AuthPage` rendering every `AuthChallenge` variant from Task 6.
- Consumes: auth/session routes from Task 8.

- [ ] **Step 1: Initialize exact frontend dependencies**

Use npm and pin:

```json
{
  "scripts": {
    "build": "tsc -b && vite build",
    "test": "vitest run",
    "typecheck": "tsc -b --pretty false",
    "e2e": "playwright test",
    "lint": "eslint . --max-warnings=0"
  },
  "dependencies": {
    "@tanstack/react-query": "5.101.4",
    "react": "19.2.8",
    "react-dom": "19.2.8",
    "react-router-dom": "7.18.2"
  },
  "devDependencies": {
    "@eslint/js": "10.0.1",
    "@playwright/test": "1.62.1",
    "@testing-library/jest-dom": "7.0.0",
    "@testing-library/react": "16.3.2",
    "@testing-library/user-event": "14.6.3",
    "@types/node": "26.1.2",
    "@types/react": "19.2.18",
    "@types/react-dom": "19.2.4",
    "@vitejs/plugin-react": "6.0.5",
    "eslint": "10.8.0",
    "eslint-plugin-react-hooks": "7.1.1",
    "eslint-plugin-react-refresh": "0.5.3",
    "globals": "17.9.0",
    "jsdom": "30.0.1",
    "typescript": "6.0.3",
    "typescript-eslint": "8.66.0",
    "vite": "8.2.0",
    "vitest": "4.1.10"
  }
}
```

TypeScript is pinned to `6.0.3` because the current stable `typescript-eslint` `8.66.0` supports TypeScript `<6.1.0`; upgrading to TypeScript 7 requires a stable parser release that declares support. Configure flat ESLint rules for TypeScript, React Hooks, browser globals, tests, and the Vite config.

Run `npm --prefix frontend install` once to create the lockfile, then `npm --prefix frontend exec -- playwright install chromium` for the local E2E runtime; never hand-edit the lockfile.

- [ ] **Step 2: Write failing auth UI tests**

```tsx
it('moves from phone to OTP without exposing flow internals', async () => {
  server.use(startWithPhoneChallenge(), phoneReturnsOtpChallenge());
  render(<AuthPage />);
  await user.type(screen.getByLabelText('Номер телефона'), '+79990000000');
  await user.click(screen.getByRole('button', { name: 'Продолжить' }));
  expect(await screen.findByLabelText('Код из SMS')).toBeVisible();
  expect(screen.queryByText(/cb-phone|access-test/)).not.toBeInTheDocument();
});
```

Add equivalent tests for captcha image/answer, password input type, account selection, consent rejection, rate limit countdown, and session restoration.

- [ ] **Step 3: Run and verify failure**

Run: `npm --prefix frontend test`

Expected: FAIL because components are missing.

- [ ] **Step 4: Implement auth API and state rendering**

`authApi.ts` must expose exact functions:

```ts
export function startAuth(signal?: AbortSignal): Promise<AuthProgress>;
export function submitChallenge(flowId: string, input: ChallengeInput, signal?: AbortSignal): Promise<AuthProgress>;
export function getSession(signal?: AbortSignal): Promise<SessionView>;
export function logout(): Promise<void>;
```

Keep `flowId` only in React memory. Apply `autocomplete="one-time-code"` to OTP and `autocomplete="current-password"` to password. Clear secret inputs immediately after submit or error.

- [ ] **Step 5: Implement accessible application shell**

Provide skip link, focus-visible states, 44px minimum interactive targets, responsive layout from 320px width, reduced-motion media query, Russian copy, and original neutral styling without APK assets.

- [ ] **Step 6: Serve built assets safely from Rust**

Use Tower HTTP `ServeDir` over `AppConfig.web_dist_dir`, defaulting to the configured, canonicalized `frontend/dist/` directory during development. Serve hashed assets with immutable caching, `index.html` with `no-cache`, and SPA fallback only for non-`/api` GET routes. Reject traversal and symlinks outside the build root; API 404s remain JSON. Phase 5 replaces only this asset source with Rust Embed after the UI contract is complete.

- [ ] **Step 7: Run frontend and embedding tests**

Run: `npm --prefix frontend run build`

Run: `npm --prefix frontend test`

Run: `cargo test --test web_assets`

`tests/web_assets.rs` creates a temporary synthetic build directory and injects it through `AppConfig.web_dist_dir`, so backend tests do not depend on a prior Node build. Expected: synthetic index served at `/`, hashed asset served with correct MIME type, `/api/missing` returns JSON 404.

- [ ] **Step 8: Commit**

```bash
git add frontend src/web.rs src/app.rs src/lib.rs tests/web_assets.rs
git commit -m "feat: add VDome auth web UI"
```

### Task 10: Phase 1 Integration Gate and Live Login Runbook

**Files:**
- Create: `tests/auth_journey.rs`
- Create: `frontend/e2e/auth.spec.ts`
- Create: `docs/authentication.md`
- Create: `README.md`
- Modify: `.gitignore`

**Interfaces:**
- Produces: documented fresh-login, refresh, restart, and logout behavior.
- Consumes: every Task 1–9 interface.

- [ ] **Step 1: Write a complete mocked auth journey test**

Start the Axum router with Wiremock provider and a temporary encrypted store. Drive HTTP calls through phone → OTP → session, drop/recreate `AppState`, assert session restoration, return one upstream `401`, assert refresh, then logout and assert storage removal.

- [ ] **Step 2: Write a browser auth journey**

Use the deterministic mock provider to cover phone → OTP → authenticated shell, captcha/password/account/consent alternatives, resend countdown, invalid/expired flow, reload/session restore, logout, keyboard focus, mobile viewport, and absence of synthetic canary secrets in DOM, storage, console, and network response bodies.

- [ ] **Step 3: Run the journeys before documentation**

Run: `cargo test --test auth_journey -- --nocapture`

Run: `npm --prefix frontend exec -- playwright test frontend/e2e/auth.spec.ts`

Expected: PASS with no token/phone/OTP text in captured tracing output.

- [ ] **Step 4: Document local and live authentication**

Document:

- `cargo run` and `http://127.0.0.1:3000`;
- WebSSO challenge behavior;
- token file/key locations and permissions;
- `VDOME_SECRET_KEY` format as Base64-encoded 32 bytes;
- logout and recovery from revoked credentials;
- live login initiated only through the local UI, with the user entering phone and SMS code.

- [ ] **Step 5: Run the complete phase gate**

Run: `npm --prefix frontend ci`

Run: `npm --prefix frontend run lint`

Run: `npm --prefix frontend run typecheck`

Run: `npm --prefix frontend test`

Run: `npm --prefix frontend run build`

Run: `cargo fmt --check`

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Run: `cargo test --all-targets`

Run: `npm --prefix frontend exec -- playwright test frontend/e2e/auth.spec.ts`

Expected: every command exits 0.

- [ ] **Step 6: Perform the allowed live login check**

Run: `RUST_LOG=domru_mts=debug cargo run`

Open the local UI, let the user enter their phone and SMS code, then call only `GET /api/v1/session`. Confirm `authenticated=true`; inspect logs to confirm phone, OTP, cookies, and token values are absent. Do not call device or mutation endpoints in this phase.

- [ ] **Step 7: Commit**

```bash
git add tests/auth_journey.rs frontend/e2e/auth.spec.ts docs/authentication.md README.md .gitignore
git commit -m "test: verify complete VDome authentication journey"
```

## Phase 1 Completion Evidence

Record in the implementation handoff:

- the final commit hashes for Tasks 1–10;
- output summaries for Cargo, frontend, and build gates;
- whether a real MTS ID session was established;
- the exact read-only session endpoint result with personal values redacted;
- any new WebSSO challenge form name encountered, without its payload or personal data.
