# M2 Mix Banks and More Of Me

## Objective

Give the musician a way to save the current mix under a name and recall it later, and a single
control that makes them louder relative to the band without rebuilding anything (PRD RF10).

## Why

Two problems, one increment.

Right now the only way to get a previous mix back is to rebuild it by hand, fader by fader. A
musician who tries something during a rehearsal and wants the old balance back has no path. And
"more of me" is the product's core value expressed as one control: every other feature so far has
been plumbing toward the moment a musician can press one button and hear more of themselves.

## Scope

Android client only. **No server change and no protocol change.**

- A named, locally stored mix snapshot: the four per-channel arrays plus master volume, ceiling
  and master mute.
- Save the current mix under a name, recall a stored one, delete one.
- A way for the musician to say which channel is theirs.
- A `More of me` control that sends a derived mix while it is on.

## Out of Scope

- Server-side presets, sharing a bank between devices, engineer-authored presets.
- Groups, and anything needing engineer configuration.
- iOS.
- The engineer volume ceiling (RF12), still unimplemented.

## Design Decisions

**Banks live on the device.** They are the musician's own, so they belong on their phone and need
no protocol. Recalling a bank applies the values locally and then sends the resulting mix through
the existing `mix` path, exactly as if the musician had moved the controls by hand. That is what
keeps this increment free of server work.

**`More of me` is derived, never stored.** While it is on, the mix that gets sent is computed from
the stored values; the stored values themselves are untouched. Turning it off therefore restores
exactly what the musician had, with nothing to save or replay. This is the same principle that
makes leaving solo restore the previous levels, and it is the property to test.

**What `More of me` does.** The musician's channel is set to 100 and every other channel is
multiplied by 0.5. Both halves matter: halving the others always produces an audible change, and
forcing the musician's channel to full guarantees the ratio moves in their favour even if that
channel was turned down. The constants are deliberately simple and are the obvious thing to tune
later.

**Which channel is mine.** The musician designates it, because there is no engineer configuration
yet and PRD RF8 has no profile flow implemented. If no channel is designated, the control is
inert and says so rather than guessing.

**Persistence.** `SharedPreferences` holding JSON, with a guard so malformed stored data cannot
crash the app. Musicians leave a saved bank on a phone for weeks; it has to survive restarts.

## Tasks

- [x] B1: A pure `MixSnapshot` type with JSON round trip and the `More of me` derivation, test-first.
- [x] B2: Local persistence plus save, recall and delete of named banks in the UI.
- [x] B3: A way to designate the musician's channel, and the `More of me` control wired to the send path.
- [ ] B4: Build checks, then validate saving, recalling and `More of me` on the device.

## Verification

### B1 to B3

- `gradle test assembleDebug`: passed, 24 Android JVM tests, 0 failures (10 added:
  `MixSnapshotTest` 6, `MixBankStoreJsonTest` 4). Re-run by the orchestrator as a spot check with
  matching counts.

### A design correction worth recording

The first implementation avoided `org.json` by hand-writing a **general-purpose JSON parser**
(around 180 lines: `readDocument`, `readValue`, `readObject`, `readArray`, `readString`,
`readNumber`, `readLiteral`, `skipWhitespace`, `expect`) inside `MixSnapshot.kt`.

The diagnosis was correct: this module sets `unitTests.isReturnDefaultValues = true`, so AGP's
mockable `android.jar` stubs `org.json` during local unit tests and a JVM round trip cannot use it.
The remedy was not. A hand-written JSON parser in production is a liability — escapes, unicode,
malformed input and nesting are exactly where they break — and it had pushed the file to 246 lines
to express about 60 lines of logic.

Fixed by adding a test-only dependency:

```kotlin
testImplementation("org.json:json:20240303")
```

`MixSnapshot.kt` went from 246 to 85 lines and the parser is gone; a tree-wide grep for
`MixJson|readDocument|readValue|readObject|readArray|readString|readNumber|readLiteral|skipWhitespace`
returns no matches.

**The dependency's effect was proven, not assumed.** With the dependency all 24 tests pass; with it
commented out the two round-trip tests fail with a `NullPointerException`, because a stubbed
`org.json` returns null and cannot round-trip an object. The failure inversion is the evidence that
the real implementation is on the test classpath.

Lesson for the next delegation: the worker was pushed into the workaround by an
allowed-edit-surface list that excluded `build.gradle.kts`. When a constraint in the spec would
force a poor solution, the surfaces should include the file that holds the proper fix.

### B4

Verified on the device, on SM-S916B with a guitar on `INPUT 2`:

- A bank was saved under a name and appeared in the list.
- The mix was then deliberately changed and the bank recalled; the operator confirmed the faders,
  the pan, the mutes and the solos all came back.
- `More of me` was exercised with the guitar designated as the musician's channel.
- The bank survived closing and reopening the app, which exercises the `SharedPreferences` load and
  the JSON parse path: the newest and least covered code in this increment.

The operator was asked to confirm the restore and the restart persistence separately rather than
accepting a general "it works", because neither can be inferred from the rest working. In particular
an in-memory-only store would have looked identical until the app was restarted.

Not exercised: `More of me` with no channel designated. The unit tests cover the derivation
returning null in that case, but nobody saw the UI refuse it.

## Known Problem Introduced Here

`MainActivity.kt` grew from around 464 to **764 lines** with the banks and `More of me` wiring. That
is the same single-file problem this project just fixed on the server, now on the client. It should
be split the same way, and that is the next increment rather than a task inside this one.

## Acceptance Criteria

- A snapshot round-trips through JSON with every field preserved, including all four channel arrays.
- Recalling a bank sets the controls and sends the resulting mix.
- Turning `More of me` off restores exactly the values the musician had, with nothing replayed.
- With no channel designated, `More of me` does nothing and the UI says why.
- Banks survive an app restart.
- Malformed or missing stored data does not crash the app; it falls back to an empty bank list.
- The PMON wire format and the control protocol are unchanged.

## Acceptance Checks

- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`
- `cargo test --workspace` (must stay green; the server should not have been touched)

## Route

Delegated: one bounded writer. The pure logic is test-first, and the UI wiring follows the pattern
already used for the per-channel controls.

## Delivery Forecast

Roughly 320 authored changed lines across a new snapshot type, persistence, the UI wiring and
tests. Below the 400-line budget, so the default `ask-on-risk` strategy needs no chain.

## Known Limitation

With the two-channel Volt 4 a bank holds two channels, which is enough to prove recall but not
representative of a full band. The model itself is channel-count agnostic.
