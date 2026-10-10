use std::collections::HashMap;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::capture::TARGET_SAMPLE_RATE;
use crate::engine::{EngineEvent, EventBus};
use crate::mix::{GroupLayout, MixState};
use crate::protocol::{control_error, mix_ack, parse_mix_command, ControlConfig, GroupConfig};

/// Counts the live control connections per client IP.
///
/// More than one socket can come from the same IP (a reload before the old one closes), so the
/// registry counts rather than flags; the engine reports a musician as connected while the count is
/// above zero. The IP is the join key between [MixState], the UDP target and the control peer.
pub(crate) type ControlPeers = Arc<Mutex<HashMap<IpAddr, usize>>>;

pub(crate) fn spawn_control_thread(
    control_port: u16,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    stopped: Arc<AtomicBool>,
    source_channels: u8,
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
            mix_states,
            stopped,
            source_channels,
            groups,
            peers,
            events,
        ));
    })
}

async fn run_control_server(
    listener: std::net::TcpListener,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    stopped: Arc<AtomicBool>,
    source_channels: u8,
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
        match accepted {
            None => continue,
            Some(accepted) => match accepted {
                Ok((stream, peer)) => {
                    let states = Arc::clone(&mix_states);
                    let groups = Arc::clone(&groups);
                    let peers = Arc::clone(&peers);
                    let events = events.clone();
                    tokio::spawn(async move {
                        if let Err(error) = handle_control_connection(
                            stream,
                            peer,
                            states,
                            source_channels,
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
struct PeerGuard {
    address: SocketAddr,
    peers: ControlPeers,
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
                self.events.emit(EngineEvent::ClientDisconnected {
                    address: self.address,
                });
            }
        }
    }
}

/// Registers a live connection and returns the guard that deregisters it, so the caller never has
/// to remember to undo this on every exit path.
fn register_peer(peers: &ControlPeers, events: &EventBus, address: SocketAddr) -> PeerGuard {
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
        events: events.clone(),
    }
}

async fn handle_control_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    source_channels: u8,
    groups: Arc<GroupLayout>,
    peers: ControlPeers,
    events: EventBus,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let websocket = accept_async(stream).await?;
    let _peer = register_peer(&peers, &events, peer);
    let (mut writer, mut reader) = websocket.split();
    let config = serde_json::to_string(&ControlConfig {
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
    })?;
    writer.send(Message::Text(config.into())).await?;
    while let Some(message) = reader.next().await {
        let message = message?;
        match message {
            Message::Text(text) => {
                let response = match mix_states.get(&peer.ip()) {
                    None => control_error("no UDP target configured for client IP".to_owned())?,
                    Some(mix_state) => match parse_mix_command(&text, mix_state.snapshot()) {
                        Ok(values) => {
                            mix_state.update(values);
                            events.emit(EngineEvent::MixChanged { address: peer });
                            mix_ack(mix_state.snapshot())?
                        }
                        Err(error) => control_error(error)?,
                    },
                };
                writer.send(Message::Text(response.into())).await?;
            }
            Message::Close(_) => break,
            Message::Ping(payload) => writer.send(Message::Pong(payload)).await?,
            _ => {}
        }
    }
    Ok(())
}
