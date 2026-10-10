use std::collections::HashMap;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::capture::TARGET_SAMPLE_RATE;
use crate::engine::{EngineEvent, EventBus};
use crate::mix::GroupLayout;
use crate::protocol::{
    control_error, control_message_type, mix_ack, parse_mix_command, parse_register_command,
    registered_name, ControlConfig, GroupConfig,
};
use crate::targets::TargetRegistry;

/// Counts the live control connections per client IP.
///
/// More than one socket can come from the same IP (a reload before the old one closes), so the
/// registry counts rather than flags; the engine reports a musician as connected while the count is
/// above zero. The IP is the join key between [MixState], the UDP target and the control peer.
pub(crate) type ControlPeers = Arc<Mutex<HashMap<IpAddr, usize>>>;

/// How often a control connection checks whether a device switch changed the channel count.
const CONFIG_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The live control configuration shared between the engine and every connection.
///
/// A device switch can change the captured channel count while the server runs. The engine stores the
/// current count here; each connection notices a change on its next poll and re-sends `config` on the
/// same socket, so a client rebuilds its per-channel controls without losing the control channel.
pub(crate) struct ConfigState {
    channels: AtomicU8,
}

impl ConfigState {
    pub(crate) fn new(channels: u8) -> Self {
        Self {
            channels: AtomicU8::new(channels),
        }
    }

    pub(crate) fn current(&self) -> u8 {
        self.channels.load(Ordering::SeqCst)
    }

    /// Publishes a new channel count. Storing an unchanged count is harmless: a connection compares
    /// it against the count it last sent.
    pub(crate) fn set_channels(&self, channels: u8) {
        self.channels.store(channels, Ordering::SeqCst);
    }
}

/// Builds the `config` control message for a channel count and group layout.
///
/// Free of connection state so it can be sent on connect and re-sent on a device switch from the
/// same code, which is what keeps the two messages identical in shape.
fn config_message(source_channels: u8, groups: &GroupLayout) -> Result<String, serde_json::Error> {
    serde_json::to_string(&ControlConfig {
        message_type: "config",
        source_channels,
        sample_rate: TARGET_SAMPLE_RATE,
        groups: groups
            .groups()
            .iter()
            .map(|group| GroupConfig {
                name: group.name.clone(),
                channels: group.channels.clone(),
            })
            .collect(),
    })
}

pub(crate) fn spawn_control_thread(
    control_port: u16,
    targets: Arc<TargetRegistry>,
    stopped: Arc<AtomicBool>,
    config_state: Arc<ConfigState>,
    groups: Arc<GroupLayout>,
    peers: ControlPeers,
    events: EventBus,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let listener = match std::net::TcpListener::bind(("0.0.0.0", control_port)) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("control listener bind error: {error}");
                return;
            }
        };
        if let Err(error) = listener.set_nonblocking(true) {
            eprintln!("control listener setup error: {error}");
            return;
        }
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("control runtime error: {error}");
                return;
            }
        };
        runtime.block_on(run_control_server(
            listener,
            targets,
            stopped,
            config_state,
            groups,
            peers,
            events,
        ));
    })
}

async fn run_control_server(
    listener: std::net::TcpListener,
    targets: Arc<TargetRegistry>,
    stopped: Arc<AtomicBool>,
    config_state: Arc<ConfigState>,
    groups: Arc<GroupLayout>,
    peers: ControlPeers,
    events: EventBus,
) {
    let listener = match TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("control listener error: {error}");
            return;
        }
    };
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => Some(accepted),
            _ = tokio::time::sleep(Duration::from_millis(100)) => None,
        };
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        // The accept loop already ticks a few times a second, so it is also the clock that removes
        // registered targets whose control channel stayed gone past the grace period.
        for address in targets.sweep_expired(Instant::now()) {
            println!("control registration {address} removed after the grace period");
        }
        match accepted {
            None => continue,
            Some(accepted) => match accepted {
                Ok((stream, peer)) => {
                    let targets = Arc::clone(&targets);
                    let config_state = Arc::clone(&config_state);
                    let groups = Arc::clone(&groups);
                    let peers = Arc::clone(&peers);
                    let events = events.clone();
                    tokio::spawn(async move {
                        if let Err(error) = handle_control_connection(
                            stream,
                            peer,
                            targets,
                            config_state,
                            groups,
                            peers,
                            events,
                        )
                        .await
                        {
                            eprintln!("control connection {peer} error: {error}");
                        }
                    });
                }
                Err(error) => eprintln!("control accept error: {error}"),
            },
        }
    }
}

/// Removes one connection from the registry when the task ends, for any reason, and emits the
/// matching disconnect event on the last one. A guard keeps the bookkeeping correct across every
/// early return in the connection loop.
///
/// On the last connection for an IP, the peer's registered target is marked disconnected: it is
/// not removed here, because the grace period decides that, and it starts now.
struct PeerGuard {
    address: SocketAddr,
    peers: ControlPeers,
    targets: Arc<TargetRegistry>,
    events: EventBus,
}

impl Drop for PeerGuard {
    fn drop(&mut self) {
        let ip = self.address.ip();
        let mut peers = self.peers.lock().expect("control peer registry poisoned");
        match peers.get_mut(&ip) {
            Some(count) if *count > 1 => {
                *count -= 1;
            }
            _ => {
                peers.remove(&ip);
                drop(peers);
                self.targets.mark_disconnected(ip, Instant::now());
                self.events.emit(EngineEvent::ClientDisconnected {
                    address: self.address,
                });
            }
        }
    }
}

/// Registers a live connection and returns the guard that deregisters it, so the caller never has
/// to remember to undo this on every exit path.
fn register_peer(
    peers: &ControlPeers,
    events: &EventBus,
    targets: &Arc<TargetRegistry>,
    address: SocketAddr,
) -> PeerGuard {
    let ip = address.ip();
    {
        let mut peers = peers.lock().expect("control peer registry poisoned");
        let count = peers.entry(ip).or_insert(0);
        *count += 1;
        if *count == 1 {
            events.emit(EngineEvent::ClientConnected { address });
        }
    }
    PeerGuard {
        address,
        peers: Arc::clone(peers),
        targets: Arc::clone(targets),
        events: events.clone(),
    }
}

async fn handle_control_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    targets: Arc<TargetRegistry>,
    config_state: Arc<ConfigState>,
    groups: Arc<GroupLayout>,
    peers: ControlPeers,
    events: EventBus,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let websocket = accept_async(stream).await?;
    let _peer = register_peer(&peers, &events, &targets, peer);
    let (writer, mut reader) = websocket.split();

    // The sink is shared between the read loop and the config watcher through an async mutex. The
    // read loop only holds it while sending, never while parked in a read.
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    let sent_channels = Arc::new(AtomicU8::new(config_state.current()));
    writer
        .lock()
        .await
        .send(Message::Text(
            config_message(sent_channels.load(Ordering::SeqCst), &groups)?.into(),
        ))
        .await?;

    // The watcher notices a device switch while the connection is idle and re-sends `config` on the
    // SAME socket, so a client rebuilds its per-channel controls without losing the control channel.
    let watcher_writer = Arc::clone(&writer);
    let watcher_state = Arc::clone(&config_state);
    let watcher_groups = Arc::clone(&groups);
    let watcher_sent = Arc::clone(&sent_channels);
    let watcher_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(CONFIG_POLL_INTERVAL).await;
            let current = watcher_state.current();
            if current == watcher_sent.load(Ordering::SeqCst) {
                continue;
            }
            watcher_sent.store(current, Ordering::SeqCst);
            let Ok(config) = config_message(current, &watcher_groups) else {
                break;
            };
            if watcher_writer
                .lock()
                .await
                .send(Message::Text(config.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    loop {
        // Safety net for a runtime that does not drive the watcher's timer while this task is parked
        // on a read: re-check the channel count before parking again, so the next client message also
        // re-sends `config`. It is a cheap atomic load when nothing changed.
        let current = config_state.current();
        if current != sent_channels.load(Ordering::SeqCst) {
            sent_channels.store(current, Ordering::SeqCst);
            writer
                .lock()
                .await
                .send(Message::Text(config_message(current, &groups)?.into()))
                .await?;
        }

        let Some(message) = reader.next().await else {
            break;
        };
        let message = message?;
        match message {
            Message::Text(text) => {
                let message_type = control_message_type(&text).unwrap_or_default();
                let response = if message_type == "register" {
                    // A registration is the client announcing itself: the address comes from the
                    // socket and the port and name come from the message. Success is answered with
                    // nothing, so a client talking to a server that predates registration (which
                    // answers with its usual error or ignores it) behaves exactly as it did. The
                    // shapes of the existing messages are untouched; this only adds a type.
                    match parse_register_command(&text) {
                        Ok(command) => {
                            let name = registered_name(&command.name);
                            let address = targets.register(
                                peer.ip(),
                                command.udp_port,
                                name.clone(),
                                &groups,
                            );
                            match &name {
                                Some(name) => println!(
                                    "control registration from {peer}: {address} as \"{name}\""
                                ),
                                None => println!(
                                    "control registration from {peer}: {address} (unnamed)"
                                ),
                            }
                            None
                        }
                        Err(error) => Some(control_error(error)?),
                    }
                } else {
                    // The join is live: a peer whose target was removed at runtime finds no mix
                    // state here and gets the same error an IP that was never configured has always
                    // received. The control channel itself is deliberately left open.
                    Some(match targets.mix_state(peer.ip()) {
                        None => control_error("no UDP target configured for client IP".to_owned())?,
                        Some(mix_state) => match parse_mix_command(&text, mix_state.snapshot()) {
                            Ok(values) => {
                                mix_state.update(values);
                                events.emit(EngineEvent::MixChanged { address: peer });
                                mix_ack(mix_state.snapshot())?
                            }
                            Err(error) => control_error(error)?,
                        },
                    })
                };
                if let Some(response) = response {
                    if writer
                        .lock()
                        .await
                        .send(Message::Text(response.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            Message::Close(_) => break,
            Message::Ping(payload) => {
                if writer
                    .lock()
                    .await
                    .send(Message::Pong(payload))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            _ => {}
        }
    }

    watcher_task.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::Group;

    fn drums() -> GroupLayout {
        GroupLayout::new(vec![Group {
            name: "Drums".to_owned(),
            channels: vec![0, 1],
        }])
    }

    #[test]
    fn config_message_carries_the_source_channels_and_groups() {
        let json = config_message(4, &drums()).expect("config should serialize");

        assert!(json.contains(r#""source_channels":4"#));
        assert!(json.contains(r#""groups":[{"name":"Drums","channels":[0,1]}]"#));
    }

    #[test]
    fn config_state_reports_the_latest_channel_count() {
        let state = ConfigState::new(2);

        assert_eq!(state.current(), 2);

        state.set_channels(4);

        assert_eq!(state.current(), 4);
    }

    use tokio_tungstenite::connect_async;

    type ClientStream = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn read_source_channels(client: &mut ClientStream) -> Option<u8> {
        while let Some(message) = client.next().await {
            if let Ok(Message::Text(text)) = message {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if value.get("type").and_then(|kind| kind.as_str()) == Some("config") {
                        return value
                            .get("source_channels")
                            .and_then(|channels| channels.as_u64())
                            .map(|channels| channels as u8);
                    }
                }
            }
        }
        None
    }

    /// Reads until a message of `kind` arrives, returning its parsed body. Bounded by a deadline so
    /// a missing answer fails the test instead of hanging it.
    async fn read_message_of_type(
        client: &mut ClientStream,
        kind: &str,
    ) -> Option<serde_json::Value> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match tokio::time::timeout(remaining, client.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        if value.get("type").and_then(|kind| kind.as_str()) == Some(kind) {
                            return Some(value);
                        }
                    }
                }
                Ok(Some(Ok(_))) => continue,
                _ => return None,
            }
        }
        None
    }

    /// Exercises the re-send over a real socket, without audio hardware: a client that connected
    /// with two channels learns about four on the same connection.
    ///
    /// The server runs on its own runtime thread, exactly as the engine runs it, so the client's
    /// runtime and the accept loop never share a scheduler.
    #[tokio::test]
    async fn a_channel_count_change_is_pushed_to_a_connected_client() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind control listener");
        let address = listener.local_addr().expect("listener address");
        let stopped = Arc::new(AtomicBool::new(false));
        let config_state = Arc::new(ConfigState::new(2));

        let server_stopped = Arc::clone(&stopped);
        let server_config = Arc::clone(&config_state);
        let server = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("control runtime should build");
            runtime.block_on(run_control_server(
                listener,
                Arc::new(
                    TargetRegistry::new(&[], &GroupLayout::default())
                        .expect("registry should build"),
                ),
                server_stopped,
                server_config,
                Arc::new(drums()),
                Arc::new(Mutex::new(HashMap::new())),
                EventBus::default(),
            ));
        });

        let (mut client, _) = tokio::time::timeout(
            Duration::from_secs(5),
            connect_async(format!("ws://{address}")),
        )
        .await
        .expect("client should connect within 5s")
        .expect("client should connect");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), read_source_channels(&mut client))
                .await
                .expect("first config should arrive"),
            Some(2)
        );

        // The equivalent of the engine publishing a new device's channel count.
        config_state.set_channels(4);

        // The connection re-checks the channel count before each read, so any client message makes
        // it re-send `config`. A mix command is the message a musician's client would send anyway.
        let mix = r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#;
        client
            .send(Message::Text(mix.into()))
            .await
            .expect("client should send");

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), read_source_channels(&mut client))
                .await
                .expect("re-sent config should arrive"),
            Some(4)
        );

        stopped.store(true, Ordering::SeqCst);
        // The accept loop re-checks `stopped` on the next accept, so nudge it with one connection so
        // the server thread can exit deterministically.
        let _ = std::net::TcpStream::connect(address);
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || server.join()),
        )
        .await;
    }

    /// The removal semantics, exercised over a real socket: removing a target stops its audio but
    /// does NOT close its control channel. The next `mix` gets the existing no-target error, and the
    /// connection stays open for the one after it; adding the address back joins the peer again.
    #[tokio::test]
    async fn a_removed_targets_control_peer_keeps_its_channel_and_gets_the_no_udp_target_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind control listener");
        let address = listener.local_addr().expect("listener address");
        let stopped = Arc::new(AtomicBool::new(false));
        // The client connects from 127.0.0.1, so this target's IP is the one its control messages
        // join against.
        let targets = Arc::new(
            TargetRegistry::new(
                &["127.0.0.1:50000".parse().expect("target address")],
                &GroupLayout::default(),
            )
            .expect("registry should build"),
        );

        let server_stopped = Arc::clone(&stopped);
        let server_targets = Arc::clone(&targets);
        let server = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("control runtime should build");
            runtime.block_on(run_control_server(
                listener,
                server_targets,
                server_stopped,
                Arc::new(ConfigState::new(2)),
                Arc::new(drums()),
                Arc::new(Mutex::new(HashMap::new())),
                EventBus::default(),
            ));
        });

        let (mut client, _) = tokio::time::timeout(
            Duration::from_secs(5),
            connect_async(format!("ws://{address}")),
        )
        .await
        .expect("client should connect within 5s")
        .expect("client should connect");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), read_source_channels(&mut client))
                .await
                .expect("config should arrive"),
            Some(2)
        );

        let mix = r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#;
        client
            .send(Message::Text(mix.into()))
            .await
            .expect("client should send");
        assert!(
            read_message_of_type(&mut client, "mix_ack").await.is_some(),
            "a configured target's peer should be acknowledged"
        );

        targets
            .remove("127.0.0.1:50000")
            .expect("configured target should be removed");

        client
            .send(Message::Text(mix.into()))
            .await
            .expect("client should send");
        let error = read_message_of_type(&mut client, "error")
            .await
            .expect("the removed peer should get an error");
        assert_eq!(
            error.get("message").and_then(|message| message.as_str()),
            Some("no UDP target configured for client IP")
        );

        // The channel was not closed: a second message on the same socket gets the same answer.
        client
            .send(Message::Text(mix.into()))
            .await
            .expect("client should send");
        assert!(
            read_message_of_type(&mut client, "error").await.is_some(),
            "the control channel should still be open after the removal"
        );

        // Adding the address back joins the peer again, with no reconnect.
        targets
            .add("127.0.0.1:50000", &GroupLayout::default())
            .expect("fresh address should be accepted");
        client
            .send(Message::Text(mix.into()))
            .await
            .expect("client should send");
        assert!(
            read_message_of_type(&mut client, "mix_ack").await.is_some(),
            "a re-added target should be acknowledged on the same socket"
        );

        stopped.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(address);
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || server.join()),
        )
        .await;
    }

    /// Starts a control server on an ephemeral port and returns its address, the shared target
    /// registry, and the handles needed to stop it. Shared by the registration tests.
    fn start_registration_server(
        targets: Arc<TargetRegistry>,
    ) -> (SocketAddr, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind control listener");
        let address = listener.local_addr().expect("listener address");
        let stopped = Arc::new(AtomicBool::new(false));
        let server_stopped = Arc::clone(&stopped);
        let server = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("control runtime should build");
            runtime.block_on(run_control_server(
                listener,
                targets,
                server_stopped,
                Arc::new(ConfigState::new(2)),
                Arc::new(drums()),
                Arc::new(Mutex::new(HashMap::new())),
                EventBus::default(),
            ));
        });
        (address, stopped, server)
    }

    async fn stop_server(
        address: SocketAddr,
        stopped: &Arc<AtomicBool>,
        server: std::thread::JoinHandle<()>,
    ) {
        stopped.store(true, Ordering::SeqCst);
        // The accept loop re-checks `stopped` on the next accept, so nudge it with one connection.
        let _ = std::net::TcpStream::connect(address);
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || server.join()),
        )
        .await;
    }

    /// The full path over real sockets: a registration creates a target at the socket's address
    /// (even when the message claims another one), the control join starts working, and the UDP
    /// send thread the engine runs delivers the next packet to the announced port.
    #[tokio::test]
    async fn a_registration_creates_a_target_that_starts_receiving_audio() {
        let targets =
            Arc::new(TargetRegistry::new(&[], &GroupLayout::default()).expect("empty registry"));
        let (address, stopped, server) = start_registration_server(Arc::clone(&targets));

        // The "phone": its port is what the registration announces, and where its audio must land.
        let phone = std::net::UdpSocket::bind("127.0.0.1:0").expect("phone socket");
        let phone_port = phone.local_addr().expect("phone address").port();
        let target: SocketAddr = format!("127.0.0.1:{phone_port}")
            .parse()
            .expect("target address");

        let (mut client, _) = tokio::time::timeout(
            Duration::from_secs(5),
            connect_async(format!("ws://{address}")),
        )
        .await
        .expect("client should connect within 5s")
        .expect("client should connect");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), read_source_channels(&mut client))
                .await
                .expect("config should arrive"),
            Some(2)
        );

        client
            .send(Message::Text(
                format!(
                    r#"{{"type":"register","name":"Ana","udp_port":{phone_port},"address":"10.0.0.99:9"}}"#
                )
                .into(),
            ))
            .await
            .expect("client should send");

        // The claimed address is ignored: the target is built from the socket's 127.0.0.1.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !targets.read().iter().any(|entry| entry.address == target) {
            assert!(
                std::time::Instant::now() < deadline,
                "the registration should create a target"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        {
            let entries = targets.read();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].address, target);
            assert_eq!(entries[0].origin.name(), Some("Ana"));
        }

        // The join is live: the registered peer's mix is acknowledged on the same socket.
        let mix = r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#;
        client
            .send(Message::Text(mix.into()))
            .await
            .expect("client should send");
        assert!(
            read_message_of_type(&mut client, "mix_ack").await.is_some(),
            "a registered peer should be acknowledged"
        );

        // The same UDP send thread the engine runs: one packet in, one packet at the phone.
        let (sender, receiver) = std::sync::mpsc::sync_channel(4);
        let network = crate::network::spawn_network_thread(
            receiver,
            Arc::clone(&targets),
            Arc::new(crate::network::PacketStats {
                sent: std::sync::atomic::AtomicU64::new(0),
                discarded: std::sync::atomic::AtomicU64::new(0),
            }),
        )
        .expect("network thread should spawn");
        sender
            .send(crate::protocol::AudioPacket {
                channels: 2,
                sample_rate: 48_000,
                sequence: 0,
                samples: vec![0, 0],
            })
            .expect("packet should queue");
        let received = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                phone.set_read_timeout(Some(Duration::from_secs(4))).ok();
                let mut buffer = [0u8; 4096];
                phone
                    .recv(&mut buffer)
                    .map(|length| length > 0)
                    .unwrap_or(false)
            }),
        )
        .await
        .expect("audio should arrive within 5s")
        .expect("recv task should finish");
        assert!(received, "the registered phone should receive audio");

        drop(sender);
        network.join().expect("network thread should exit");
        stop_server(address, &stopped, server).await;
    }

    /// The grace period over a real socket: dropping the control channel keeps the target, and the
    /// drop is what arms its eventual removal. The exact boundary is covered by the pure tests in
    /// `targets`; this proves the wiring between the connection guard and the sweep.
    #[tokio::test]
    async fn a_dropped_registration_keeps_the_target_for_the_grace_period_and_then_removes_it() {
        let targets =
            Arc::new(TargetRegistry::new(&[], &GroupLayout::default()).expect("empty registry"));
        let (address, stopped, server) = start_registration_server(Arc::clone(&targets));
        let target: SocketAddr = "127.0.0.1:50123".parse().expect("target address");

        let (mut client, _) = tokio::time::timeout(
            Duration::from_secs(5),
            connect_async(format!("ws://{address}")),
        )
        .await
        .expect("client should connect within 5s")
        .expect("client should connect");
        assert!(
            read_message_of_type(&mut client, "config").await.is_some(),
            "config should arrive"
        );
        client
            .send(Message::Text(
                r#"{"type":"register","name":"Ana","udp_port":50123}"#.into(),
            ))
            .await
            .expect("client should send");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !targets.read().iter().any(|entry| entry.address == target) {
            assert!(
                std::time::Instant::now() < deadline,
                "the registration should create a target"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        client.close(None).await.expect("the client should close");
        drop(client);

        // The grace keeps the target: a sweep at the present time removes nothing.
        assert!(targets.sweep_expired(std::time::Instant::now()).is_empty());
        assert!(
            targets.read().iter().any(|entry| entry.address == target),
            "a brief drop must not cut the audio"
        );

        // Once the grace is past, the drop's mark makes the sweep remove the target.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let removed = loop {
            let removed = targets.sweep_expired(
                std::time::Instant::now()
                    + crate::targets::REGISTRATION_GRACE_PERIOD
                    + Duration::from_secs(1),
            );
            if !removed.is_empty() {
                break removed;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the dropped socket should mark the target for removal"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };

        assert_eq!(removed, vec![target]);
        assert!(targets.read().is_empty());

        stop_server(address, &stopped, server).await;
    }
}
