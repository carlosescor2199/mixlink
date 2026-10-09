# Preexisting Client Bugs Found During The Split

## Objective

Fix the three client bugs that surfaced while splitting `MainActivity`, all of them older than
that refactor and deliberately left alone at the time because a move must not smuggle in fixes.

## Why

Two are small and unambiguous. The third can leave a musician with a dead volume control, which is
worth fixing properly rather than quickly.

## The Three Bugs

### Bug 1: the More of me toggle survives losing its channel

`renderMusicianChannel` clears a musician channel that is out of range and persists `null`, but it
never turns the `More of me` checkbox off. After the channel count shrinks, the box can stay
checked while the send path sees a null channel and silently falls back to the raw snapshot. The
control looks on and does nothing.

Fix: whenever the musician channel becomes null, turn `More of me` off, persist that, and say so in
the status line rather than leaving a lying checkbox.

### Bug 2: mix ownership is never reset at the start of a session

`mixAcknowledged` means "the server accepted a mix from me, so the server owns it". It is set on the
acknowledgement and never cleared. Two consequences:

- Within one process, a Stop followed by a Start keeps the flag set from the previous session, so
  until a fresh acknowledgement arrives the client applies nothing locally.
- Worse, if the server is restarted while the client is stopped, the server's mix state goes back to
  neutral. The client still believes the server owns the mix, so it applies nothing either, and the
  master volume control is dead. The status line then claims the mix is held at the server's last
  setting, which is no longer true.

Fix: clear the flag when a session starts. The client immediately sends its current mix and gets a
fresh acknowledgement within milliseconds, re-establishing ownership. If the control channel never
comes up, the musician keeps a working master volume, which is a far better failure than a dead
slider.

**The tradeoff, recorded rather than hidden:** between the start of a session and the first
acknowledgement there is a short window in which the server may still be applying a retained mix
while the client also applies the master locally. In practice the window is milliseconds and the
client starts from neutral values. Treating a possibly-stale retained mix as authoritative is what
caused the dead slider; a brief overlap is the cheaper error.

### Bug 3: the starting status is overwritten immediately

`startReceiver` sets `Status: Starting...` and then calls `updateStats(0, 0, null)` inline, which
sets the status to `Receiving` because `running` is already true. The starting line is never seen.

Fix: order it so the starting status survives until real packets arrive.

## Scope

Client only. No server change, no protocol change, no layout change, no new dependency.

## Out of Scope

- Groups, iOS, the engineer ceiling (RF12).
- Any other refactor.

## Tasks

- [x] C1: Bug 1, with the checkbox, the persisted flag and the status line kept consistent.
- [x] C2: Bug 2, with a named way to clear ownership called from the start path.
- [x] C3: Bug 3.
- [ ] C4: Build checks, then confirm the three behaviours on the device.

## Verification

- `gradle test assembleDebug`: passed, 24 Android JVM tests, 0 failures, re-run by the orchestrator
  as a spot check with matching counts.
- `cargo test --workspace`: 33 passed, untouched.

Where each fix landed, checked in the tree after the change:

| Fix | Evidence |
| --- | --- |
| Bug 1 | `moreOfMeEnabled = false` plus the out-of-range status message, inside the existing out-of-range branch so the checkbox, the flag, the persisted channel and the status agree, followed by a re-send so the derived boost is dropped |
| Bug 2 | `ControlChannel.clearMixOwnership()` called from `startReceiver` before the status render and the connect |
| Bug 3 | `updateStats(0, 0, null)` now runs before the starting status, so the starting line survives until the first packet |

Test-first was waived for all three and that is deliberate: the affected code is Activity and
WebSocket lifecycle with no JVM coverage, `clearMixOwnership` is a one-line setter, and the pure
`moreOfMe` derivation already has tests. No artificial test was invented.

### C4

Not done. The three behaviours have not been seen on a device.

## Acceptance Criteria

- Clearing or losing the musician channel turns `More of me` off, persists it, and says so.
- Starting a session clears the ownership flag, so a failed control channel leaves the master volume
  working.
- `Status: Starting...` is visible until the first packet arrives.
- The existing 24 JVM tests still pass with the same counts.
- No behaviour changes beyond these three.

## Acceptance Checks

- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`
- `cargo test --workspace` (must stay green; the server is not touched)

## Route

Delegated: one bounded writer. All three are small, but two of them live in code with no JVM
coverage, so the device session is the real verification.

## Delivery Forecast

Under 100 authored lines. Well below the budget.

## Known Limitation

The fixes are verified by a build plus a device session, because the affected code has no unit
coverage. Bug 2 in particular is a failure-mode improvement rather than a correctness proof, and
the tradeoff it accepts is documented above so a future reader does not rediscover it as a bug.
