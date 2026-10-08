# M0/M1 Android Client Controls

## Objective

Add local playback protections and receiver metrics to the native Android PMON client without changing the PMON protocol or introducing remote control.

## Scope

- Add local volume and maximum-level sliders from 0% to 100%.
- Add a local mute toggle.
- Apply mute, volume, and the hard maximum-level ceiling to PCM16 samples before writing to `AudioTrack`.
- Display received packets, lost sequences, last sample rate, approximate packets per second, and monotonic-clock inter-arrival/jitter estimates in milliseconds.
- Keep UDP reception, metric calculation, PCM transformation, and audio writes off the UI thread.
- Preserve the PMON parser, Start/Stop behavior, and `onDestroy` cleanup.

## Constraints

- Do not edit `PRD.md`.
- Do not modify `odd/tasks/m0-udp-pcm.md`.
- Do not add WebSocket support or change the PMON protocol.
- Use Android Views and avoid unnecessary dependencies.
- Keep technical artifacts in English.

## Tasks

- [x] T1: Add local playback controls and a testable PCM16 transformation with mute, volume, hard limiting, and clipping-safe conversion.
- [x] T2: Add monotonic receiver metrics and display them without moving network or audio work onto the UI thread.
- [x] T3: Run the available Android and workspace checks and record their results.
- [x] T4: Reduce live-monitoring playback latency with low-latency AudioTrack configuration and non-blocking writes that expose discarded audio.
- [x] T5: Select 48 kHz input capture when the device supports it, preferring stereo without changing PMON.
- [x] T6: Accept 96/192 kHz input and reduce it to 48 kHz before PMON packetization.

## Acceptance Criteria

- Volume and maximum-level controls are bounded to 0-100%; mute forces output samples to zero.
- The maximum-level control is a hard ceiling applied before PCM16 conversion, including for full-scale input samples.
- Packet metrics update from the receiver thread and are rendered on the UI thread only.
- Live playback uses stereo PCM16, low-latency AudioTrack performance, and non-blocking writes; short writes are counted as discarded samples and packets.
- Stop and `onDestroy` continue to close the socket and release `AudioTrack`.
- The PMON parser and wire format are unchanged.

## Verification Commands

- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`
- `cargo test --workspace`

## T4 Hypothesis and Tradeoff

The blocking `AudioTrack.write()` call can wait for playback buffer space while UDP reception continues to deliver packets, allowing roughly half a second of audio to accumulate. Using `PERFORMANCE_MODE_LOW_LATENCY` with a monitoring-oriented usage and `WRITE_NON_BLOCKING` should keep the receiver close to real time by dropping audio when the device queue is full.

The tradeoff is lower latency at the cost of possible underruns, short writes, and audible cuts. The client now reports discarded samples and packets with discarded samples so that this tradeoff is observable without changing the PMON protocol.

## T4 Verification Status

- Commands: `gradle -p apps/android test`; `gradle -p apps/android assembleDebug`; `cargo test --workspace`; `adb devices`.
- Result: passed using Gradle 9.6.0 from the local Gradle cache; Android JVM tests passed, `assembleDebug` passed, and `cargo test --workspace` passed with 3 tests. ADB found one connected Samsung device. The end-to-end PMON stream and audible latency comparison were not rerun after this change.

## T5 Root Cause and Change

The server selected `default_input_config()` unconditionally. The Volt 4 reported that default as 192000 Hz, while the Android client plays PMON PCM16 at 48000 Hz; this fourfold rate mismatch increased live-monitoring latency. The server now enumerates `supported_input_configs()`, selects a range containing 48000 Hz when available, otherwise selects a supported 96/192 kHz source configuration with stereo preference, and decimates complete PCM frames by the integer ratio before packetization. The PMON header always announces 48000 Hz and the wire format is unchanged.

## T5 Verification Status

- Unit coverage: sample-rate range selection accepts inclusive 48000 Hz ranges; pure PCM tests cover 192 kHz to 48 kHz decimation, unchanged 48 kHz, and rejection of invalid ratios.
- Required workspace checks: `cargo fmt --all -- --check` passed; `cargo test --workspace` passed with 8 tests; `cargo check --workspace` passed.

## Limitation

These are local playback protections only. Remote engineer control is intentionally deferred to a future protocol; this increment does not implement WebSocket or any remote control message.

## Verification Status

- Editor diagnostics: no errors in the changed Kotlin or layout files.
- `cargo fmt --all -- --check`: passed.
- `cargo check --workspace`: passed.
- `cargo test --workspace`: passed, 3 tests.
- Android JVM tests: passed, 5 tests.
- Android debug build: passed with Gradle 9.6.0 and the Android Studio bundled JDK.
- APK installed successfully on the connected Samsung S23+ with `adb install -r`.
- End-to-end control/metering test: passed on Samsung S23+ with Volt 4 audio; 6,561 UDP packets sent and 0 discarded during 1m05s, with playback confirmed.
