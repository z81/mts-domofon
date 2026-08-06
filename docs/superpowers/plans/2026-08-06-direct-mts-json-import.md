# Direct MTS ID JSON Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the local MTS Login relay with a direct official MTS ID redirect and a secure local import of the JSON displayed by the VDome callback.

**Architecture:** The existing auth-start endpoint remains the source of a fresh, stateful MTS authorization URL. React validates that URL as an exact `https://login.mts.ru/amserver/oauth2/authorize` target and navigates the current tab directly to it. A new bounded Rust endpoint parses the complete callback JSON into the existing `TokenBundle`, atomically persists it through `AuthCoordinator`, and returns only `AuthProgress::Authenticated`; relay routes and relay-only code are removed.

**Tech Stack:** Rust 2024, Axum, Reqwest, Secrecy/Zeroize, Tokio, React 19, TypeScript, Vitest, Testing Library.

## Global Constraints

- Do not proxy any `/amserver/**` or `/wsso/authenticate` MTS Login traffic.
- Do not launch or manage another browser process and do not require an extension.
- Accept at most 64 KiB for the import request and never log or echo submitted JSON or token values.
- Keep the import field hidden inside the authentication dialog until explicitly selected; never render it at the bottom of the app.
- Parse and persist atomically, discard `idToken` after schema validation, and clear the frontend field before awaiting the response.
- Preserve all unrelated dirty-worktree changes and never add a co-author trailer.

---

### Task 1: Secure backend JSON import and relay removal

**Files:**
- Modify: `src/provider/mts/sso.rs`
- Modify: `src/auth/coordinator.rs`
- Modify: `src/api/auth.rs`
- Modify: `src/app.rs`
- Modify: `Cargo.toml`
- Modify: `tests/auth_coordinator.rs`
- Create: `tests/mts_sso_import.rs`
- Delete: `tests/mts_sso_relay.rs`

**Interfaces:**
- Consumes: callback JSON with `webSsoAccessToken`, `idToken`, and `keycloakTokenPair.accessToken` / `refreshToken`.
- Produces: `SsoTransport::import_callback_response(SecretString) -> Result<TokenBundle, AppError>`.
- Produces: `AuthCoordinator::import_web_sso_response(SecretString) -> Result<AuthProgress, AppError>`.
- Produces: `POST /api/v1/auth/web-sso/import` with `{ "response": "<complete JSON>" }` returning `AuthProgress`.

- [ ] **Step 1: Write failing transport/coordinator/API tests**

Add tests proving that a valid complete response installs all three retained secrets, discards the ID token, returns an authenticated session, and persists encrypted material. Use synthetic markers only:

```rust
const IMPORT_JSON: &str = r#"{
  "webSsoAccessToken":"manual-web-sso",
  "idToken":"manual-id-token-discarded",
  "keycloakTokenPair":{
    "accessToken":"manual-access",
    "refreshToken":"manual-refresh",
    "expiresIn":"600"
  }
}"#;

let progress = coordinator
    .import_web_sso_response(SecretString::from(IMPORT_JSON))
    .await
    .unwrap();
assert!(matches!(progress, AuthProgress::Authenticated { .. }));
```

Add rejection tests for malformed JSON, missing/empty required fields, unknown outer request fields, and an import request over 64 KiB. Assert the previous session remains byte-for-byte unchanged after every failure. Assert API errors and captured tracing output contain none of the synthetic secret markers.

- [ ] **Step 2: Run the focused Rust tests and confirm RED**

Run:

```bash
cargo test --all-features --test mts_sso_import --test auth_coordinator
```

Expected: compilation/test failures because the import interface and route do not exist.

- [ ] **Step 3: Implement callback-response parsing and atomic installation**

Extend the transport trait and production transport with:

```rust
fn import_callback_response(
    &self,
    raw_response: SecretString,
) -> Result<TokenBundle, AppError>;
```

Parse `raw_response.expose_secret()` directly into the existing private `CallbackResponse`, call its existing `into_tokens()`, and map every syntax/schema failure to the generic callback schema error. In `AuthCoordinator`, hold the operation mutex, parse first, persist with the existing `install_web_sso_session`, clear stale browser flows only after persistence succeeds, and return `AuthProgress::Authenticated`.

- [ ] **Step 4: Add the bounded local API endpoint**

Add a dedicated import extractor/handler with a 64 KiB body limit and strict outer shape:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebSsoImportRequest {
    response: SecretString,
}
```

Register only `POST /api/v1/auth/web-sso/import`. Use zeroizing request bytes, reject non-JSON content types, reject empty/oversized response strings, and return only `Json<AuthProgress>`. Do not attach raw parse errors as causes.

- [ ] **Step 5: Remove relay runtime and relay-only dependencies**

Remove `/api/v1/auth/flows/{flow_id}/web`, `/amserver`, `/amserver/{*path}`, and `/wsso/authenticate` routes; remove relay request/response/cookie types, flow bindings, response rewriting, relay CSP, relay header helpers, and coordinator relay methods. Remove relay-specific query redaction entries only when no longer used elsewhere. Remove `httpdate` if it has no remaining consumers. Delete `tests/mts_sso_relay.rs` and replace its relevant security assertions with import tests.

- [ ] **Step 6: Run focused backend tests and confirm GREEN**

Run:

```bash
cargo test --all-features --test mts_sso_import --test auth_coordinator --test mts_sso_flow --test mts_sso_request
cargo fmt --all -- --check
```

Expected: all focused tests and formatting pass.

- [ ] **Step 7: Commit backend work**

```bash
git add Cargo.toml src/api/auth.rs src/app.rs src/auth/coordinator.rs src/provider/mts/sso.rs tests/auth_coordinator.rs tests/mts_sso_import.rs tests/mts_sso_relay.rs
git commit -m "feat: import direct MTS callback JSON"
```

---

### Task 2: Direct redirect and hidden JSON import UI

**Files:**
- Modify: `frontend/src/features/auth/authApi.ts`
- Modify: `frontend/src/features/auth/authApi.test.ts`
- Modify: `frontend/src/features/auth/AuthPage.tsx`
- Modify: `frontend/src/features/auth/AuthPage.test.tsx`
- Modify: `frontend/src/app/App.test.tsx`

**Interfaces:**
- Consumes: `POST /api/v1/auth/start` returning `AuthProgress::browser_redirect`.
- Consumes: `POST /api/v1/auth/web-sso/import` with `{ response: string }`.
- Produces: `AuthApi.importWebSsoResponse(response: string, signal?: AbortSignal): Promise<AuthProgress>`.
- Produces: exact-provider validation before `navigate(authorization_url)`.

- [ ] **Step 1: Write failing API and component tests**

Extend the fake API with `importWebSsoResponse`. Assert the production boundary sends exactly:

```ts
expect(fetcher).toHaveBeenCalledWith('/api/v1/auth/web-sso/import', {
  body: JSON.stringify({ response: callbackJson }),
  credentials: 'same-origin',
  headers: { 'Content-Type': 'application/json' },
  method: 'POST',
  signal: undefined,
})
```

Component tests must assert:

- a valid `https://login.mts.ru/amserver/oauth2/authorize?...` URL is passed unchanged to `navigate`;
- non-HTTPS, wrong-host, credential-bearing, non-default-port, and wrong-path URLs are rejected without navigation;
- the JSON field is absent initially and appears only after clicking **Вставить ответ MTS**;
- submit is disabled for blank input, duplicate clicks make one request, the field is cleared before the promise settles, success calls `onAuthenticated`, and errors never render a submitted secret marker.

- [ ] **Step 2: Run frontend tests and confirm RED**

Run:

```bash
cd frontend && npm test -- --run src/features/auth/authApi.test.ts src/features/auth/AuthPage.test.tsx
```

Expected: failures because direct navigation and import UI/API are absent.

- [ ] **Step 3: Implement the production API boundary**

Add to `AuthApi` and `createProductionAuthApi`:

```ts
importWebSsoResponse: async (response, signal) =>
  readLocalResponse<AuthProgress>(await fetcher('/api/v1/auth/web-sso/import', {
    body: JSON.stringify({ response }),
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })),
```

The synthetic API returns a synthetic authenticated session without inspecting the pasted value.

- [ ] **Step 4: Implement exact direct navigation**

Replace local relay navigation with an exact validator:

```ts
function directMtsLoginUrl(raw: string): string {
  const url = new URL(raw)
  if (url.protocol !== 'https:' || url.hostname !== 'login.mts.ru' ||
      (url.port !== '' && url.port !== '443') ||
      url.username !== '' || url.password !== '' ||
      url.pathname !== '/amserver/oauth2/authorize') {
    throw new Error('invalid MTS authorization URL')
  }
  return url.href
}
```

For production `browser_redirect`, call `navigate(directMtsLoginUrl(progress.authorization_url))`. Keep synthetic challenge flows in-page.

- [ ] **Step 5: Implement the hidden import view**

Add a secondary **Вставить ответ MTS** action on the choice view. Its view contains one `textarea` labelled **JSON от MTS**, `autoComplete="off"`, `spellCheck={false}`, explanatory copy telling the user to paste into localhost rather than chat, and buttons **Подключить** / **Назад**. Before awaiting `api.importWebSsoResponse`, copy the state into a local variable and immediately `setSecret('')`; guard duplicate submission with a ref. On success reuse `applyProgress`; on failure show only `Не удалось импортировать ответ MTS`.

- [ ] **Step 6: Run focused frontend tests and confirm GREEN**

Run:

```bash
cd frontend && npm test -- --run src/features/auth/authApi.test.ts src/features/auth/AuthPage.test.tsx src/app/App.test.tsx
npm run build
```

Expected: focused tests and production build pass.

- [ ] **Step 7: Commit frontend work**

```bash
git add frontend/src/features/auth/authApi.ts frontend/src/features/auth/authApi.test.ts frontend/src/features/auth/AuthPage.tsx frontend/src/features/auth/AuthPage.test.tsx frontend/src/app/App.test.tsx
git commit -m "feat: add direct MTS JSON login"
```

---

### Task 3: Integration verification and live handoff

**Files:**
- Verify all modified files.
- Do not submit credentials, phone numbers, OTPs, callback JSON, or real tokens during automated checks.

**Interfaces:**
- Consumes: completed backend and frontend tasks.
- Produces: a running local application at `http://127.0.0.1:3000` with direct MTS navigation and local JSON import.

- [ ] **Step 1: Run the complete verification matrix**

```bash
cargo test --all-features
cargo clippy --all-features --lib --tests -- -D warnings
cargo fmt --all -- --check
cd frontend && npm test && npm run build
cd .. && git diff --check
```

Expected: every command exits zero; generated dependency warnings may appear but the main crate has no clippy warnings.

- [ ] **Step 2: Perform safe route smoke tests**

Verify `/health/live`, anonymous `/api/v1/session`, rejection of `/amserver/NUI/`, and rejection of malformed synthetic import JSON. Start auth only far enough to assert the returned URL has the exact MTS origin/path; do not follow it or send SMS.

- [ ] **Step 3: Review secret handling and scope**

Search modified source, tests, logs, and git diff for real token prefixes or pasted user data. Confirm the JSON field is not rendered outside the auth dialog and the old relay text/routes are absent.

- [ ] **Step 4: Restart the existing development server with the fresh build**

Gracefully stop only the known `domru-mts` server session, rebuild the frontend, and start `cargo run` on `127.0.0.1:3000`. Do not launch a browser.

- [ ] **Step 5: User verification**

Tell the user to open localhost, click **Войти с MTS ID**, copy the complete JSON displayed by the VDome callback, reopen localhost, and use **Вставить ответ MTS**. Never ask the user to paste that JSON into chat.
