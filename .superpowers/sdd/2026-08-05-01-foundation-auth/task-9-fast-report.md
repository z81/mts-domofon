# Task 9 — fast core intercom UI report

## Scope delivered

- Responsive Russian-language VDome preview with navigation for dashboard, access, cameras/live/archive, calls, events, and call settings.
- Synthetic, explicitly labelled MTS ID / phone / SMS UI flow; no external auth request, token storage, secrets, or MTS mutations.
- Per-door confirmation; confirmation for opening during a call; confirmation for blocking a caller.
- Incoming, active, and recent call views, including microphone, camera and audio-output controls.
- Browser Notifications affordance, camera archive timeline and synthetic clip-download presentation, linked-flat switcher and typed future client boundary.
- Deliberately excluded: support/FAQ/appeals, favourites, documents, resident management, billing/meters, passes, marketplace, theme/share, and auto-open.

## RED / GREEN evidence

1. Added four behaviour tests before implementation for MTS phone/SMS flow, archive/clip flow, active-call door confirmation, and notification/block confirmation. Existing two dashboard/door-confirmation tests were retained.
2. RED: `npm test -- src/app/App.test.tsx` failed with 4 missing behaviours (no phone login action, camera/call navigation, and settings notification action).
3. GREEN: after the state-driven implementation, `npm test` reported `6 passed`.

## Verification commands

```text
cd frontend && npm test
Test Files  1 passed (1)
Tests  6 passed (6)

cd frontend && npm run typecheck
exit 0

cd frontend && npm run build
✓ built in 88ms
```

## Files changed

- `frontend/src/app/App.tsx` — interactive screen flows and safe synthetic states.
- `frontend/src/app/App.test.tsx` — retained two existing tests plus four behavioural flows.
- `frontend/src/app/styles.css` — responsive styles for the added core screens.
- `frontend/src/api/intercom.ts` — typed, uninstantiated future API boundary without secrets.
- `.gitignore` — ignores `frontend/tsconfig.tsbuildinfo`.

## Self-review and integration gaps

- All preview-changing actions are local UI state and display a synthetic/safe notice; no backend endpoint is called.
- Future backend integration should supply an authenticated implementation of `IntercomPreviewClient`, real permission/error states, WebRTC media handling and native `Notification.requestPermission()` behind an explicit product policy.
- The generated camera image is CSS geometry, not copied APK material; live/archive timestamps and all events are synthetic.
