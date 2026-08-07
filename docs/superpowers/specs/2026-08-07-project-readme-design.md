# Project README Design

## Goal

Add a root `README.md` with the requested concise, operator-focused structure and English tone while describing only capabilities that exist in `domru-mts`.

The README must let a new user understand what the application does, run it locally, authenticate, find the main API groups, and avoid publishing local credentials.

## Audience

- Users running the application locally for their own MTS/VDome intercom account.
- Contributors building and testing the Rust backend and React frontend.

## Content Structure

1. **Overview** — describe `domru-mts` as a local web client and API for MTS/VDome intercom services, followed by a compact feature list covering flats, cameras, live/archive playback, door control, calls, events, and auto-open policies.
2. **Disclaimer** — state that this is an independent, unofficial project with no affiliation or endorsement; the software is provided as-is and used at the user's risk; the user is responsible for legal, service-term, and access-control compliance; physical actions require verification; and liability is limited to the maximum extent permitted by applicable law.
3. **Requirements** — list Rust 1.96, Node.js with npm, and `ffmpeg` for live camera playback.
4. **Run locally** — show `npm ci`, the frontend production build, `cargo run`, and the default `http://127.0.0.1:3000/` URL.
5. **Configuration** — document the supported operational variables `VDOME_BIND`, `VDOME_DATA_DIR`, `VDOME_WEB_DIST_DIR`, and optional `VDOME_SECRET_KEY`. State that the server intentionally accepts loopback binds only.
6. **Authentication** — summarize the browser MTS ID flow, direct MTS callback JSON import, and explicit legacy SMS flow without including real account data.
7. **API endpoints** — provide a short table of the canonical `/api/v1` endpoint groups for sessions, dashboard/devices, camera snapshot/live/archive, door intents, calls, events, and auto-open.
8. **Local data and security** — explain that session, device, and push state is encrypted under `data/`; the directory and `.env` are ignored by Git; and neither should be committed, shared, or included in project archives.
9. **Development** — include the verified backend and frontend test, lint, build, and optional Playwright commands.

## Deliberate Omissions

- Do not add Docker or Kubernetes instructions: this repository has no supported image, Dockerfile, or deployment manifest.
- Do not add issue tracker, package registry, architecture image, or pull-request links without a configured public repository.
- Do not add a license section while the repository has no license file.
- Do not describe `domru-mts` as a transparent reverse proxy; its API is an ownership-checked local facade.

## Safety and Examples

- Use placeholders and synthetic values only.
- Never include the local `data/` contents, authorization responses, tokens, phone numbers, addresses, or account identifiers.
- Mention that `data/` contains sensitive encrypted material even though Git ignores it.
- Keep provider endpoints at their built-in defaults and avoid encouraging arbitrary upstream configuration.

## Verification

- Check every documented command and environment variable against `Cargo.toml`, `frontend/package.json`, `src/config.rs`, and `src/main.rs`.
- Run `cargo test --all-features`.
- Run `npm test`, `npm run lint`, and `npm run build` in `frontend/`.
- Scan the final README for secrets, personal addresses, and unsupported deployment claims.
- Keep the feature branch at one commit by amending the existing commit.
