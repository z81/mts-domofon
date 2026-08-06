# Direct MTS ID JSON import

## Goal

Restore the simple direct MTS ID flow without a local login proxy. The browser navigates to the official MTS ID authorization URL, the registered VDome callback displays its JSON response, and the user imports that complete JSON into the local application.

## User flow

1. In the local authentication dialog, the user clicks **Open MTS ID**.
2. The frontend requests a fresh authorization URL from the Rust backend and navigates the current tab directly to that validated `https://login.mts.ru/` URL.
3. MTS performs authentication using its own page, cookies, fingerprinting, and delivery mechanisms. The registered VDome callback eventually displays JSON containing `webSsoAccessToken`, `idToken`, and `keycloakTokenPair`.
4. The user copies the complete JSON, reopens `http://127.0.0.1:3000`, chooses **Import MTS JSON**, and pastes it into a single field inside the authentication dialog.
5. The local backend validates and installs the session, after which the normal dashboard is shown.

The JSON must be pasted into the local application, never into chat.

## Backend

- Keep the existing authorization-start contract and return the provider authorization URL to the frontend.
- Add one bounded local import endpoint accepting at most 64 KiB. It parses the existing MTS callback schema and requires non-empty web SSO, Keycloak access, Keycloak refresh, and ID tokens.
- Discard the ID token after schema validation, reuse the existing encrypted session persistence path, and return only the provider-neutral session view.
- Parse and persist atomically: malformed or incomplete input must not replace an existing session.
- Never log, echo, serialize into an error, or retain the submitted JSON beyond the request. Clear the frontend field after submission.
- Remove the `/amserver/**` and `/wsso/authenticate` relay routes and relay-only cookies, response rewriting, CSP, flow binding, and tests. The application must not proxy MTS Login traffic.

## Frontend

- For `browser_redirect`, navigate directly to the returned MTS URL instead of the local `/web` relay route.
- Keep the import UI inside the authentication dialog and hidden until the user chooses **Import MTS JSON**. Do not render an import block at the bottom of the app.
- Use a single paste field with autocomplete and spellcheck disabled, a clear import action, generic validation errors, and no token previews.
- Do not launch or manage another browser process and do not require an extension.

## Verification

- Rust tests cover valid import, malformed/incomplete/oversized input, atomic persistence, secret redaction, and removal of relay routes.
- Frontend tests cover direct provider navigation, hidden-by-default import UI, successful import, input clearing, duplicate-submit prevention, and generic errors without secret content.
- Full Rust and frontend suites, formatting, clippy, production build, and a safe direct-redirect smoke test must pass. A real user login is verified only by the user because it requires credentials and may send SMS.

## Non-goals

- No browser extension, embedded/managed browser, MTS Login reverse proxy, legacy anonymous SMS API, automatic clipboard access, or token submission through chat.
