# Server Discovery

## Objective

Let the client find the server without anybody typing an IP address, and keep working when the
server's address changes (PRD RF6, "descubrimiento automático").

## Why

Measured, not theoretical. During the groups test the machine moved from Wi-Fi to Ethernet and its
address changed from `192.168.1.34` to `192.168.1.27`. The audio never noticed, because the server
sends UDP to the phone and the phone does not care where it comes from, while the control channel
died with `failed to connect to /192.168.1.34 (port 50001)`. Nothing in the app said the address had
changed; it said it could not connect, and the empty `Channels` section gave no clue why.

A typed address makes every network change a silent failure. This is the requirement that removes
it, and it costs the user real time every time it is missing.

## The Design Decision: Broadcast Beacon, Not mDNS

The PRD names mDNS. This document proposes a **UDP broadcast beacon** instead, and the deviation is
deliberate.

The server broadcasts a small datagram to the broadcast address every second. The client listens on
that port and learns two things at once: the server's **current address**, from the packet's source,
and its **control port**, from the payload.

Why not mDNS:

- Zero new dependencies on either side. mDNS means a crate on the server and `NsdManager` on the
  client, plus a service type, TXT records and a resolver lifecycle to get wrong.
- It solves the actual failure. A beacon follows the server, so an address change is picked up on
  the next beacon. mDNS also solves it, at roughly five times the machinery.
- The payload is ours. Sample rate and control port ride along, which mDNS would need TXT records
  for anyway.

What is given up: mDNS is a standard, so other tools can see the service, and it can cross subnets
with a reflector. A beacon is link-local and MixLink-specific only. Neither matters for a product
that runs on one trusted LAN, which the PRD states explicitly.

If a standard discovery protocol is wanted later for interop, it can be added beside the beacon; the
client would simply have two ways to learn the same two facts.

## Scope

- Server: a discovery thread broadcasting a versioned beacon, with the control port and sample rate
  in it, on a fixed discovery port.
- Client: listen for beacons while idle, list what it finds, and fill in the address and control
  port with one tap. Re-discovery must not fight a manual entry.
- Pure logic tested first on both sides: the beacon payload round trip and the choice of which
  discovered server to use when several answer.

## Out of Scope

- QR codes. They are the other half of RF6 and belong in their own increment; a QR is for joining
  a specific session, a beacon is for finding the LAN at all.
- mDNS, as argued above.
- Auto-connecting without confirmation. The client fills the fields and shows what it found; the
  musician still presses Start. Silently connecting to whatever answers on a shared network is not
  a thing to do to somebody.

## Design Decisions

**The beacon is a separate, tiny protocol.** It carries a magic, a version, the control port and the
sample rate. It is not PMON and not the control channel, so it cannot break either.

**A beacon is not proof the server is usable.** It says somebody is advertising. The client still
validates by connecting, and reports a failed connection the normal way.

**Manual entry stays.** A musician with a known address must not have discovery forced on them, and
a beacon that never arrives must not block anything. The middle ground: the client listens while
idle, shows what it found, and does nothing to the fields unless tapped.

**Several servers may answer.** The client lists them rather than guessing. Choosing silently between
two studios would be a bug.

## Tasks

- [x] D1: Beacon format and its round trip, plus the broadcast thread, tested first.
- [x] D2: Client listener and the "found servers" list, with the two-facts parsing tested first.
- [x] D3: One tap fills the address and control port; manual entry still works and is not overwritten.
- [ ] D4: Checks, then a device session where the server's address changes and the client recovers.

## Acceptance Criteria

- A beacon round-trips its fields, and a malformed or wrong-version beacon is ignored rather than
  crashing.
- With the server advertising, the client shows it without anyone typing an address.
- Tapping a found server fills the address and the control port and nothing else.
- A manually typed address is never overwritten while the user is editing it.
- With no beacon, the client behaves exactly as it does today.
- Changing the server's address is picked up without restarting the client.
- PMON and the control protocol are unchanged.

## Acceptance Checks

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`
- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`

## Route

Server and protocol work in a new module; the client work is a separate Android unit.

## Delivery Forecast

Roughly 350 authored changed lines across the beacon, the listener, the UI and the tests. Below the
400-line budget.

## Verification

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 54 tests (5 added).
- `cargo check --workspace`: passed, no warnings.
- `gradle test assembleDebug`: passed, 39 Android JVM tests, 0 failures (10 added), re-run by the orchestrator as a spot check with matching counts.
- RED observed first on both sides: `cannot find struct Beacon`, `cannot find value DISCOVERY_VERSION`, `Unresolved reference 'ServerBeacon'`, `Unresolved reference 'DiscoveredServers'`.

The beacon was proven without a phone, by capturing it with a `UdpClient` bound to the discovery port
while the release server ran:

```
LENGTH=11
HEX=4D 4C 4E 4B 01 51 C3 80 BB 00 00
MAGIC=MLNK VERSION=1 CONTROL_PORT=50001 SAMPLE_RATE=48000
```

**One risk to check in D4:** the capture came from `172.23.192.1`, the WSL adapter's address. With
several interfaces present, a limited broadcast may leave through the wrong one, and a beacon that
never reaches the phone's subnet is a beacon that does not work. The byte content is correct; which
interface it exits on is exactly what D4 has to answer, and the first question is whether the phone
sees it at all.

### Judgement calls left as they are

`decode_beacon` on the server is gated `#[cfg(test)]`: the server only sends beacons, the reader
exists for the round-trip proof, and leaving it in the binary produced a `dead_code` warning with no
existing convention for silencing it.

Discovery listens while idle, pauses during a session, and resumes on stop. A bind failure on the
discovery port stops discovery silently: with no beacon the screen looks exactly as it did before,
which is the stated acceptance, and surfacing it would add noise for a case the spec does not ask
about.

## Known Limitation

Broadcast is link-local. A client on a different subnet, or on a network that blocks broadcast, will
not see the beacon and must type the address, which the design keeps working on purpose. The device
session can only prove the same-subnet case.
