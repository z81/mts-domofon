# Direct lock buttons

## Goal

Remove the lock selector from each access-point card. Render one direct action for every real MTS lock so the user chooses the door by pressing its named button.

## Interaction

- Keep one card per access point.
- Render one button per lock inside the card, in provider order.
- For a single lock, keep the compact label **Открыть** and an accessible name that identifies both the access point and lock.
- For multiple locks, label each button **Открыть: {lock name}**. If a lock name is blank, use **Замок {number}** as the visible fallback.
- Pressing a button prepares an intent for exactly that lock. There is no select state and no implicit first-lock choice.
- Keep the existing confirmation dialog; it shows the chosen lock and remains the only place that executes the physical open request.
- While any lock on an access point is being prepared, disable all buttons on that point and show **Подготовка…** only on the pressed lock.
- An unavailable access point or a point without locks keeps one disabled **Недоступно** action.

## Scope

Only `AccessList` rendering, its local pending identity, styles, and focused frontend tests change. Backend inventory, intent, execute, retry, and confirmation semantics remain unchanged.

## Verification

- A failing UI test first proves a two-lock point has no combobox and exposes two named buttons.
- Pressing the second button must call `createDoorIntent` with the second lock number and leave execution pending until explicit confirmation.
- Existing cancellation, expiry, double-click suppression, unavailable-state, lint, typecheck, production build, and full Rust checks must remain green.
- No real door-open intent is created during verification.
