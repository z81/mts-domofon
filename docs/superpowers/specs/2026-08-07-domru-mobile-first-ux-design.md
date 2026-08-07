# Dom.ru Mobile-First UX Design

## Goal

Replace the current dashboard-style shell with a resident-facing intercom interface modeled on the information architecture and interaction density of the user-provided Dom.ru APK. The redesign must make the most common actions immediate:

- see the entrance camera;
- open a specific door with one obvious action;
- notice and handle an incoming call;
- review recent events and calls;
- reach archive and bounded auto-open without crowding the home screen.

This is a frontend information-architecture and presentation change. Existing authenticated MTS-backed behavior, ownership checks, confirmations, media handling, and local API contracts remain authoritative.

## Design Direction

The application uses the Dom.ru mobile model instead of an administrative dashboard:

- white consumer UI with dark text, pale neutral surfaces, and a coral-red primary accent;
- three persistent destinations only: **Главная**, **События**, and **Ещё**;
- the home screen is the operational hub for cameras and doors;
- secondary controls appear on detail screens or in **Ещё**;
- incoming calls temporarily take priority over normal navigation;
- desktop widens the same interface instead of introducing a separate sidebar product.

The rejected alternatives are retaining five top-level sections or merely reskinning the current shell. Both preserve duplicate navigation and make frequent intercom actions harder to find.

## Application Shell

### Mobile

A compact header identifies the current home or property. A fixed white bottom bar contains three labeled icon buttons:

1. **Главная**;
2. **События**;
3. **Ещё**.

The active destination uses the coral-red accent. Content has 16-pixel side spacing, large touch targets, and enough bottom padding to stay clear of the navigation bar.

### Desktop

The same three destinations remain visible in a narrow top or left navigation treatment, without the current large dark sidebar. Content is centered with a comfortable maximum width. The home screen may use two columns when space allows: camera/media on the left and door actions plus recent activity on the right. Controls retain mobile-sized touch targets.

## Home Screen

The home screen orders content by frequency and urgency.

### Header and property selection

The screen title is **Главная**. When multiple flats exist, the selected address is a compact property control under or beside the title. Changing it refreshes cameras, access points, events, call state, archive ownership, and auto-open state using the existing behavior.

Authentication problems replace provider-backed content with a clear sign-in action. Individual feature failures remain isolated and do not blank the entire screen.

### Camera card

The primary camera appears first in a rounded media card. The card contains:

- live image or explicit loading/error/unsupported state;
- camera name and online state;
- one primary **Смотреть** action;
- a mute/unmute control whenever the live stream has audio;
- a compact link to switch cameras when more than one exists.

Selecting the card opens a camera detail surface with three capability-driven modes: **Эфир**, **Кадр**, and **Архив**. Unsupported modes are omitted rather than displayed as dead controls. The media area preserves aspect ratio and never substitutes mock imagery for unavailable provider media.

### Door actions

The **Двери** section follows the camera. Every owned lock is rendered as its own large button/card; there is no lock selector. The visible label combines the access point and lock name only when needed to distinguish multiple locks.

Pressing a door button prepares the existing short-lived unlock intent and opens a concise confirmation sheet naming the exact address, access point, and lock. The final confirmation performs the physical action. Pending, success, unavailable, expired-intent, and failure states stay attached to the relevant door so another door remains understandable.

### Auto-open status

An armed auto-open policy is always visible near the doors as a high-salience status strip with its exact scope: next call or deadline. The strip has a direct disable action. When disabled, a small secondary **Автооткрытие** entry opens its settings; the full policy form does not occupy the home screen.

### Recent activity

The latest few owned events appear as a compact preview below primary actions. A **Все события** action switches to the Events destination. This preview failure does not affect camera or door controls.

## Camera Detail

Camera detail opens as a full content page on mobile and a large panel/page on desktop. It retains the selected camera and provides:

- **Эфир** with HLS playback, native play/pause, and sound controls;
- **Кадр** with explicit refresh and retry;
- **Архив** with recorded intervals, date/time selection, play, stop, and honest provider-unavailable messaging.

A compact camera switcher is available only when multiple cameras exist. Leaving the detail surface stops disposable playback sessions through the existing cleanup behavior.

## Events Screen

**События** is one chronological feed, not separate top-level pages. A horizontally scrollable filter row contains:

- **Все**;
- **Звонки**;
- **Доступ**;
- **Камеры**.

Cards use a thumbnail or tinted event icon, a short title, source, outcome, and local time. Call outcomes have readable text in addition to color. Pagination is a simple **Показать ещё** action.

Selecting a camera event with an archive timestamp opens that camera in Archive mode at the event time. Call entries expose their recorded outcome and duration but do not add a duplicate persistent Calls destination. Empty, loading, retryable-error, and exhausted states are explicit.

## More Screen

**Ещё** contains only secondary items that belong to the requested core intercom scope:

- current address/account and authentication/session actions;
- bounded auto-open configuration;
- technical connection status when it is actionable.

Support, FAQ, requests, documents, theme selection, sharing, favorites, billing, and unrelated profile settings are excluded.

## Incoming Call Experience

An owned incoming call presents above the current destination so the user does not need to find a Calls tab. On mobile it is a full-screen call surface; on desktop it is a prominent modal/panel. It shows the source, live media when available, connection phase, and large actions for:

- answer;
- reject/end;
- mute/unmute as applicable;
- open the door associated with the current call.

Opening a door still uses the safe call-scoped behavior. Auto-open results are shown in the call surface without silently retrying. Once a call is finished, the user returns to the prior destination and the result remains available in the Events feed.

## Interaction and Accessibility Rules

- Primary tap targets are at least 44 by 44 CSS pixels.
- Text is not reduced to dashboard-style microcopy; secondary labels remain readable on a phone.
- Icon-only controls have Russian accessible names.
- Focus is trapped and restored for confirmation and call dialogs.
- Loading does not cause the whole layout to jump; media and list skeleton areas keep stable dimensions.
- Destructive or physical actions use explicit labels and confirmation where already required.
- Color is never the only signal for availability, call outcome, or error state.
- Safe-area insets are respected for mobile bottom navigation and full-screen call controls.

## State and Component Boundaries

The redesign should split the current monolithic application component into presentation-focused surfaces while keeping shared orchestration centralized:

- application shell and three-destination navigation;
- home screen;
- camera card and camera detail;
- door action grid and confirmation;
- events screen and recent-events preview;
- more/settings screen;
- incoming-call overlay.

The existing feature APIs remain injected for tests. Current ownership, session, polling, object-URL cleanup, HLS cleanup, unlock-intent, and auto-open state logic must not be weakened during extraction.

## Testing and Verification

Implementation follows red-green-refactor for the new navigation and extracted surfaces. Frontend coverage must prove:

- exactly three persistent destinations and usable mobile navigation;
- home camera and independent door actions;
- exact-door confirmation before unlock execution;
- capability-driven camera modes and media cleanup;
- events filtering, paging, and archive navigation;
- auto-open armed/disabled presentation and safe configuration flow;
- incoming-call priority and return to the prior screen;
- loading, empty, unauthorized, unavailable, and retry states;
- desktop and narrow layouts do not hide core actions.

Existing Rust and frontend suites remain green. Runtime verification is read-only: it may load the dashboard, devices, snapshots, event history, archive availability, and media manifests, but must not open a real door, answer/reject a real call, or arm auto-open.
