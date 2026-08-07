# MTS Door Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Populate the existing door-opening controls from the current MTS `accessPoint` and `drivePoint` inventories.

**Architecture:** Keep the public `DeviceProvider` interface unchanged. Teach `MtsDeviceProvider::access_points(flat, None)` to discover canonical equipment aliases from the flat availability endpoint, fetch each advertised door category with the existing bounded pagination code, and merge successful category inventories deterministically. Explicit `Some(category)` calls remain single-category reads.

**Tech Stack:** Rust 2024, Tokio, Reqwest, Serde, Wiremock, existing Axum/React application.

## Global Constraints

- Preserve the existing one-use, expiring confirmation intent; never automatically open a door.
- Verification against the real account is read-only and reports only aggregate counts.
- Never expose tokens, provider URLs, flat ids, access-point ids, lock ids, or upstream bodies.
- Do not derive synthetic doors from camera payloads or retry a door-opening mutation.
- Do not add dependencies or change the frontend unless a failing test proves it necessary.

---

### Task 1: Category-aware MTS access-point inventory

**Files:**
- Modify: `tests/contract/intercom_provider.rs`
- Modify: `src/provider/mts/intercom.rs`

**Interfaces:**
- Consumes: `MtsClient::authorized_with_path_segment`, `MtsDeviceProvider::fold_pages`, `merge_access_point`, and `DeviceProvider::access_points(&FlatId, Option<&str>)`.
- Produces: private `available_access_categories(&FlatId) -> Result<Vec<&'static str>, AppError>` and category aggregation behind the unchanged public trait method.

- [ ] **Step 1: Write failing provider contract tests**

Add `AVAILABLE_PATH_PREFIX` and tests whose Wiremock expectations prove:

```rust
const AVAILABLE_PATH_PREFIX: &str = "/vdome-smart-facade/devices/api/v1/flats";

#[tokio::test]
async fn unfiltered_access_points_discover_and_merge_advertised_door_categories() {
    // availability returns mixed-case duplicates, camera, accessPoint, drivePoint, unknown
    // accessPoint and drivePoint each return a valid page with one row
    // call access_points(flat, None)
    // assert deterministic two-point result and exact encoded availability/category requests
}

#[tokio::test]
async fn unfiltered_access_points_return_empty_without_advertised_door_categories() {
    // availability returns only camera/elevators
    // assert empty result and no accessPoints request
}

#[tokio::test]
async fn unfiltered_access_points_keep_successful_category_when_another_fails() {
    // both categories advertised; accessPoint succeeds and drivePoint returns 503
    // assert the successful point remains
}

#[tokio::test]
async fn unfiltered_access_points_fail_when_alias_discovery_or_every_category_fails() {
    // assert stable provider errors in both cases
}
```

Change parser/schema tests that are not testing discovery from `access_points(flat, None)` to `access_points(flat, Some("accessPoint"))`, so their request surface remains focused on the access-point page under test.

- [ ] **Step 2: Run the new tests and verify RED**

Run:

```bash
cargo test --test contract intercom_provider::unfiltered_access_points -- --nocapture
```

Expected: the first test sees only the old category-less request, and the availability expectations are unmet.

- [ ] **Step 3: Implement bounded alias discovery**

In `src/provider/mts/intercom.rs`, add constants and a private method equivalent to:

```rust
const AVAILABLE_PATH: &str = "/vdome-smart-facade/devices/api/v1/flats";
const ACCESS_POINT_CATEGORY: &str = "accessPoint";
const DRIVE_POINT_CATEGORY: &str = "drivePoint";
const MAX_AVAILABLE_ALIASES: usize = 32;

async fn available_access_categories(
    &self,
    flat: &FlatId,
) -> Result<Vec<&'static str>, AppError> {
    validate_scope(&flat.0)?;
    let request = self.client.authorized_with_path_segment(
        Method::GET,
        ProviderBase::Gateway,
        AVAILABLE_PATH,
        &flat.0,
        Some("available"),
    )?;
    let aliases: Option<BoundedVec<String, MAX_AVAILABLE_ALIASES>> =
        self.client.send_json(request).await?;
    let mut available = BTreeSet::new();
    for alias in aliases.map(BoundedVec::into_vec).unwrap_or_default() {
        if alias.len() > MAX_SCOPE_BYTES {
            return Err(schema_error());
        }
        let alias = alias.trim();
        if alias.eq_ignore_ascii_case(ACCESS_POINT_CATEGORY) {
            available.insert(ACCESS_POINT_CATEGORY);
        } else if alias.eq_ignore_ascii_case(DRIVE_POINT_CATEGORY) {
            available.insert(DRIVE_POINT_CATEGORY);
        }
    }
    Ok([ACCESS_POINT_CATEGORY, DRIVE_POINT_CATEGORY]
        .into_iter()
        .filter(|category| available.contains(category))
        .collect())
}
```

Use the existing schema error for oversized alias values. Blank and unsupported bounded aliases are ignored, matching the APK normalization behavior.

- [ ] **Step 4: Implement deterministic category aggregation**

Refactor the current access-point branch into a private single-category fold returning `BTreeMap<String, AccessPointAccumulator>`. For `Some(category)`, perform only that fold. For `None`, discover aliases, fold each category independently, merge only complete successful maps, and return an error only if all advertised categories fail. Pass a remaining row budget into page validation so all successfully combined categories retain the existing aggregate `MAX_ITEMS = 5_000` bound rather than receiving 5,000 rows each.

Add an accumulator merge helper with these exact invariants:

```rust
fn merge_access_point_map(
    target: &mut BTreeMap<String, AccessPointAccumulator>,
    source: BTreeMap<String, AccessPointAccumulator>,
) -> Result<(), AppError> {
    // same id + different name => schema_error
    // merge locks through existing conflict/bound checks
    // merge camera ids through existing aggregate bound checks
}
```

Convert the final map to `Vec<AccessPoint>` only after all successful categories have merged.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run:

```bash
cargo test --test contract intercom_provider -- --nocapture
cargo test --test door_open -- --nocapture
```

Expected: all provider contract and door safety tests pass; no mutation reaches MTS.

- [ ] **Step 6: Commit the implementation**

```bash
git add src/provider/mts/intercom.rs tests/contract/intercom_provider.rs
git commit -m "fix: discover MTS door categories"
```

### Task 2: Verification and local handoff

**Files:**
- No source changes expected.

**Interfaces:**
- Consumes: completed category-aware provider and existing `/api/v1/devices`, intent, and execute routes.
- Produces: a rebuilt local server whose existing UI receives non-empty access-point inventory.

- [ ] **Step 1: Run repository verification**

Run:

```bash
cargo fmt --all -- --check
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
npm --prefix frontend test -- --run
npm --prefix frontend run lint
npm --prefix frontend run typecheck
npm --prefix frontend run build
```

Expected: every command exits zero.

- [ ] **Step 2: Restart the development server with the new binary**

Build the Rust binary, stop only the known current `domru-mts` process, and start it bound to `127.0.0.1:3000` with the existing environment. Do not start an external browser.

- [ ] **Step 3: Perform an authenticated read-only smoke check**

Using the existing local authenticated session, call only dashboard/device read routes. Report only:

```text
access_points=<count> available=<count> locks=<count> openable=<count>
```

Expected: `access_points`, `locks`, and `openable` are greater than zero for an account whose APK shows doors. Do not create or execute a door-open intent.

- [ ] **Step 4: Inspect final state**

Run:

```bash
git status --short
git log -3 --oneline
```

Expected: the implementation and design/plan commits are present, and no unrelated or secret-bearing files are tracked.
