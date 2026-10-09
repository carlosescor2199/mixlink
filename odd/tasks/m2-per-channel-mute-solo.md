# M2 Per-Channel Mute and Solo

## Objective

Complete the per-channel control set in PRD RF9 by adding a mute and a solo toggle for every
source channel, so a musician can drop a channel or listen to one in isolation without moving any
fader and without losing the fader positions.

## Why

The client currently has a per-channel fader and a per-channel pan, plus one master mute. RF9 asks
for faders, pan, mute **and** solo per channel. Mute and solo are the two that remain.

The product reason: a musician rehearsing wants to hear only their own instrument for a bar, or
drop the click, and then come straight back. Reaching for a master mute or dragging a fader to zero
loses the setting they had, which is exactly what makes the move unusable live.

## Why groups are not in this increment

A group ("drums", "vocals") needs someone to define which channels belong to it. That is engineer
configuration, and there is no interface or protocol for it yet. Adding groups here would mix a
configuration problem into a mixing problem and roughly double the change, so it is deliberately
deferred to its own increment.

## Scope

- Server: add a per-channel mute flag and a per-channel solo flag to the mix state.
- Mixing rule: a channel contributes only when it is not muted and either nothing is soloed or
  that channel is soloed.
- Protocol: optional `mutes` and `solos` arrays on the `mix` message, echoed in the acknowledgement,
  with the same merge rule as `channels` and `pans` (absent preserves, oversized rejected).
- Android: one mute and one solo toggle per source channel, sent with every mix update, and
  applied in the local fallback path.
- Keep the PMON v1 wire format untouched.

## Out of Scope

- Groups, saved mixes, iOS.
- Engineer-side channel naming or configuration.
- Any change to the pan law or the stereo bus.

## Design Decisions

**Solo is per client, not global.** The mix state is already per client IP, so one musician soloing
a channel does not affect anybody else. That falls out of the existing model and needs no extra
machinery.

**Solo does not move the faders.** The rule is applied at mix time on top of the stored gains, so
coming out of solo restores exactly what the musician had.

**Combining mute and solo.** A channel is audible exactly when it is not muted and either no
channel is soloed or it is one of the soloed ones. Muting a soloed channel keeps it silent, which
is what a console does and is the less surprising rule.

**Flags are booleans, not levels.** `mutes` and `solos` use JSON booleans rather than the 0-100
values the other arrays use, because there is no meaningful intermediate value. They get their own
merge helper so the existing 0-100 clamp is not quietly applied to them.

## Protocol Change

Client to server:

```json
{ "type": "mix", "volume_percent": 80, "max_level_percent": 90, "muted": false,
  "channels": [100, 40], "pans": [50, 100], "mutes": [false, true], "solos": [false, true] }
```

Absent fields preserve the stored values, exactly like `channels` and `pans`. Lists longer than the
supported maximum are rejected.

Acknowledgement:

```json
{ "type": "mix_ack", "volume_percent": 80, "max_level_percent": 90, "muted": false,
  "channels": [100, 40], "pans": [50, 100], "mutes": [false, true], "solos": [false, true] }
```

The acknowledgement echoes the full tables, one entry per supported channel, not only the entries
the client sent.

## Tasks

- [x] S1: Server per-channel mute and solo flags plus the audible-channel rule, with pure tests.
- [x] S2: Protocol `mutes` and `solos` arrays with a boolean merge helper and acknowledgement echo, with parsing tests and a live WebSocket probe.
- [x] S3: Android mute and solo toggles per source channel, sent with every mix update.
- [x] S4: Validate mute and solo on the device with real audio.

Correction to an earlier draft of this document: S3 originally said the toggles would be "applied
in the local fallback", which contradicts the architecture note in
`m2-stereo-bus-pan.md`. The client holds the server's finished stereo mix, so there is no
per-channel audio left to mute locally. The toggles are protocol-only, and the worker correctly
followed the explicit constraint rather than the stale line.

## Verification

### S1 and S2

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 33 tests (7 added).
- `cargo check --workspace`: passed.

New coverage: solo silences every other channel, clearing the last solo restores the previous
levels exactly, muting a channel silences only that channel, a muted soloed channel stays silent,
solo applies on top of the stored gains without changing them, absent `mutes` and `solos` preserve
the stored flags, and the lists apply while an oversized one is rejected.

Live WebSocket probe against the rebuilt release binary:

| Step | Result |
| --- | --- |
| Connect | `{"type":"config","source_channels":2,"sample_rate":48000}` |
| `mutes:[true,false]` + `solos:[false,true]` | `mutes=[True,False,False]`, `solos=[False,True,False]` |
| A message carrying neither field | both preserved |
| 33 flags | `type=error` |

### S3

- `gradle test assembleDebug`: passed, 14 Android JVM tests, 0 failures, re-run by the
  orchestrator as a spot check with matching counts.
- One `CheckBox` per channel per flag, labelled `Mute N` and `Solo N`, with the flags held in
  `@Volatile` copy-on-write `BooleanArray`s following the existing gain and pan pattern, and sent
  as the `mutes` and `solos` arrays on every `mix` message.
- `Pcm16Processor` was deliberately not touched, for the reason recorded above.

Test-first was waived for S3 and that is deliberate: the change lives in Android checkbox listeners
and JSON construction on a background executor, so a test would assert Kotlin's own `BooleanArray`
semantics rather than any rule of ours. Structural verification by build was used instead.

### S4

Verified on the device with real audio, on SM-S916B with a guitar on `INPUT 2`:

- `Solo 2` left only the guitar audible and silenced the other channel.
- Clearing `Solo 2` restored both channels to exactly their previous levels, with no readjustment
  needed.

The second observation is the one that matters. It is what distinguishes applying solo at mix time
on top of the stored gains from the cheaper alternative of zeroing the gains, which would have made
the musician rebuild the mix after every solo. The operator was asked specifically to report any
level that did not come back correctly, and reported none.

Run environment: the release binary at `target/release/personal-monitoring.exe`, APK from
`gradle assembleDebug`, server on `192.168.1.34` with `192.168.1.10:50000` as the client target
plus a loopback target used to confirm capture was flowing before involving the phone (400 packets
in four seconds). Worth keeping: confirming capture on the loopback target first avoids chasing an
audio problem that is actually a dead input device.

## Acceptance Criteria

- A soloed channel stays audible while every other channel is silenced.
- Clearing the last solo restores every channel to exactly its previous level. No gain is lost.
- Muting a channel silences it whether or not it is soloed.
- Muting every channel is silent even with a solo set.
- An absent `mutes` or `solos` field preserves the stored flags; an oversized list is rejected.
- Pan, gains, master volume and the ceiling behave exactly as before.
- The PMON v1 wire format is unchanged.

## Acceptance Checks

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`
- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`

## Route

Direct inline for the server and protocol work in `apps/server/src/main.rs`. A separate Android
unit for the toggles.

## Delivery Forecast

Roughly 300 authored changed lines across the server, protocol and Android work. Below the
400-line budget, so the default `ask-on-risk` strategy needs no chain.

## Known Limitation

With the two-channel Volt 4 the toggles act on two sources, so solo is easy to verify but not
representative of a full band. The rule itself is what a multichannel interface needs.
