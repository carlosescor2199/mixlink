# M0 UDP PCM Capture

## Objective

Connect the existing CPAL input capture to a non-blocking UDP sender for the M0 proof of concept.

## Scope

- Preserve `--device <text>` selection.
- Add `--target <host:port>` with default `127.0.0.1:50000`.
- Convert F32, I16, and U16 input samples to PCM16 little-endian.
- Send one documented binary packet per audio callback through a bounded channel and a network thread.
- Report sent and discarded packets after Ctrl+C shutdown.

## Constraints

- Do not edit `PRD.md`.
- Do not add Tauri or mobile client code.
- Keep technical artifacts in English.

## Tasks

- [x] T1: Extend argument parsing and define the packet format.
- [x] T2: Connect CPAL callbacks to a bounded channel and UDP worker.
- [x] T3: Add focused unit tests and run formatting, tests, and workspace checks.

## Acceptance checks

- `cargo fmt --all`
- `cargo test --workspace`
- `cargo check --workspace`

## Verification

- Focused server tests: 3 passed.
- Workspace tests: 3 passed.
- Workspace check: passed.
- Formatting: `cargo fmt --all` passed.

## Route

- Direct inline: the implementation is localized to the existing server entry point and one task document.
