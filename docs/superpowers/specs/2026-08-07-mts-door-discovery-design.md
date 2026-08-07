# MTS door discovery and opening controls

## Goal

Make the existing **Открыть** controls appear for every door and vehicle gate available to the selected MTS flat. Keep the current two-step confirmation flow and the existing provider mutation contract; verification must never open a real door.

## Recovered provider contract

The current MTS APK first reads `GET /vdome-smart-facade/devices/api/v1/flats/{id}/available`, normalizes the returned equipment aliases, and then requests access points separately with `category=accessPoint` and `category=drivePoint`. Calling the access-point endpoint without a category can return an empty page even when doors exist. Each category is independently paginated with `page`, `size`, and `sort=name,asc`.

## Design

- When `DeviceProvider::access_points` is called without an explicit category, the MTS provider discovers the flat's available aliases and retains only the canonical door categories `accessPoint` and `drivePoint`.
- It fetches every retained category, validates every bounded Spring page using the existing checks, and merges access points deterministically by provider panel id.
- Duplicate panels and locks across categories are combined with the existing conflict detection. Unsupported aliases are ignored and never exposed.
- An explicit category call remains a single-category query for compatibility and focused tests.
- If alias discovery succeeds but contains no door categories, the provider returns an empty list. Category reads degrade independently: successful categories are retained, while an error is returned only when every advertised door category fails. Alias discovery failures remain provider errors; the service may still degrade independently to cameras as it does today.
- `ensure_lock` uses the same discovered inventory as the read path, so a confirmation intent cannot target a door that was absent from the selected flat.

## User flow and safety

The frontend already renders an **Открыть** button per available access point, provides a lock selector when necessary, creates a short-lived one-use intent, and requires a second explicit **Подтвердить открытие** action. This flow stays unchanged. A network-ambiguous mutation is not retried automatically.

## Verification

- Contract tests cover exact alias path encoding, canonical alias filtering, both MTS door categories, pagination, deterministic merge/deduplication, empty availability, and failure propagation.
- Service/API tests continue proving ownership and one-use confirmation semantics.
- Frontend tests confirm the existing buttons render from non-empty inventory and still require confirmation.
- Run focused provider and door tests, all Rust tests, formatting, clippy, and all frontend checks.
- Perform only authenticated read-only smoke checks against MTS: verify aggregate access-point and lock counts and that the local UI receives them. Do not execute a door intent.

## Non-goals

No camera-derived synthetic doors, automatic opening, confirmation bypass, mutation retries, provider identifiers in logs, or changes to unrelated application features.
