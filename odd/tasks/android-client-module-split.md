# Android Client Module Split

## Objective

Split `apps/android/app/src/main/java/com/mixlink/android/MainActivity.kt` from 764 lines into
focused modules, so the next features do not keep landing in one file.

## Why

The same single-file problem the server had, now on the client. `MainActivity` holds the UDP receive
loop, AudioTrack playback and metrics, the WebSocket control channel and its state machine, the
building of every per-channel control, the banks UI, and the Activity lifecycle. Groups come next
and would land in the same file again.

## This Refactor Is Riskier Than The Server One, And That Changes The Rules

The server split was safe because 26 tests covered most of the moved code. **`MainActivity` has
almost no JVM coverage**: the receive loop, the control channel and every view builder are
untested. A move that compiles is therefore NOT evidence that it still works.

Two consequences, both mandatory:

1. **Move blocks verbatim.** No logic edits, no renames beyond the mechanical ones needed to pass
   values in, no "while I am here" cleanups. If a block cannot be moved without changing it, leave
   it in `MainActivity` and report that.
2. **The behavioural verification is the device session that follows**, not the build. That session
   was already planned and now does double duty: it validates the split as well as the banks
   feature.

If, while moving, something looks wrong, **report it and leave it alone**. The purpose is to move
code, not to fix it.

## Target Layout

```
MainActivity.kt          lifecycle, view lookup and the wiring between the pieces
ClientMixState.kt        the volatile per-channel and master values, their defaults and mutation
PmonPlayer.kt            the UDP receive loop, AudioTrack playback and the receiver metrics
ControlChannel.kt        the OkHttp WebSocket, its connection state, sending and event reporting
ChannelControlViews.kt   building the per-channel fader, pan, mute and solo rows
BankControlViews.kt      building the banks, musician channel and More of me controls
```

Guidance on the seams:

- `PmonPlayer` needs no `Context`: `AudioTrack` is built from `AudioTrack.Builder`. It reports
  metrics, errors and completion through a listener interface that `MainActivity` implements.
- `ControlChannel` owns the executor, the client and the listener, and reports opened, acknowledged,
  error and closed events through a listener interface.
- The two view builders are functions that take a `Context` and a container, and return what the
  caller needs to wire up. They must not reach back into the Activity.
- `ClientMixState` holds `channelGains`, `channelPans`, `channelMutes`, `channelSolos`, the master
  values and `sourceChannels`, keeping the existing `@Volatile` copy-on-write pattern exactly as it
  is. Do not redesign it.

## Out of Scope

- Any behaviour change, however small.
- Splitting `Pcm16Processor`, `PmonPacket`, `ControlStatus`, `MixSnapshot` or `MixBankStore`; they
  are already focused.
- Changing the layout XML, the protocol or the server.
- Adding dependencies.

## Tasks

- [ ] A1: Extract `ClientMixState` and update every reader and writer.
- [ ] A2: Extract `PmonPlayer` with its listener interface.
- [ ] A3: Extract `ControlChannel` with its listener interface.
- [ ] A4: Extract `ChannelControlViews` and `BankControlViews`.
- [ ] A5: Reduce `MainActivity` to lifecycle and wiring, then run the build checks.

## Acceptance Criteria

- `gradle test assembleDebug` passes and the existing 24 JVM tests still pass with the same counts.
- `MainActivity.kt` is substantially smaller and contains no receive loop, no WebSocket handling and
  no view construction.
- Every moved block is behaviourally identical. The device session is what proves it.
- No new dependency, no layout change, no protocol change, no server change.

## Acceptance Checks

- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`
- `cargo test --workspace` (must stay green; the server is not touched)

## Route

Delegated: one bounded writer, with a very explicit instruction not to change behaviour.

## Delivery Forecast

Roughly 700 moved lines across seven files. Almost all movement.

## Device Confirmation

The device session that followed is what verified this split behaviourally. On SM-S916B the app
connected to the server, streamed audio, rendered every per-channel control, saved and recalled a
bank, and exercised `More of me`, all through the reworked wiring.

That matters because it is the first time the moved code ran at all. A build only proves the pieces
compile together; it says nothing about whether the receive loop still receives, the socket still
connects, or the control closures still reach the right state. The session is recorded here as the
evidence, not the build.

## Known Limitation

The split is verified by a build and by the device session that follows, not by unit tests, because
the moved code has no JVM coverage. That is recorded rather than hidden: a future refactor of this
area should build the coverage first.
