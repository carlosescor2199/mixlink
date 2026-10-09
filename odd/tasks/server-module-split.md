# Server Module Split

## Objective

Split `apps/server/src/main.rs` from a single 1334-line file into focused modules, without
changing any behaviour, so future increments land in a file that answers one question.

## Why

One file currently holds 46 top-level items across six unrelated domains: CLI parsing, CPAL
capture with decimation, mix maths, the PMON and JSON protocols, the UDP fan-out, and the
WebSocket control server. Reading or changing the pan law means scrolling past the code that
opens the audio stream.

This is not a style preference. The cost is already visible and it compounds: groups, saved
mixes and engineer configuration are all queued, and each one has to be threaded through the
same file. Every future change gets more expensive until the seams exist.

## Target Layout

```
src/
  main.rs        entry point and wiring only
  cli.rs         Arguments, parse_arguments, resolve_targets
  capture.rs     device selection, stream building, Decimator, sample conversion
  mix.rs         MixState, MixValues, pan law, mix_channels, merge helpers
  protocol.rs    AudioPacket and its serialization, control message types and parsing
  network.rs     the UDP fan-out thread
  control.rs     the WebSocket control server
```

The boundary rule: `mix.rs` must not know a socket exists, and `capture.rs` must not know JSON
exists. A module that needs something from another domain takes it as a parameter.

## The Rule That Makes This Safe

**Pure move. No behaviour change, no signature changes, no refactors smuggled in.**

The 26 existing tests are the safety net. They must be green before the split and green after it,
with the same count. If a test has to change to make the split work, the split is wrong.

Two things are explicitly NOT part of this task, even if they look tempting while moving code:

- Renaming anything. Names move as they are.
- Changing logic, error messages, or the wire format.

## Where Tests Go

The single test module is split too, so a test lives next to what it tests:

- mix maths, pan law, merge rules, and the mix-state tests go with `mix.rs`
- control message parsing and acknowledgement tests go with `protocol.rs`
- argument parsing and target resolution tests go with `cli.rs`
- decimation and sample conversion tests go with `capture.rs`

Tests may move between modules, but no assertion may change.

## Tasks

- [x] T1: Create `mix.rs` and move the mix state, pan law, mixer and merge helpers with their tests.
- [x] T2: Create `protocol.rs` and move the PMON packet, its serialization, the control message types and parsing with their tests.
- [x] T3: Create `capture.rs`, `network.rs`, `control.rs` and `cli.rs`, moving each domain with its tests.
- [x] T4: Reduce `main.rs` to wiring, then run formatting, the full test suite and the workspace check.

## Verification

Baseline before the split, and after:

| | Before | After |
| --- | --- | --- |
| `cargo test --workspace` | 26 passed | 26 passed, 0 failed |
| `#[test]` occurrences | 26 | 26 |
| `assert` occurrences | 49 | 49 |
| `cargo fmt --all -- --check` | passed | passed |
| `cargo check --workspace` | passed | passed, no warnings |

The test and assertion counts are the evidence that the move was faithful: equal counts mean no
test was lost, duplicated or rewritten to fit the new layout.

Module sizes after the split:

| File | Lines |
| --- | --- |
| `mix.rs` | 396 |
| `capture.rs` | 361 |
| `cli.rs` | 174 |
| `protocol.rs` | 154 |
| `main.rs` | 143 |
| `control.rs` | 130 |
| `network.rs` | 54 |

### Boundary check

The spec only required `mix.rs` not to know a socket exists. The first pass left a **two-way
dependency**: `mix.rs` imported `MixCommand` while `protocol.rs` imported `MixValues` for the
acknowledgement. That meant the domain module knew the wire format, which is the same class of
problem the split exists to remove.

Fixed before committing by moving `parse_mix_command` into `protocol.rs`. The mapping between wire
and domain now lives with the wire format, and the dependency runs one way only:

```
protocol.rs -> mix.rs      (one way)

mix.rs now has no `use crate::` line at all
```

The remaining edges are `network.rs -> mix, protocol`, `capture.rs -> network, protocol` and
`control.rs -> capture, mix, protocol`, all one way, with `main.rs` doing the wiring.

### Deliberately left alone

The writer reported two clippy candidates it did not touch, because the task forbade smuggling
changes into a move: `best_config.as_ref().map_or(true, ...)` and a manual `div_ceil`. Neither was
fixed here.

## Acceptance Criteria

- `cargo test --workspace` passes with 26 tests and 0 failures, the same count as before.
- `cargo fmt --all -- --check` and `cargo check --workspace` pass.
- No item is duplicated: every function, struct and constant exists in exactly one module.
- `main.rs` contains only the entry point and the wiring between domains.
- No behaviour, signature, error message or wire format changed.

## Acceptance Checks

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`

## Route

Delegated: one bounded writer. The task is mechanical and is verified by tests that already
exist, which makes it a good fit for a worker with fresh context.

## Delivery Forecast

Roughly 1400 moved lines across seven files. Almost all of it is movement, so the reviewable
question is whether each module has a coherent boundary, not line count.

## Known Limitation

`apps/android/.../MainActivity.kt` is around 430 lines and has the same problem forming. It is
deliberately not addressed here; mixing a Rust module split with an Android refactor would make
both harder to review.
