use std::collections::HashMap;
use std::error::Error;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::mix::{mix_channels, MixState, OUTPUT_CHANNELS};
use crate::protocol::{serialize_packet, AudioPacket};

pub(crate) struct PacketStats {
    pub(crate) sent: AtomicU64,
    pub(crate) discarded: AtomicU64,
}

pub(crate) fn spawn_network_thread(
    packet_receiver: Receiver<AudioPacket>,
    targets: Vec<SocketAddr>,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    packet_stats: Arc<PacketStats>,
) -> Result<JoinHandle<()>, Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    Ok(thread::spawn(move || {
        for packet in packet_receiver {
            for target in &targets {
                let mix_state = mix_states
                    .get(&target.ip())
                    .expect("every target must have a mix state");
                let samples = mix_channels(
                    &packet.samples,
                    usize::from(packet.channels),
                    mix_state.snapshot(),
                );
                let target_packet = AudioPacket {
                    channels: OUTPUT_CHANNELS,
                    sample_rate: packet.sample_rate,
                    sequence: packet.sequence,
                    samples,
                };
                let bytes = serialize_packet(&target_packet);
                match socket.send_to(&bytes, target) {
                    Ok(_) => {
                        packet_stats.sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => {
                        packet_stats.discarded.fetch_add(1, Ordering::Relaxed);
                        eprintln!("UDP send error to {target}: {error}");
                    }
                }
            }
        }
    }))
}
