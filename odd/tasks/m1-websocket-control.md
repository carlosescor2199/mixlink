# M1 WebSocket Remote Control

## Objective

Add basic unauthenticated local/LAN WebSocket control for the M0 PMON audio stream without changing the UDP protocol or the 48 kHz output contract.

## Scope

- Add server `--control-port <port>` with default `50001`; keep UDP `--target` unchanged.
- Listen on `0.0.0.0:<control-port>` in a separate Tokio runtime/thread so CPAL capture is never blocked by control traffic.
- Accept JSON text messages of the form `{ "type": "mix", "volume_percent": 80, "max_level_percent": 90, "muted": false }`.
- Clamp volume and maximum level to 0-100 and update lock-free shared atomics.
- Apply mute, gain, and a clipping-safe ceiling to reduced PCM16 samples before UDP packet enqueueing.
- Return a JSON state acknowledgement for valid mix updates; report control errors without stopping audio.
- Add an Android OkHttp WebSocket client at `ws://<host>:<control-port>`, defaulting to control port `50001`, independent from PMON UDP.
- Connect on Start, close on Stop and `onDestroy`, send slider/checkbox changes, and preserve local PCM protection as fallback.
- Surface WebSocket control errors without hiding UDP metrics or errors.

## Protocol

Client to server:

```json
{ "type": "mix", "volume_percent": 80, "max_level_percent": 90, "muted": false }
```

Server acknowledgement:

```json
{
  "type": "mix_ack",
  "volume_percent": 80,
  "max_level_percent": 90,
  "muted": false
}
```

Invalid messages receive a JSON error response and do not change the current mix state.

## Ports and reachability

- PMON UDP target remains configured by `--target` and defaults to `127.0.0.1:50000`.
- Control WebSocket defaults to TCP `50001` and listens on all interfaces (`0.0.0.0`) for same-host/LAN clients.
- The control channel has no authentication or encryption; use it only on a trusted LAN. Do not expose it to the Internet.

## Tasks

- [x] T1: Add Rust control state, JSON parsing/acknowledgement, WebSocket runtime, CLI port, and PCM16 transformation tests.
- [x] T2: Add Android OkHttp WebSocket lifecycle, control-port input, outbound mix messages, ack/error handling, and preserve local fallback processing.
- [x] T3: Run formatting, Rust tests/checks, Android JVM tests, and `assembleDebug`; record results and environment limitations.

## Acceptance Criteria

- CPAL capture still reduces supported 192 kHz input to 48 kHz before packetization and announces 48,000 Hz.
- Audio callback reads atomics only and never waits on a mutex or WebSocket operation.
- Valid mix updates are clamped, acknowledged, and applied to PCM16 without clipping; mute outputs zero.
- Control failures do not terminate UDP audio capture.
- Android UI remains View-based; UDP parsing/metrics/audio processing stay unchanged and off the UI thread.
- Stop and `onDestroy` close both UDP and WebSocket resources.
- Existing `PRD.md` and `odd/tasks/m0-udp-pcm.md` remain unchanged.

## Verification Commands

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`
- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`

## Verification Status

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 10 tests.
- `cargo check --workspace`: passed.
- `gradle -p apps/android test`: passed with Gradle 9.6.0 from the local Gradle cache.
- `gradle -p apps/android assembleDebug`: passed; debug APK assembled.
- Editor diagnostics: no errors in the changed Rust or Kotlin files.
- The `gradle` command is not on `PATH`; verification used the cached `gradle.bat` directly. Gradle reported existing deprecation warnings for `kotlinOptions` and Gradle 10 compatibility.
- Direct WebSocket integration check: passed; PowerShell sent a `mix` command to `ws://127.0.0.1:50001` and received `mix_ack` with the clamped state. The test client reported a close-handshake warning after the ack; command delivery was successful.
- No physical Android end-to-end WebSocket/UDP audio session was run in this change.
- Android manifest enables cleartext traffic because M1 uses unauthenticated `ws://` on the trusted local LAN; this must be replaced with authenticated `wss://` before any non-LAN deployment.

## Implementation Notes

- The CPAL callback keeps the existing source-rate conversion and integer decimation path; the atomic mix state is read only after samples are reduced to 48 kHz and immediately before bounded UDP enqueueing.
- A failed control bind/runtime is logged by its own thread and does not terminate audio capture.
- The control channel listens on `0.0.0.0:50001` by default and has no authentication or encryption; it is intended only for a trusted LAN.
