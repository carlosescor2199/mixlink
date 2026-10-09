# M1 Independent Client Mixes

## Objective

Apply volume, mute, and ceiling controls independently for each configured UDP client while preserving the PMON UDP and WebSocket JSON protocols.

## Scope

- Build one `Arc<HashMap<IpAddr, Arc<MixState>>>` from resolved UDP targets.
- Associate each WebSocket connection with `peer.ip()` and update only that IP's `MixState`.
- Return `no UDP target configured for client IP` for WebSocket peers without a configured target, without changing state.
- Keep CPAL capture and decimation limited to raw PCM16 packet production at 48 kHz.
- Clone, mix, serialize, and send each target's datagram in the UDP worker thread.
- Preserve packet sequence, sample rate, 192/48 kHz selection, repeated targets by distinct IP, and control port `50001`.

## Association and Limitation

The WebSocket client is associated by its TCP peer IP address. A resolved UDP target IP may occur only once: multiple targets for the same IP are rejected clearly at startup because one IP cannot unambiguously select one independent `MixState`. Different ports on the same IP are therefore unsupported.

## Tasks

- [x] T1: Build per-IP mix state, reject duplicate target IPs, and route WebSocket controls by peer IP.
- [x] T2: Move mixing out of the CPAL callback and apply each target's atomic state in the UDP worker.
- [x] T3: Add pure tests for independent PCM output and client-state isolation; run Rust verification.

## Verification

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`

## Constraints

- Do not change the PMON v1 UDP wire format or existing WebSocket JSON shapes.
- Do not edit `PRD.md`, `odd/tasks/m0-udp-pcm.md`, or Android protocol code.
- Keep network operations and sample cloning outside the CPAL callback.

## Verification Status

Workspace checks:

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 15 tests.
- `cargo check --workspace`: passed.

Two-device end-to-end run (2026-10-08), server on Windows:

- Capture: `INPUT 1/2 (Volt 4)`, native 48000 Hz, 2 channels, I16, no decimation applied.
- Targets: `192.168.1.10:50000` (SM-S916B) and `192.168.1.85:50000` (SM-A546E).
- The server listed both targets and handled control connections from both client IPs
  (`192.168.1.10:59268` and `192.168.1.85:54428`), confirming that both devices had an
  established control channel during the run. An independent `Get-NetTCPConnection` check
  showed an `Established` socket on TCP 50001 from `192.168.1.10`.
- SM-S916B client: `Status: Receiving`, 6853 packets received, 0 sequences lost, 48000 Hz,
  100.0 packets per second, inter-arrival 9.9 ms, jitter 1.1 ms, 4416 audio samples discarded
  across 5 packets, no errors.
- Independence: a control change on one device did not alter the other device's audio
  (observed by the operator during the run).

Not captured:

- SM-A546E client-side metrics. The device left USB/adb before the metrics screenshot, so only
  the operator observation is recorded for that device. Its control channel is still evidenced
  server-side by the connection log above.
- Server aggregate counters at shutdown (packets sent and discarded).
- Per-client packet loss as a percentage over the full session; only the client
  `Sequences lost` counter above is recorded, which is 0 over 6853 packets.

Gap found during the run, now closed:

- The client originally gave no positive confirmation that the control channel was connected.
  The `Control:` line only rendered after `showControlError`, so a healthy session looked
  identical to one whose WebSocket never connected, and server-side evidence was needed to
  confirm the connection. The client now reports the control channel state directly:
  `Control: connected - mix applied on the server` while the channel is up, and
  `Control: unavailable - mixing on this device` when it is not.
