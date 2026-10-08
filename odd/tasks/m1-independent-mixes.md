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
