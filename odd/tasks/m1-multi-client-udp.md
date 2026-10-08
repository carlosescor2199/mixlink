# M1 Multi-Client UDP

## Objective

Allow the existing PMON UDP capture to stream each serialized packet to multiple client destinations without changing the PMON packet protocol or the WebSocket control channel.

## Scope

- Accept repeated `--target <host:port>` arguments while preserving one target and the default `127.0.0.1:50000`.
- Resolve every configured target before starting audio capture and fail clearly when a target is invalid or resolves to no address.
- Keep CPAL callbacks independent from sockets and blocking network operations.
- Send each serialized PMON packet to every resolved destination from the UDP network thread.
- Count successful datagrams per destination in `packets sent`; count failed datagrams as discarded while continuing with other destinations.
- Print the resolved destination list at startup.
- Preserve the PMON v1 UDP packet format and the WebSocket control protocol.

## Commands

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo check --workspace`

## Current Limits

- Targets are configured at process startup; changing destinations requires restarting the server.
- UDP delivery remains best-effort and has no acknowledgements or retry queue.
- Every client must be on the same LAN as the server; the control channel and UDP stream are not intended for Internet exposure.

## Tasks

- [x] T1: Parse repeated targets, resolve and print all destinations before capture.
- [x] T2: Broadcast serialized packets from the network thread and count delivery attempts per destination.
- [x] T3: Add focused unit coverage and run formatting, tests, and workspace checks.

## Constraints

- Do not change `PRD.md` or `odd/tasks/m0-udp-pcm.md`.
- Do not change the PMON v1 wire format or the WebSocket protocol.
- Preserve existing unrelated work.

## Verification

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace`: passed, 12 tests.
- `cargo check --workspace`: passed.

## Implementation Notes

- The CPAL callback still enqueues `AudioPacket` values through the bounded channel and does not access sockets.
- The UDP worker serializes each packet once, sends the bytes to every resolved destination, and counts each successful or failed datagram independently.
- Invalid target resolution stops startup with the target value included in the error message.
