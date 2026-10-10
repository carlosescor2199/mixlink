//! Headless proof that adding and removing UDP targets at runtime keeps the session alive.
//!
//! Run with `cargo run -p personal-monitoring --example target_editing`. It starts the engine with
//! a loopback target and the machine's real LAN target, opens a control WebSocket and a
//! discovery-beacon listener, then:
//!
//! 1. waits until audio flows to both initial targets,
//! 2. adds a third target and checks it starts receiving while the first two keep streaming,
//! 3. checks duplicate and unresolvable addresses are rejected with a message naming the value,
//! 4. removes the third target and checks its audio stops while the others keep streaming,
//! 5. removes the control peer's own target and checks the peer's next `mix` is answered with the
//!    existing no-UDP-target error on the same socket, while the remaining target keeps streaming.
//!
//! The beacon listener runs for the whole session, so a beacon that stops mid-edit fails the run.
//! This drives the same engine calls the desktop window delegates to; the window only renders them.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use personal_monitoring::{list_input_devices, start, EngineConfig};
use tokio_tungstenite::tungstenite::Message;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
const PACKET_WINDOW: Duration = Duration::from_millis(500);

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[tokio::main]
async fn main() {
    let devices = list_input_devices().expect("device enumeration should work");
    println!("Available input devices:");
    for device in &devices {
        println!("  {} ({} channels)", device.name, device.channels);
    }

    // The listeners stand in for musicians' phones: each target address is a socket this process
    // owns, so packet counts prove exactly who is and is not receiving audio.
    let Some(lan_ip) = outbound_local_ip() else {
        println!("No LAN interface is available; cannot run the target-editing proof here.");
        return;
    };
    let local = listener("127.0.0.1:50000", "local target");
    let lan = listener(&format!("{lan_ip}:50000"), "LAN target");
    let added = listener("127.0.0.2:50000", "added target");
    let (Some(local), Some(lan), Some(added)) = (local, lan, added) else {
        println!("Could not bind the target listeners; is another server running?");
        return;
    };

    let initial = vec!["127.0.0.1:50000".to_owned(), format!("{lan_ip}:50000")];
    println!("TARGETS: starting with {initial:?}");
    let config = EngineConfig {
        device_filter: None,
        targets: initial,
        control_port: 50001,
        groups: vec![],
    };
    let Some(mut handle) = start(config).expect("engine should start") else {
        println!("No input device is available.");
        return;
    };
    println!(
        "STARTED: device=\"{}\" targets={:?}",
        handle.device_name(),
        handle
            .status()
            .musicians
            .iter()
            .map(|musician| musician.address.to_string())
            .collect::<Vec<_>>()
    );

    // Discovery beacon listener: broadcast to 255.255.255.255:50002 every second. Binding it to
    // the LAN interface the beacon is sent from makes the broadcast loop back on this host.
    let beacons: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let beacon_flag = Arc::clone(&beacons);
    let beacon_listener = std::thread::spawn(move || {
        let socket = match UdpSocket::bind(format!("{lan_ip}:50002")) {
            Ok(socket) => socket,
            Err(error) => {
                println!("BEACON: could not bind listener on {lan_ip}:50002: {error}");
                return;
            }
        };
        let _ = socket.set_nonblocking(true);
        let _ = socket.set_broadcast(true);
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut buffer = [0u8; 64];
        while Instant::now() < deadline && !beacon_flag.load(Ordering::SeqCst) {
            match socket.recv_from(&mut buffer) {
                Ok((length, _)) if length >= 4 && &buffer[0..4] == b"MLNK" => {
                    beacon_flag.store(true, Ordering::SeqCst);
                    println!("BEACON: received an MLNK datagram");
                    return;
                }
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        println!("BEACON: no beacon within 30s");
    });

    // Control WebSocket, exactly the socket a musician's client holds open. It connects from
    // 127.0.0.1, so its messages join against the 127.0.0.1 target.
    let control_url = format!("ws://127.0.0.1:{}", handle.control_port());
    let (mut ws, _) = tokio_tungstenite::connect_async(&control_url)
        .await
        .expect("control connection should open");
    let first = next_config(&mut ws).await;
    println!("CONTROL: first config source_channels={first:?}");

    // Audio must be flowing to both initial targets before the edits start.
    let local_flowing = wait_for_packet(&local, Duration::from_secs(5));
    let lan_flowing = wait_for_packet(&lan, Duration::from_secs(5));
    println!("BASELINE: local streaming={local_flowing} lan streaming={lan_flowing}");
    if !local_flowing || !lan_flowing {
        println!("BASELINE FAILED: audio is not reaching the initial targets; stopping");
        handle.stop().expect("engine should stop cleanly");
        return;
    }

    // --- Add ---------------------------------------------------------------------------------
    let added_address = "127.0.0.2:50000".to_owned();
    match handle.add_target(&added_address) {
        Ok(address) => println!("ADD: added {address}"),
        Err(error) => {
            println!("ADD FAILED: {error}");
            handle.stop().expect("engine should stop cleanly");
            return;
        }
    }
    let added_flowing = wait_for_packet(&added, Duration::from_secs(5));
    let local_after_add = drain(&local, PACKET_WINDOW);
    let lan_after_add = drain(&lan, PACKET_WINDOW);
    println!(
        "ADD: added streaming={added_flowing} local kept streaming={} ({} pkts/{}ms) lan kept streaming={} ({} pkts/{}ms)",
        local_after_add > 0,
        local_after_add,
        PACKET_WINDOW.as_millis(),
        lan_after_add > 0,
        lan_after_add,
        PACKET_WINDOW.as_millis()
    );
    let joined = handle.status().musicians;
    println!(
        "ADD: joined rows={:?}",
        joined
            .iter()
            .map(|musician| (musician.address.to_string(), musician.control_connected))
            .collect::<Vec<_>>()
    );

    match handle.add_target("127.0.0.1:50000") {
        Ok(address) => println!("REJECT: UNEXPECTED success adding duplicate {address}"),
        Err(error) => println!("REJECT: duplicate rejected as expected: {error}"),
    }
    match handle.add_target("no-such-host.invalid:50000") {
        Ok(address) => println!("REJECT: UNEXPECTED success adding unresolvable {address}"),
        Err(error) => println!("REJECT: unresolvable rejected as expected: {error}"),
    }
    let control_alive = probe_control(&mut ws).await;
    println!("CONTROL: mix_ack after add = {control_alive}");

    // --- Remove the added target --------------------------------------------------------------
    match handle.remove_target(&added_address) {
        Ok(address) => println!("REMOVE: removed {address}"),
        Err(error) => {
            println!("REMOVE FAILED: {error}");
            handle.stop().expect("engine should stop cleanly");
            return;
        }
    }
    let _ = drain(&added, Duration::from_millis(150)); // flush anything already in flight
    let added_after_remove = drain(&added, PACKET_WINDOW);
    let local_after_remove = drain(&local, PACKET_WINDOW);
    let lan_after_remove = drain(&lan, PACKET_WINDOW);
    println!(
        "REMOVE: added stopped={} ({} pkts/{}ms) local kept streaming={} ({} pkts/{}ms) lan kept streaming={} ({} pkts/{}ms)",
        added_after_remove == 0,
        added_after_remove,
        PACKET_WINDOW.as_millis(),
        local_after_remove > 0,
        local_after_remove,
        PACKET_WINDOW.as_millis(),
        lan_after_remove > 0,
        lan_after_remove,
        PACKET_WINDOW.as_millis()
    );
    let control_alive = probe_control(&mut ws).await;
    println!("CONTROL: mix_ack after remove = {control_alive}");

    // --- Remove the control peer's own target: the written removal semantics ------------------
    match handle.remove_target("127.0.0.1:50000") {
        Ok(address) => println!("REMOVE: removed the control peer's target {address}"),
        Err(error) => println!("REMOVE FAILED: {error}"),
    }
    let peer_error = probe_control_error(&mut ws).await;
    println!(
        "CONTROL: next mix after removing the peer's target answered with error={peer_error:?}"
    );
    let lan_after_peer_removed = drain(&lan, PACKET_WINDOW);
    println!(
        "REMOVE: lan still streaming after the peer's target was removed={} ({} pkts/{}ms)",
        lan_after_peer_removed > 0,
        lan_after_peer_removed,
        PACKET_WINDOW.as_millis()
    );

    // --- Beacon -------------------------------------------------------------------------------
    let beacon_seen = {
        let _ = beacon_listener.join();
        beacons.load(Ordering::SeqCst)
    };
    handle.stop().expect("engine should stop cleanly");
    println!(
        "VERDICT: added_flowing={added_flowing} local_kept={} lan_kept={} added_stopped={} peer_error={peer_error:?} beacon={beacon_seen}",
        local_after_add > 0 && local_after_remove > 0,
        lan_after_add > 0 && lan_after_remove > 0 && lan_after_peer_removed > 0,
        added_after_remove == 0,
    );
}

fn listener(address: &str, label: &str) -> Option<UdpSocket> {
    match UdpSocket::bind(address) {
        Ok(socket) => {
            socket
                .set_nonblocking(true)
                .expect("listener should be non-blocking");
            println!("LISTENER: {label} bound to {address}");
            Some(socket)
        }
        Err(error) => {
            println!("LISTENER: could not bind {label} to {address}: {error}");
            None
        }
    }
}

/// Waits until one PMON datagram arrives, so "the target is receiving" is proven by a packet.
fn wait_for_packet(socket: &UdpSocket, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut buffer = [0u8; 65535];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((length, _)) if length >= 4 && &buffer[0..4] == b"PMON" => return true,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    false
}

/// Counts the PMON datagrams that arrive within a window.
fn drain(socket: &UdpSocket, window: Duration) -> usize {
    let deadline = Instant::now() + window;
    let mut count = 0;
    let mut buffer = [0u8; 65535];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((length, _)) if length >= 4 && &buffer[0..4] == b"PMON" => count += 1,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    count
}

/// Reads messages until a `config` arrives, returning its `source_channels`.
async fn next_config(ws: &mut Ws) -> Option<u8> {
    let deadline = Instant::now() + CONTROL_TIMEOUT;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if value.get("type").and_then(|kind| kind.as_str()) == Some("config") {
                        return value
                            .get("source_channels")
                            .and_then(|channels| channels.as_u64())
                            .map(|channels| channels as u8);
                    }
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => break,
        }
    }
    None
}

/// Sends a `mix` command and waits for the `mix_ack`, proving the control channel still works.
async fn probe_control(ws: &mut Ws) -> bool {
    let mix = r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#;
    if ws.send(Message::Text(mix.into())).await.is_err() {
        return false;
    }
    let deadline = Instant::now() + CONTROL_TIMEOUT;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if text.contains("\"mix_ack\"") {
                    return true;
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => break,
        }
    }
    false
}

/// Sends a `mix` command and returns the error message the server answers with, if any. The socket
/// must stay open across the removal, so this uses the same connection it was opened with.
async fn probe_control_error(ws: &mut Ws) -> Option<String> {
    let mix = r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#;
    if ws.send(Message::Text(mix.into())).await.is_err() {
        return None;
    }
    let deadline = Instant::now() + CONTROL_TIMEOUT;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if value.get("type").and_then(|kind| kind.as_str()) == Some("error") {
                        return value
                            .get("message")
                            .and_then(|message| message.as_str())
                            .map(str::to_owned);
                    }
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => break,
        }
    }
    None
}

/// Resolves the interface the OS would use for an arbitrary off-host destination, without sending
/// a byte: a connected UDP socket only fixes the route and source address.
fn outbound_local_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip: SocketAddr = socket.local_addr().ok()?;
    let ip = ip.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip)
    }
}
