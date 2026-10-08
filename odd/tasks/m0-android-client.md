# M0 Android Native Client

## Objective

Create the first native Android client for M0. The client receives PMON v1 UDP datagrams from the Rust server and plays their PCM16 little-endian stereo payload through `AudioTrack`.

## Scope

- Add a self-contained Kotlin Android application under `apps/android`.
- Support Android API 29 and newer, including Samsung S23+ class devices.
- Provide server host/IP input, port input defaulting to `50000`, and Start/Stop controls.
- Receive UDP packets off the UI thread with `DatagramSocket`.
- Validate the 20-byte PMON v1 header and exact PCM16 payload length.
- Detect sequence gaps and report received packets, lost sequences, last sample rate, and errors.
- Play compatible announced sample rates through stereo `AudioTrack`, falling back to 48 kHz when necessary.
- Keep the PMON parser in a separate JVM-testable class.

## Constraints

- Do not edit `PRD.md`.
- Do not modify `odd/tasks/m0-udp-pcm.md`.
- Use Android Views, not Compose.
- Avoid unnecessary dependencies.
- Keep technical artifacts in English.
- `INTERNET` is a normal permission and does not require runtime permission handling.

## Verification Commands

- `gradle -p apps/android test`
- `gradle -p apps/android assembleDebug`
- `adb devices`

## Environment Note

Android Studio's bundled JDK, Android SDK, Build Tools, and ADB are installed locally. Gradle 9.6.0 is available from the user Gradle cache; the repository does not yet include a Gradle wrapper.

## Tasks

- [x] T1: Create the minimal Gradle Android project and view-based client UI.
- [x] T2: Implement PMON v1 parsing, UDP reception, sequence-loss tracking, and AudioTrack playback.
- [x] T3: Add JVM parser tests and document environment-limited verification.

## Acceptance Criteria

- The host field is empty by default and never functionally defaults to localhost.
- Start validates host and port, then performs all UDP work off the UI thread.
- Stop and `onDestroy` close the socket, stop the receiver thread, and release `AudioTrack`.
- Invalid PMON packets are rejected and surfaced as errors without crashing the app.
- Sequence gaps are counted and displayed.
- The parser has JVM tests for valid and invalid packets.

## Verification Status

- Android debug build: passed with `assembleDebug`.
- APK: `apps/android/app/build/outputs/apk/debug/app-debug.apk`.
- ADB: available; the Samsung S23+ was connected during the end-to-end test and is no longer connected for this session.
- Android JVM tests: not run separately yet; the debug build passed.
- End-to-end Windows test: passed on Samsung S23+ with Volt 4 audio; 8,504 UDP packets sent and 0 discarded during 1m26s, with audible playback confirmed.
