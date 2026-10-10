# Server UI (Tauri)

## Objective

Give the engineer a real interface for the server instead of command-line arguments (PRD section 5,
"Servidor (Rust + Tauri)").

## Why

The engineer currently types `--device`, `--target` and `--group` by hand, and every one of them is
**fixed at startup**. Adding a musician means restarting the server and dropping everybody's audio.
Defining a group means getting the syntax right with no feedback, and the only confirmation that it
worked is that the process did not exit.

The PRD put Tauri in the plan from the start. The modular split of the server is what makes it
tractable now: each concern already has a home and a seam.

## The Real Problem: Everything Is Startup-Only

This is the decision that shapes the increment, and it is not about the GUI.

Today the engine builds its world once and then runs: the capture stream is opened for one device,
the target list becomes a mix-state map, and the group layout is validated and frozen. A UI that can
only display that is a dashboard. A UI that can **change** it needs the engine to support
reconfiguration while audio is running.

That is the difference between showing the engineer a picture and letting them do their job. Adding a
musician to a live session without dropping the others is the reason to build this at all.

So the work splits cleanly:

- Making the engine a **library** the UI can drive.
- Making the engine **reconfigurable**, which is a real change to how it holds its state.

## Phasing

**Increment 1: engine as a library, and a read-only window.**

Extract the engine behind a programmatic API: start and stop, and query the device, the capture
format, the targets, the connected clients and their counters. The UI shows that, live.

This looks unambitious and is not. It proves the two things everything else depends on: that the
engine can be driven without a `main`, and that runtime state can reach a UI. It also produces a
genuinely useful tool on day one, because "is anybody connected and how bad is the packet loss" is
today answered by reading console output.

The CLI stays and keeps working unchanged. A rack machine runs headless, and a UI-only server would
be a regression for exactly the deployment the product wants.

**Increment 2: editing targets and groups, without restarting.**

Add and remove musicians and groups while audio runs, which requires the engine to own a mutable
configuration with defined semantics for what happens to a client already streaming.

**Increment 3: device and buffer selection, meters, and session presets.**

Changing the input device means tearing down the capture stream and rebuilding it. That is not hard
but it is disruptive, and it deserves its own increment with its own care.

## Scope Of This Document

Increments 1 and 2 in outline, and increment 1 in detail enough to plan. Increment 3 is named only so
the shape of the whole is visible.

## Design Decisions To Settle

**Where the engine lives.** `apps/server` becomes a library with a thin binary, and the Tauri app is
a separate crate that depends on it. The alternative, moving the engine into the Tauri crate, would
tie the headless deployment to the UI stack.

**How the UI learns about runtime state.** The engine must not know about Tauri. It exposes its own
observation API, and the Tauri layer translates it. That keeps the engine testable without a window,
which matters because none of the audio path has UI coverage today and should not gain a dependency
on one.

**What a "client" is in the UI.** Today a target is an IP with counters, and a WebSocket peer is a
separate thing associated by IP. The UI will have to present one row per musician, which means the
engine should describe that relationship rather than have the UI infer it from two lists.

**Restart semantics.** Increment 2 makes configuration editable, and the first question is what
happens to a musician whose target is removed mid-session: their stream stops and their controls are
orphaned. That has to be a decision with a written answer, not whatever falls out of the code.

**What the engineer cannot do.** RF12, the volume ceiling, is still unimplemented on the engine side
even though the mix state has the field. The UI is where it becomes real, and it should stay out of
increment 1 rather than be half-built in a status view.

## Out Of Scope

- iOS.
- QR codes.
- Moving the audio engine's behaviour. This increment changes how the engine is **driven**, not what
  it does. Any change to the mixing, the wire format or the control protocol is a separate increment.

## Tasks

- [ ] T1: Turn `apps/server` into a library plus a thin binary, keeping the CLI behaviour identical and every test passing.
- [ ] T2: Define the engine's programmatic API and its observation API, with the relationship between a target and a control peer expressed by the engine.
- [ ] T3: Scaffold the Tauri app and prove one value travels end to end, from the engine to a window.
- [ ] T4: Build the status view: device, capture format, targets, clients, counters, errors.
- [ ] T5: Checks and a run against the real hardware, with audio flowing and the window open.

## Acceptance Criteria

- The CLI binary behaves exactly as it does today; every existing test passes unchanged.
- The engine can be started and stopped programmatically, and reports its state, with no dependency on Tauri.
- The window shows the live device, format, targets and per-client counters while audio plays.
- An error in the engine surfaces in the window rather than only on a console nobody is watching.
- Adding the UI does not change the audio path, the wire format or the control protocol.

## Acceptance Checks

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`
- `gradle -p apps/android test` and `assembleDebug`, as a guard that nothing leaked
- The Tauri app builds and runs against the real device

## Route

Server-side work in `apps/server`, the new crate alongside it. Android is untouched.

## Delivery Forecast

Not forecast yet. Increment 1 alone is a library extraction plus a new app scaffold, and its size
depends on answers this document asks for rather than assumes. A number here would be a guess
dressed as a plan.

## Known Limitation

Tauri adds a dependency on the UI stack for the build of the desktop app, which the headless server
must not inherit. That is the reason the engine becomes a library rather than moving into the Tauri
crate, and it is worth restating when someone later suggests the simpler layout.
