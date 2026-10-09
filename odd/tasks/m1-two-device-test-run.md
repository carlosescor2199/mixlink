# M1 Two-Device Test Run

## Objective

Validate the M1 milestone end to end with two physical Android devices on the same LAN:
each device receives the PMON audio stream and controls its own independent mix.

This document is a runbook. It records the exact commands and the observations to capture,
so the result can be written back into `m1-independent-mixes.md` as verification evidence.

## Current Status

Not yet executed. No two-device run has been recorded for M1. All previously recorded
end-to-end evidence is single-device (Samsung S23+ with the Volt 4).

The double gain defect in the client is fixed; see "Resolved Defect - Double Gain Application".
The APK must be rebuilt and reinstalled before this run, because the fix is client-side.

## Artifacts

- Server binary: `target/release/personal-monitoring.exe` (workspace root target directory).
- Android APK: `apps/android/app/build/outputs/apk/debug/app-debug.apk`.

## Prerequisites

- One audio interface connected to the server machine (Volt 4 used so far).
- Two Android devices (API 29+) on the **same subnet** as the server.
- Windows Firewall allows inbound TCP on the control port (default `50001`).
  The first run may prompt; a blocked control port produces a client-side WebSocket error
  while UDP audio still plays, which is a misleading partial failure.
- Router AP/client isolation is disabled. With isolation enabled, phone-to-PC traffic
  is dropped and the UDP stream never arrives.

## Step 1 - Collect the two device IPs

On each phone: Settings -> About phone -> Status -> IP address.

Write them down as `<phone1-ip>` and `<phone2-ip>`. Both are needed **before** the server starts.

## Step 2 - Start the server

From the repository root:

```powershell
.\target\release\personal-monitoring.exe `
  --device Volt `
  --target <phone1-ip>:50000 `
  --target <phone2-ip>:50000 `
  --control-port 50001
```

Expected startup output:

```
UDP targets:
  <phone1-ip>:50000
  <phone2-ip>:50000
Available input devices:
  ...
Selected input: ...
Capture format: channels=..., sample rate=..., ...
```

Pass criteria:

- [ ] Both phone IPs are listed under `UDP targets`.
- [ ] The selected input is the intended interface.
- [ ] The announced capture sample rate is reduced to 48000 Hz (the client plays 48 kHz).

Startup errors and their meaning:

- `duplicate UDP target IP ... is not supported` - the same IP was passed twice. One IP maps to
  one independent mix, so distinct IPs are required.
- `invalid --target ...` - the value did not resolve. Re-check the phone IP.

Note: targets are fixed at process start. If a phone changes IP, restart the server.

## Step 3 - Install and start each client

```powershell
adb install -r apps\android\app\build\outputs\apk\debug\app-debug.apk
```

On **both** devices, in the app:

- Host: the **server** LAN IP (not the phone IP).
- Port: `50000`.
- Control WebSocket port: `50001`.
- Tap **Start**.

Pass criteria, per device:

- [ ] `Status: Receiving`.
- [ ] `Packets received` increases steadily.
- [ ] `Sequences lost` stays at or near 0.
- [ ] `Last sample rate: 48000 Hz`.
- [ ] `Errors: none` and `Control: none`.
- [ ] Audio is audible.

## Step 4 - Establish the baseline

Set **both** devices to `Volume 100%`, `Maximum level 100%`, `Mute` off.

With the control channel connected, the server owns the gain for each client IP and the client
applies no local processing. The sliders are remote controls: their values travel over the
WebSocket and are applied per client IP on the server. A slider at `50%` should now sound like
roughly half level (about `-6 dB`), not a quarter.

Confirm on each device that the control channel is up: the `Control:` line shows no error. If it
shows a WebSocket error, that device fell back to local processing and the level comparison in
Step 5 is not valid for absolute gain.

## Step 5 - Prove independence

With both devices streaming:

1. On device A only, drag `Volume` down.
   - [ ] A becomes quieter.
   - [ ] B does **not** change.
2. On device A only, toggle `Mute`.
   - [ ] A goes silent.
   - [ ] B keeps playing.
3. On device A only, lower `Maximum level`.
   - [ ] A's peaks are limited.
   - [ ] B does **not** change.
4. Reverse the roles (control from B, observe A).

Independence is proven when a control change on one device never alters the other device's audio.

## Step 6 - Capture the evidence

On each device, before stopping, write down from the stats panel:

- Packets received
- Sequences lost
- Packets per second
- Inter-arrival (ms)
- Jitter (ms)
- Audio samples discarded
- Audio packets with discarded samples
- Final `Errors:` / `Control:` line

Stop the server with `Ctrl+C`. It prints the aggregate counters. Record:

- Samples seen
- Packets sent
- Packets discarded

## Acceptance Criteria for the Run

- [ ] Two devices stream PMON audio simultaneously from one server.
- [ ] Each device controls only its own mix; the other device is unaffected.
- [ ] `Volume 50%` sounds like roughly half level (about `-6 dB`), confirming the gain is applied once.
- [ ] No control errors; WebSocket connects on both devices.
- [ ] Packet loss stays at or below the PRD target of 0.5%.
- [ ] Results are written back into `m1-independent-mixes.md` as verification evidence.

## Resolved Defect - Double Gain Application

The client applied `Pcm16Processor.apply` to every received packet using the local slider values,
while the same values were also sent to the server, which applied the equivalent scaling per
client IP (`apply_mix` in `apps/server/src/main.rs`). Both sides computed
`sample * volume_percent / 100` clamped to a `max_level_percent` ceiling, so in series the volume
was squared: `50%` produced about `25%` (`-12 dB` instead of `-6 dB`).

Fix: local processing is now the fallback path only.

- `Pcm16Processor.applyLocalProtection` decides between pass-through and local processing.
- `MainActivity` passes `remoteControlActive = controlWebSocket != null`, so while the control
  channel is connected the client emits the received samples unchanged and the server owns the mix.
- When no control channel is available, the client still applies the local sliders as protection.

Verification:

- Test-first: the two new tests were observed failing with `Unresolved reference
  'applyLocalProtection'` before the function existed.
- `gradle -p apps/android testDebugUnitTest`: passed, `Pcm16ProcessorTest` 4 tests, 0 failures.
- `gradle -p apps/android test assembleDebug`: passed, APK rebuilt.
- `cargo fmt --all -- --check`, `cargo test --workspace` (15 passed), `cargo check --workspace`: passed.

## Remaining Edge Case - Control Channel Outage

The server keeps the last received `MixState` for a client IP when the WebSocket disconnects;
it does not reset on disconnect. After the client falls back to local processing, the last
server-side values are still applied, which reintroduces the squared gain for the duration of
the outage.

Resetting the server state to neutral on disconnect would close this, but it would also drop an
engineer-set `Maximum level` ceiling at exactly the moment the client loses its control channel,
which works against RF12. This is a product decision and is intentionally not implemented.

Impact on this run: keep both devices connected for the whole session and note any WebSocket
error in the evidence. An outage invalidates the absolute-level observations taken during it.

## Constraints

- Do not change the PMON v1 wire format for this run.
- Keep technical artifacts in English.

## Run Log

First run executed 2026-10-08. Result: passed.

Environment:

- Server `192.168.1.34` (Wi-Fi), Windows. Firewall: inbound TCP 50001 allowed on the Public
  profile for the server executable, otherwise the control channel is blocked while UDP audio
  still plays.
- Devices: `SM-S916B` at `192.168.1.10`, `SM-A546E` at `192.168.1.85`.
- APK rebuilt with the double gain fix (commit `0835789`) and reinstalled on both devices
  before the run.

Outcome: passed. Closed 2026-10-08 with the gaps below accepted.

- Both devices received PMON audio from one server at 48000 Hz.
- `Sequences lost: 0` over 6853 packets on the SM-S916B, jitter 1.1 ms.
- Independence confirmed: a control change on one device did not affect the other.
- 4416 audio samples discarded across 5 packets, the expected consequence of the
  low-latency non-blocking write path.
- Both control channels confirmed server-side; the connection log records
  `192.168.1.10:59268` and `192.168.1.85:54428`.

Accepted gaps:

- SM-A546E client-side metrics were not captured; the device left USB/adb before the
  screenshot. Its control channel is evidenced by the server connection log.
- Server aggregate counters at shutdown were not captured. The server was stopped without a
  clean Ctrl+C, so no summary was printed.
- The client provides no positive control-channel indicator; server-side evidence was needed
  to confirm the WebSocket. See `m1-independent-mixes.md`, "Verification Status".

Detailed evidence is recorded in `odd/tasks/m1-independent-mixes.md`.
