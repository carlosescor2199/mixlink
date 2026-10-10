# Clients Announce Themselves

## Objective

Remove the IP address from the workflow entirely. A musician opens their app, the app finds the
server, announces itself with the musician's own name, and starts receiving audio. The engineer's
desktop app only shows who is connected.

## Why

The current model has the engineer typing each musician's IP into the server. The user's words: they
do not want to type every IP in the desktop app, they want clients to connect normally and the
desktop to simply show who is connected.

It is also what the PRD asked for from the start: "configuración simple, sin pedir la IP a mano".
The manual musician list was a step in the right direction — it removed the restart — but it kept
the engineer transcribing addresses, which is work a computer should do.

## The Model

1. The client finds the server by itself. **Discovery already exists** and is the prerequisite: a
   client that cannot find the server cannot announce itself to it.
2. The client opens the control channel and **registers**: it sends its name and the UDP port it is
   listening on.
3. The server **adds that client as a target automatically**, from the registration, and starts
   streaming to it.
4. The desktop app **lists whoever is connected**. No add form, no addresses.
5. **The name comes from the mobile app**, typed by the musician, not by the engineer.

## Prerequisite: The Beacon Is Broken From The Desktop App

The beacon derives its outgoing interfaces **once at startup**. The desktop app always starts with an
empty musician list, so it derives none and falls back to the unbound `255.255.255.255` send, which
on this machine Windows routes through the **WSL adapter**. The phone therefore never sees it, and
adding a musician later does not help.

**This must be fixed first**, or step 1 above does not happen and the whole model collapses. The
beacon must derive from the interfaces that are live, not from a list that was empty when it
started.

## Design Decisions

**Registration is a control message.** A new message type on the existing WebSocket, carrying the
name and the UDP port. The shape of existing messages does not change.

**A registration is a target.** The server resolves the client's address from the socket, never from
the message, so a client cannot claim to be somebody else's address.

**`--target` stays, for a rack.** A fixed installation with static addresses should still be able to
pin them, and it is useful for debugging. It becomes the exception, not the path.

**The desktop's manual add stays as an advanced action.** Removing it would make debugging harder,
but it stops being how the product is used.

**A dropped control channel does not immediately drop the audio.** The musician's phone losing the
socket for a moment should not cut their audio. The target is kept for a grace period and only then
removed. The grace period is a named constant and its value is a decision to record, not a magic
number.

**The engineer can still label a musician locally.** The name comes from the client; the desktop may
keep a local override for the engineer's own reference. It is not sent to the client.

## Scope

- The beacon interface fix.
- A registration message and the server side that turns it into a target, with the grace period on
  disconnect.
- The Android client sending its name and UDP port on connect, with a field for the musician to type
  their name, remembered across restarts.
- The desktop app listing connected musicians without an add form as the primary path.

## Out Of Scope

- Groups in the UI. A separate ask, and this increment is already a protocol change.
- QR codes, iOS, RF12.
- Removing `--target` or the manual add, both of which stay as fallbacks.

## Tasks

- [ ] R1: Fix the beacon so it derives from live interfaces rather than a list captured at startup, tested.
- [ ] R2: The registration message, the server turning it into a target, and the grace period on disconnect, tested.
- [ ] R3: The Android client sending its name and UDP port, with a name field remembered across restarts.
- [ ] R4: The desktop app showing connected musicians, with the add form demoted to an advanced action.
- [ ] R5: Checks, then a device session where the phone announces itself with nothing typed anywhere.

## Acceptance Criteria

- With nothing typed in either app, a phone finds the server, announces itself, and receives audio.
- The musician's name, typed on the phone, appears in the desktop app.
- A client cannot register an address other than its own socket's.
- A brief control-channel drop does not cut the audio; a longer one removes the target after the
  named grace period.
- `--target` and the manual add still work.
- The PMON wire format and the shapes of existing control messages are unchanged.

## Acceptance Checks

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`
- `gradle -p apps/android test` and `assembleDebug`
- The desktop app builds and runs against the real device

## Route

Server and protocol work in `apps/server`, the client in the Android module, the UI in
`apps/desktop`. One writer, in `F:\Projects\mixlink-main`.

## Delivery Forecast

Not forecast. This changes a protocol and touches all three surfaces; a number here would be a guess
until the registration shape is settled in code.

## Known Limitation

Registration assumes the client can reach the server's control port. A network that blocks the
control channel but allows the audio path leaves the client unable to announce itself, and it must
then fall back to a typed address, which is why that path stays.
