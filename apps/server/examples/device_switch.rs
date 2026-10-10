//! Headless proof that switching the input device keeps the rest of the session alive.
//!
//! Run with `cargo run -p personal-monitoring --example device_switch`. It starts the engine, opens a
//! control WebSocket and a discovery-beacon listener, attempts a switch that must fail, then switches
//! to a second real device. It reports whether the control socket stayed open, whether a `config`
//! message was re-sent when the channel count changed, and whether beacons kept arriving.
//!
//! This drives the same engine calls the desktop window delegates to; the window only renders them.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use personal_monitoring::{list_input_devices, start, EngineConfig};
use tokio_tungstenite::tungstenite::Message;

const CONFIG_TIMEOUT: Duration = Duration::from_secs(3);
const EVENT_TIMEOUT: Duration = Duration::from_secs(2);

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[tokio::main]
async fn main() {
    let devices = list_input_devices().expect("device enumeration should work");
    println!("Available input devices:");
    for device in &devices {
        println!("  {} ({} channels)", device.name, device.channels);
    }

    let config = EngineConfig {
        device_filter: None,
        targets: discovery_targets(),
        control_port: 50001,
        groups: vec![],
    };
    let Some(mut handle) = start(config).expect("engine should start") else {
        println!("No input device is available.");
        return;
    };
    let started_device = handle.device_name().to_owned();
    let started_channels = handle.status().source_channels;
    println!("Started on \"{started_device}\" ({started_channels} channels)");

    // Discovery beacon listener. The beacon is broadcast to 255.255.255.255:50002 every second.
    let beacons: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let beacon_flag = Arc::clone(&beacons);
    let beacon_listener = std::thread::spawn(move || {
        // Bind the listener to the same interface the beacon is sent from, so the broadcast loops
        // back to us on this host instead of being dropped.
        let bind = match outbound_local_ip() {
            Some(ip) => format!("{ip}:50002"),
            None => "0.0.0.0:50002".to_owned(),
        };
        let socket = match UdpSocket::bind(&bind) {
            Ok(socket) => socket,
            Err(error) => {
                println!("BEACON: could not bind listener on {bind}: {error}");
                return;
            }
        };
        let _ = socket.set_nonblocking(true);
        let _ = socket.set_broadcast(true);
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut buffer = [0u8; 64];
        while Instant::now() < deadline {
            match socket.recv_from(&mut buffer) {
                Ok((length, _)) if length >= 4 && &buffer[0..4] == b"MLNK" => {
                    beacon_flag.store(true, Ordering::SeqCst);
                    println!("BEACON: received an MLNK datagram");
                    return;
                }
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        println!("BEACON: no beacon within 8s");
    });

    // Control WebSocket, exactly the socket a musician's client holds open across a switch.
    let control_url = format!("ws://127.0.0.1:{}", handle.control_port());
    let (mut ws, _) = tokio_tungstenite::connect_async(&control_url)
        .await
        .expect("control connection should open");
    let first = next_config(&mut ws).await;
    println!("CONTROL: first config source_channels={first:?}");

    // A switch that must fail: an unknown device. The engine must keep running and the socket alive.
    match handle.switch_device(Some("definitely-not-a-real-input-device")) {
        Ok(_) => println!("FAILURE PATH: UNEXPECTED success switching to a missing device"),
        Err(error) => println!("FAILURE PATH: switch rejected as expected: {error}"),
    }
    let alive = probe_control(&mut ws).await;
    println!("FAILURE PATH: control channel alive afterwards = {alive}");
    println!(
        "FAILURE PATH: still on \"{}\" = {}",
        handle.device_name(),
        handle.device_name() == started_device
    );

    // A real switch, preferring a device whose channel count differs so a re-sent config is expected.
    let target = devices
        .iter()
        .find(|device| {
            device.channels > 0
                && device.name != handle.device_name()
                && device.channels as u8 != handle.status().source_channels
        })
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.channels > 0 && device.name != handle.device_name())
        });

    let Some(target) = target else {
        println!("SWITCH: no second usable input device is available; skipping the real switch");
        handle.stop().expect("engine should stop cleanly");
        finish(&beacons, beacon_listener);
        return;
    };

    let expected_channels = target.channels as u8;
    let target_name = target.name.clone();
    let current_channels = handle.status().source_channels;
    println!(
        "SWITCH: \"{started_device}\" ({current_channels} ch) -> \"{target_name}\" ({expected_channels} ch)"
    );
    match handle.switch_device(Some(&target_name)) {
        Ok(format) => println!(
            "SWITCH: committed; now \"{}\" ({} channels)",
            handle.device_name(),
            format.channels
        ),
        Err(error) => {
            println!("SWITCH: failed: {error}");
            handle.stop().expect("engine should stop cleanly");
            finish(&beacons, beacon_listener);
            return;
        }
    }

    let configs = if expected_channels != current_channels {
        let resent = next_config(&mut ws).await;
        println!(
            "SWITCH: re-sent config source_channels={resent:?} (expected {expected_channels})"
        );
        resent
    } else {
        println!("SWITCH: channel count unchanged; config re-send was not expected");
        None
    };
    let alive = probe_control(&mut ws).await;
    println!("SWITCH: control channel alive across the switch = {alive}");

    // Let the beacon listener observe the still-running engine before stopping it.
    let beacon_seen = {
        let _ = beacon_listener.join();
        beacons.load(Ordering::SeqCst)
    };
    handle.stop().expect("engine should stop cleanly");
    println!(
        "SWITCH VERDICT: config_resent={} control_alive={} beacon={}",
        configs == Some(expected_channels),
        alive,
        beacon_seen
    );
    if beacon_seen {
        println!("BEACON: kept arriving during the session");
    }
}

/// Reads messages until a `config` arrives, returning its `source_channels`.
async fn next_config(ws: &mut Ws) -> Option<u8> {
    let deadline = Instant::now() + CONFIG_TIMEOUT;
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
    let deadline = Instant::now() + EVENT_TIMEOUT;
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

fn finish(beacons: &AtomicBool, listener: std::thread::JoinHandle<()>) {
    let _ = listener.join();
    if beacons.load(Ordering::SeqCst) {
        println!("BEACON: kept arriving during the session");
    }
}

/// The UDP targets the engine resolves. `127.0.0.1` is the audio target; any real local interface
/// address is added so the discovery beacon is sent from a real interface instead of the unbound
/// fallback, which lets the local beacon listener actually see it.
fn discovery_targets() -> Vec<String> {
    let mut targets = vec!["127.0.0.1:50000".to_owned()];
    if let Some(ip) = outbound_local_ip() {
        targets.push(format!("{ip}:50000"));
    }
    targets
}

/// Resolves the interface the OS would use for an arbitrary off-host destination, without sending a
/// byte: a connected UDP socket only fixes the route and source address.
fn outbound_local_ip() -> Option<std::net::IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip)
    }
}
