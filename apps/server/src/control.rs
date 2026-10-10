use std::collections::HashMap;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::capture::TARGET_SAMPLE_RATE;
use crate::mix::{GroupLayout, MixState};
use crate::protocol::{control_error, mix_ack, parse_mix_command, ControlConfig, GroupConfig};

pub(crate) fn spawn_control_thread(
    control_port: u16,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    stopped: Arc<AtomicBool>,
    source_channels: u8,
    groups: Arc<GroupLayout>,
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
        ));
    })
}

async fn run_control_server(
    listener: std::net::TcpListener,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    stopped: Arc<AtomicBool>,
    source_channels: u8,
    groups: Arc<GroupLayout>,
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
                    tokio::spawn(async move {
                        if let Err(error) =
                            handle_control_connection(stream, peer, states, source_channels, groups)
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

async fn handle_control_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    source_channels: u8,
    groups: Arc<GroupLayout>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let websocket = accept_async(stream).await?;
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
