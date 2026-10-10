# Engineer-Defined Groups

## Objective

Let the audio engineer define named groups of source channels, and give every musician one fader
per group that moves all of its members together (PRD RF9, "con grupos (voces, batería, etc.)").

## Why

A musician mixing twelve channels one by one is not mixing, they are doing bookkeeping. The value
of a group fader is that one gesture moves the whole drum kit or all the vocals.

The definition has to come from the engineer: they are the only one who knows which input the
kick is on. A musician defining groups would be guessing at a layout they did not wire.

## Where The Engineer Defines Them

The engineer already runs the server and passes it `--device` and `--target`. Groups arrive the
same way, as a repeatable argument:

```
--group "Drums=1,2,3" --group "Vocals=4,5"
```

Channel numbers are **1-based**, because that is what the client shows as `Channel 1`, `Channel 2`.
A dedicated engineer UI is deliberately not built here: there is no configuration surface at all
yet, and the command line is honest, testable and available today.

Groups are fixed at startup, like the targets. Changing them means restarting the server, which is
already true of the destination list and is recorded here so it is not a surprise.

## Scope

- Server: parse and validate group definitions, carry them in the mix state, apply them when mixing,
  and publish them to clients.
- Protocol: the `config` message carries the groups; the `mix` message carries one level per group,
  echoed in the acknowledgement.
- Client: one fader per group, labelled with the engineer's name, alongside the per-channel controls.
- Pure logic tested first on both sides.

## Out of Scope

- Group mute and group solo. Levels first; the boolean pair can follow the same shape.
- An engineer UI. The command line is the interface for now.
- Persisting groups across restarts, groups per musician, groups defined differently per client.
- Anything that changes the PMON wire format.

## Design Decisions

**A group level multiplies its members; it does not replace them.** The effective gain of a channel
is `channel gain x its group level x master`. That is how a console works, and it means a musician
who has already balanced two drum channels keeps that balance while the group fader moves both.

**A channel belongs to at most one group.** Allowing overlap would make the effective gain depend on
the order groups are applied, which is a bug waiting to happen. The server rejects a channel listed
in two groups at startup, naming both groups.

**A channel in no group is unaffected.** It behaves exactly as it does today, so a server started
without any `--group` argument produces byte-identical output to the current build. Same
non-regression rule the gain table and the pan table were built on.

**Validation happens after the input device is selected**, because the channel count is only known
then. A group naming channel 7 on a two-channel interface is a startup error, not a silent no-op.

**Group levels live in the mix state, per client.** They are the musician's own balance, exactly like
the per-channel gains, so they need no new association machinery.

## Protocol Change

`config`, sent when a control connection opens:

```json
{ "type": "config", "source_channels": 2, "sample_rate": 48000,
  "groups": [ { "name": "Drums", "channels": [0, 1] } ] }
```

Channel indices in the protocol are **0-based**; they are 1-based only on the command line, because
the command line speaks to a human and the protocol speaks to code.

`mix`, client to server:

```json
{ "type": "mix", "volume_percent": 80, "max_level_percent": 90, "muted": false,
  "channels": [100, 40], "pans": [50, 100], "mutes": [false, true], "solos": [false, true],
  "group_levels": [80] }
```

`group_levels` follows the established merge rule: absent preserves, values are clamped to 0-100,
and a list longer than the supported maximum is rejected. It is echoed in `mix_ack`.

## Tasks

- [x] G1: Server group definitions from the command line, with validation and startup errors that name the offending group and channel, tested first.
- [x] G2: Group levels in the mix state and applied in the mixer, with the no-groups case byte-identical to today, tested first.
- [x] G3: `groups` in the `config` message and `group_levels` in `mix` and `mix_ack`, tested first, with a live WebSocket probe.
- [x] G4: Client group faders labelled from the config, sent with every mix update.
- [x] G5: Workspace and Android checks, then a device session with a real group.

## Acceptance Criteria

- A server started with no `--group` argument produces byte-identical audio to the current build.
- A group fader at 50% halves every member channel, keeping their relative balance.
- A channel in no group is untouched by any group level.
- A duplicated channel is rejected at startup with both group names in the message.
- A group naming a channel beyond the captured count is rejected after device selection.
- The client renders one fader per group, labelled with the engineer's name.
- Malformed `--group` values are rejected with a message naming the value.
- The PMON wire format is unchanged.

## Acceptance Checks

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`
- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`

## Route

Server and protocol in `apps/server/src/{cli,mix,protocol,control}.rs`; the client work is a separate
unit in the Android module.

## Delivery Forecast

Roughly 380 authored changed lines across the command line, the mix state, the protocol, the client
and the tests. Below the 400-line budget.

## Verification

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 49 tests (16 added).
- `cargo check --workspace`: passed.
- `gradle test assembleDebug`: passed, 29 Android JVM tests, 0 failures (5 added), re-run by the orchestrator as a spot check with matching counts.
- RED observed first: `cannot find struct GroupDefinition`, `cannot find value MAX_GROUPS`, `cannot find struct Group`, `no field groups on type cli::Arguments`, `cannot find function validate_groups`.

Startup validation rejects, with the offending value named:

```
channel 2 is listed in both "Drums" and "Vocals"
group "Drums" names channel 7, outside the captured range of 1..=2
--group `Drums=1,kick` has a non-numeric channel `kick`
```

Live WebSocket probe against the rebuilt release binary, started with `--group "Test=1,2"`:

| Step | Result |
| --- | --- |
| Connect | `"groups":[{"name":"Test","channels":[0,1]}]` |
| `group_levels:[80]` | echoed as `[80,100,...]` |
| A message without the field | preserved |
| 17 values | `error: group_levels accepts at most 16 values, received 17` |
| No `--group` | `"groups":[]`, and the client renders no section |

### The non-regression proof

An ungrouped channel multiplies by exactly `1.0`, and IEEE-754 guarantees `x * 1.0 == x`, so the new
`channel_gain / 100 * 1.0 * master` is bit-identical to the previous `channel_gain / 100 * master`.
A dedicated test asserts it, and all 33 pre-existing tests pass unchanged, including the ones with
exact expected samples. The PMON wire format is untouched: every new field lives in the
control-channel JSON.

### A design compromise, and it is mine

`channel_group`, the fixed server-wide membership table, rides inside `MixValues` — the per-client
value copied on every packet. It is fixed configuration and does not belong in a per-client snapshot,
and it means roughly 512 bytes copied per packet per client.

The cause was an allowed-edit-surface list that excluded `network.rs`, which is where `mix_channels`
is called and where the layout could have been passed as a parameter. The worker took the only route
left and said so. **This is the second time an over-narrow surface list in my own spec produced a
worse design**, after the hand-rolled JSON parser, and the fix is the same both times: include the
file that holds the proper solution.

It is not a correctness problem and the tests cover the behaviour, so it is recorded rather than
reopened here. Moving `channel_group` out of `MixValues` into a parameter of `mix_channels` is a
small, self-contained follow-up.

### G5

Verified on the device with the server started as
`--group "Band=1,2"`:

- The client rendered a new `Groups` section containing one fader labelled `Band`, the name the
  engineer chose, proving the whole path: argument, validation, `config`, rendering.
- The operator moved the group fader and confirmed the two member channels move together with their
  balance preserved, and that the per-channel faders keep working on top of it. That second half is
  the multiply-rather-than-replace decision, verified rather than assumed.

### Two things the session exposed that are not group bugs

**The client points at a typed IP, so changing networks breaks it silently.** Mid-session the
machine moved from Wi-Fi to Ethernet, and its address changed from `192.168.1.34` to `192.168.1.27`
while the Wi-Fi adapter kept holding the old address as a disconnected interface. The audio kept
flowing, because the server sends UDP to the phone and the phone does not care where it comes from,
while the control channel failed with `failed to connect to /192.168.1.34 (port 50001)`. Nothing in
the app said "the address changed"; it said it could not connect. This is precisely the problem PRD
RF6 (mDNS discovery, "without asking for the IP by hand") exists to remove, and it raises the cost of
leaving it unimplemented.

**A dead control channel leaves the `Channels` section empty with no explanation.** The client cannot
know the source channel count without the `config` message, because the audio packet carries the
finished stereo mix, so with no control channel there is genuinely nothing to render. That is
architectural, and it is documented in `m2-stereo-bus-pan.md`. What is not defensible is showing a
bare `Channels` heading over an empty area. The section should say that per-channel controls need the
control channel, the same way `More of me` says why it is off. Small, self-contained follow-up.

## Known Limitation

The device session can only exercise a group over the Volt 4's two channels, so the group fader will
move two members. That proves the mechanism and the protocol; it does not represent a drum kit.
