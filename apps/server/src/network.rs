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

/// Send counters for one target, so the UI can attribute loss to a musician instead of a global
/// total.
///
/// [PacketStats] stays as the global total the CLI summary prints; these are additive and do not
/// change what is sent on the wire.
#[derive(Default)]
pub(crate) struct TargetCounters {
    pub(crate) sent: AtomicU64,
    pub(crate) discarded: AtomicU64,
}

pub(crate) fn spawn_network_thread(
    packet_receiver: Receiver<AudioPacket>,
    targets: Vec<SocketAddr>,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    packet_stats: Arc<PacketStats>,
    target_counters: Arc<HashMap<IpAddr, TargetCounters>>,
) -> Result<JoinHandle<()>, Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    Ok(thread::spawn(move || {
        for packet in packet_receiver {
            for target in &targets {
                let mix_state = mix_states
                    .get(&target.ip())
                    .expect("every target must have a mix state");
                let counters = target_counters
                    .get(&target.ip())
                    .expect("every target must have send counters");
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
                        counters.sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => {
                        packet_stats.discarded.fetch_add(1, Ordering::Relaxed);
                        counters.discarded.fetch_add(1, Ordering::Relaxed);
                        eprintln!("UDP send error to {target}: {error}");
                    }
                }
            }
        }
    }))
}
