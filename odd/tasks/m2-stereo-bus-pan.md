# M2 Stereo Bus and Pan

## Objective

Turn the per-channel gain table into a real mixer: N mono source channels summed into a stereo
bus, with a pan control per source, so a musician can place each instrument in the stereo field
instead of having source channel `k` permanently bound to output slot `k`.

## Why

Source channel `k` currently maps to output slot `k`, so with two mono sources on a two-channel
interface the first is permanently in the left ear and the second permanently in the right.
A guitar on `INPUT 2` can be made quieter but cannot be centred. PRD RF9 asks for per-channel
faders **and pan**; pan is the missing half.

## The consequence, stated up front

Pan only makes sense in a summing model. Adding it means the server stops copying the
interleaved layout and starts building a stereo bus, so:

- The UDP payload stays stereo and the PMON v1 wire format is unchanged.
- `AudioPacket.channels` now describes the **output** (always 2), not the captured source count.
  The client therefore learns the number of **source** channels over the control channel, not
  from the audio packet.

The default pan is chosen to reproduce today's audio exactly: source `k` defaults to hard left
when `k` is even and hard right when `k` is odd. With two channels and neutral gains the output
is therefore byte-identical to the current build, and the musician moves sources to taste. This
mirrors the deliberate choice made for the gain table, for the same reason.

## Scope

- Server: mix N sources into a stereo bus with per-source gain and pan, using an equal-power pan
  law.
- Protocol: add an optional `pans` array to the `mix` message and echo it in the
  acknowledgement.
- Protocol: send a `config` message with the source channel count when a control connection
  opens, so the client knows how many sources it is mixing.
- Android: one pan control per channel, driven by the `config` count, and apply pan in the local
  fallback path too.

## Out of Scope

- Groups, solo, saved mixes, iOS.
- Reading more than one audio device.
- Anything that changes the PMON v1 wire format.

## Pan Law

Equal power, the console standard. For pan `p` in 0-100 with 50 at centre:

```
theta = (p / 100) * (PI / 2)
left  = cos(theta)
right = sin(theta)
```

At hard left the source is fully in the left channel, at centre each output gets `0.707` of it,
and hard right is the mirror of hard left.

## Protocol Change

Client to server:

```json
{ "type": "mix", "volume_percent": 80, "max_level_percent": 90, "muted": false,
  "channels": [100, 40], "pans": [50, 100] }
```

- `pans` is optional and follows the same rule as `channels`: absent means preserve the current
  values, present means replace the entries supplied, clamped to 0-100, and a list longer than
  the supported maximum is rejected.

Server to client, sent once when the control connection opens:

```json
{ "type": "config", "source_channels": 2, "sample_rate": 48000 }
```

Acknowledgement:

```json
{ "type": "mix_ack", "volume_percent": 80, "max_level_percent": 90, "muted": false,
  "channels": [100, 40], "pans": [50, 100] }
```

## Tasks

- [x] P1: Server stereo bus with per-source gain and equal-power pan, defaulting to the current layout, with pure tests.
- [x] P2: Protocol `pans` array plus the `config` message, with parsing tests and a live WebSocket probe.
- [x] P3: Android pan controls per channel, the source count taken from `config`, and the local fallback reduced to master controls because the received stream is already the server's stereo output.
- [ ] P4: Real-audio check on the device with the guitar, then the workspace and Android checks.

## Consequence for the Client

Once the server sums into a stereo bus, the client receives an **already mixed** stereo stream.
`AudioPacket.channels` is therefore 2, and the client can no longer see the individual sources.

Two things follow, and both are deliberate rather than accidental:

1. The channel count that drives the client controls must come from the `config` message, not
   from `packet.channels`. Reading it from the audio packet would show two faders forever.
2. The local fallback can only apply master volume, mute and the ceiling. It cannot apply the
   per-channel gains or the pans, because the audio it holds is the finished mix; there is
   nothing left to pan. The per-channel gains that the local path applied after the previous
   increment must therefore be removed from it.

The honest limitation that follows: **per-source control requires a working control channel.**
If the control port is unreachable while UDP audio flows, a musician keeps master volume, mute
and the ceiling, and their per-channel faders do nothing until the control channel comes back.
Making per-source control work without the control channel would mean shipping raw multichannel
audio to every client, which is a different architecture and a much larger bandwidth budget.

## Verification

### P1 and P2

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 26 tests (5 added for pan).
- `cargo check --workspace`: passed.

New coverage: centring a source puts it in both outputs at `0.707`, panning everything left
empties the right output, a four-channel source is summed into a two-channel output, an absent
`pans` field preserves the previous pans while an oversized list is rejected, and the default
pan table reproduces the captured interleaved layout.

Live WebSocket probe against the rebuilt release binary, with `127.0.0.1` as the target:

| Step | Result |
| --- | --- |
| Connect | `{"type":"config","source_channels":2,"sample_rate":48000}` |
| `channels:[100,40]` + `pans:[50,100]` | `channels=[100,40,100,100]`, `pans=[50,100,0,100]` |
| `pans:[0,0]` only | `channels=[100,40]` unchanged, `pans=[0,0]` |

The third row is the point of the merge rule: a message carrying only one of the two arrays
leaves the other alone.

Process note: the release binary had to be rebuilt again before probing, and the running server
holds a lock on it, so `cargo build --release` fails with `Acceso denegado (os error 5)` until
the process is stopped.

### P3

- RED observed first: `No value passed for parameter 'channelGains'` at six call sites, in both
  the debug and release unit-test compilation, after the tests moved to the corrected API.
- `gradle test assembleDebug`: passed, 14 Android JVM tests, 0 failures. The orchestrator re-ran
  the same command as a spot check and it agreed with the reported counts.
- The client takes the source count from `config`, builds one fader and one pan control per
  source, sends the `pans` array with every mix update, and mirrors the server's default pan
  table so the first message after connect does not move any source.
- The local fallback dropped the per-channel gains and applies master controls only, as the
  "Consequence for the Client" section requires. `activity_main.xml` needed no change because
  the pan controls are built into the existing `channelContainer`.

Not verified for this half: nothing was run on a device. Per-source and pan behaviour through
the app, including that a pan actually moves a source in the stereo field, is covered by P4 and
has not been done yet.

## Acceptance Criteria

- With neutral gains and default pans the stereo output is identical to the pre-change build.
- Panning a source to centre places it in both outputs at `0.707` of its level.
- Panning to an extreme removes that source from the opposite output.
- Mute still zeroes everything; the ceiling still clamps before conversion.
- An absent `pans` field preserves the previous values; oversized lists are rejected.
- The PMON v1 wire format and the UDP packet size for a stereo output are unchanged.

## Acceptance Checks

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`
- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`

## Route

Direct inline for the server and protocol work in `apps/server/src/main.rs`, then a separate
Android unit for the controls.

## Delivery Forecast

Roughly 360 authored changed lines across the server, protocol and Android work, split as
P1+P2 (server and protocol) and P3+P4 (Android and validation). Each half sits below the
400-line budget, so the default `ask-on-risk` strategy needs no chain.

## Known Limitation

With the two-channel Volt 4 the mixer has two mono sources, so pan moves those two sources
around a stereo field rather than placing a full band. The summing model itself is what a
multichannel interface needs, and that remains the X32 scenario.
