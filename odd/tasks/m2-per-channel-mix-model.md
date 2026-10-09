# M2 Per-Channel Mix Model

## Objective

Replace the single scalar gain that every client currently receives with a per-source-channel
gain matrix owned by each client, so a musician can build their own blend of the band instead
of only changing their overall volume.

## Why

`MixState` holds three scalars (`volume_percent`, `max_level_percent`, `muted`) and `apply_mix`
multiplies every sample of the interleaved stream by the same gain. Every client therefore
receives byte-identical audio with a different overall level. A musician cannot hear more of
themselves relative to the rest of the band, which is the product's core value (PRD RF3, RF9).
M2 asks for a complete native interface with per-channel faders, and that interface needs a
model to control.

## Scope

- Add a per-source-channel gain matrix to the server mix state.
- Apply it per channel in the mix path, keeping master volume, ceiling and mute as master
  controls on top of it.
- Extend the control protocol so a client can send and receive the per-channel gains.
- Add one fader per announced channel to the Android client.
- Keep the PMON UDP wire format unchanged.

## Out of Scope

- Pan. RF9 includes pan, but pan changes the summing model (mono sources into a stereo bus) and
  belongs with the mixer interface increment.
- Capturing more than the interface's native channel count. The Volt 4 exposes `INPUT 1/2` and
  `LINE IN 3/4` as two independent stereo devices with independent clocks; combining them is
  not attempted here.
- Reading multiple devices, groups, solo, saved mixes, iOS.

## Design Decisions

**Channel mapping preserves today's audio.** Source channel `k` maps to output slot `k` in the
interleaved stream (`index % channels`). With every gain at 100% the output is identical to the
current implementation, so this increment cannot regress the validated M0/M1 audio path.

**Alternatives considered and rejected.**

- Summing every source channel into both outputs (a mono bus) would deliver the "more of me"
  blend with only two sources, but it collapses stereo to mono for every musician until pan
  exists. That is a quality regression a musician would notice immediately, so it is deferred
  to the pan increment where it becomes correct.
- Treating source channels as stereo pairs would preserve stereo, but with the Volt 4's two
  channels it degenerates to exactly one fader, which proves nothing.

**Consequence, stated plainly.** With a two-channel interface this increment yields per-channel
control over two source positions, which on a stereo pair behaves like a balance control rather
than a true per-musician blend. It validates the model, the protocol and the interface plumbed
end to end. A genuine per-musician blend needs a multichannel interface with one source per
channel, which the PRD already schedules as the X32 scenario after M0.

**Master controls stay.** `volume_percent`, `max_level_percent` and `muted` remain, because the
engineer ceiling in RF12 lives on the master and must survive the per-channel matrix.

## Protocol Change

Client to server:

```json
{ "type": "mix", "volume_percent": 80, "max_level_percent": 90, "muted": false, "channels": [100, 40] }
```

- `channels` is optional. When absent, the existing per-channel gains are preserved, so a client
  that does not send it keeps working.
- Values are clamped to 0-100. A list longer than the supported maximum is rejected.

Server acknowledgement:

```json
{ "type": "mix_ack", "volume_percent": 80, "max_level_percent": 90, "muted": false, "channels": [100, 40] }
```

The acknowledgement echoes the full gain table, one entry per supported channel, not only the
entries the client sent. A client applies the entries it has faders for and ignores the rest.

## Constraints

- Do not change the PMON v1 UDP wire format.
- Do not edit `PRD.md`.
- Keep the CPAL callback free of locks, allocation and socket work.
- Keep technical artifacts in English.
- Support up to 32 source channels (PRD RF2).

## Tasks

- [x] T1: Add the per-channel gain matrix to the server mix state and apply it in the mix path, preserving current output at 100%, with pure tests.
- [x] T2: Extend the control protocol with the optional `channels` array and echo it in the acknowledgement, with parsing tests.
- [ ] T3: Add one fader per announced channel to the Android client and send the matrix with each mix update.
- [ ] T4: Run the workspace and Android checks, then validate the per-channel path end to end on the device.

## Verification

### T1 and T2

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 21 tests (6 added for this change).
- `cargo check --workspace`: passed.

New coverage: neutral gains leave a buffer unchanged, one channel's gain changes only that
channel, an absent `channels` field preserves the existing gains, out-of-range gains are clamped
and oversized lists are rejected, and the acknowledgement reports the applied gains.

Design note recorded while testing: a neutral gain table is not bit-identical for `i16::MIN`,
which the full-scale ceiling clamps to `-32767`, exactly as the pre-change code did. The
pass-through guarantee is therefore "identical to the previous implementation", which is what
the change had to preserve.

## Acceptance Checks

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`
- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`

## Acceptance Criteria

- Every gain at 100% produces byte-identical output to the pre-change implementation.
- Changing one channel's gain changes only that channel's samples.
- An absent `channels` field leaves existing per-channel gains untouched.
- Out-of-range gains are clamped and the acknowledgement reports the clamped values.
- Mute still forces every sample to zero and the ceiling still clamps before conversion.
- The Android client renders one fader per channel announced by the audio stream.

## Route

Direct inline for the server and protocol work, which is localized to `apps/server/src/main.rs`
and its tests. The Android client work is a second, separate unit.

## Delivery Forecast

Roughly 340 authored changed lines across the server, the protocol documentation, the Android
client and their tests. Below the 400-line budget, so the default `ask-on-risk` strategy needs
no chain unless the implementation grows.

## Known Limitation

Unlike M1, this increment has **not** been validated with a two-device run at the time of
writing; see each task's verification notes.
