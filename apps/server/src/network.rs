use std::error::Error;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::mix::{mix_channels, OUTPUT_CHANNELS};
use crate::protocol::{serialize_packet, AudioPacket};
use crate::targets::TargetRegistry;

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

/// Spawns the UDP send thread, which reads the live target table once per packet.
///
/// The table is read through a short-lived read lock instead of being owned by the thread, so the
/// engine can add and remove targets at runtime without restarting this thread, the capture stream
/// or the control server. Each entry carries its own mix state and counters, so one packet's send
/// loop never looks anything up by IP and can never fail on a missing entry.
pub(crate) fn spawn_network_thread(
    packet_receiver: Receiver<AudioPacket>,
    targets: Arc<TargetRegistry>,
    packet_stats: Arc<PacketStats>,
) -> Result<JoinHandle<()>, Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    Ok(thread::spawn(move || {
        for packet in packet_receiver {
            let entries = targets.read();
            for entry in entries.iter() {
                let samples = mix_channels(
                    &packet.samples,
                    usize::from(packet.channels),
                    entry.mix_state.snapshot(),
                );
                let target_packet = AudioPacket {
                    channels: OUTPUT_CHANNELS,
                    sample_rate: packet.sample_rate,
                    sequence: packet.sequence,
                    samples,
                };
                let bytes = serialize_packet(&target_packet);
                match socket.send_to(&bytes, entry.address) {
                    Ok(_) => {
                        packet_stats.sent.fetch_add(1, Ordering::Relaxed);
                        entry.counters.sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => {
                        packet_stats.discarded.fetch_add(1, Ordering::Relaxed);
                        entry.counters.discarded.fetch_add(1, Ordering::Relaxed);
                        eprintln!("UDP send error to {}: {error}", entry.address);
                    }
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::GroupLayout;
    use std::net::SocketAddr;
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;

    fn bind(value: &str) -> UdpSocket {
        UdpSocket::bind(value).expect("test socket should bind")
    }

    fn packet() -> AudioPacket {
        AudioPacket {
            channels: 2,
            sample_rate: 48_000,
            sequence: 0,
            samples: vec![0, 0],
        }
    }

    fn receive(socket: &UdpSocket, timeout: Duration) -> Option<usize> {
        socket
            .set_read_timeout(Some(timeout))
            .expect("read timeout should set");
        let mut buffer = [0u8; 4096];
        socket.recv(&mut buffer).ok()
    }

    fn registry(addresses: &[SocketAddr]) -> Arc<TargetRegistry> {
        Arc::new(TargetRegistry::new(addresses, &GroupLayout::default()).expect("registry"))
    }

    fn stats() -> Arc<PacketStats> {
        Arc::new(PacketStats {
            sent: AtomicU64::new(0),
            discarded: AtomicU64::new(0),
        })
    }

    /// Removal happens under the registry's write lock, before the next packet is queued, so the
    /// send thread must see the shorter table on the next packet: the removed target gets nothing,
    /// the kept one keeps streaming, and the thread is never restarted.
    #[test]
    fn a_removed_target_stops_receiving_while_the_others_keep_streaming() {
        let removed = bind("127.0.0.1:0");
        let kept = bind("127.0.0.2:0");
        let targets = registry(&[
            removed.local_addr().expect("removed address"),
            kept.local_addr().expect("kept address"),
        ]);
        let (sender, receiver) = sync_channel(4);
        let thread = spawn_network_thread(receiver, Arc::clone(&targets), stats())
            .expect("network thread should spawn");

        sender.send(packet()).expect("packet should queue");
        assert!(receive(&removed, Duration::from_secs(2)).is_some());
        assert!(receive(&kept, Duration::from_secs(2)).is_some());

        targets
            .remove(&removed.local_addr().expect("removed address").to_string())
            .expect("configured target should be removed");

        sender.send(packet()).expect("packet should queue");
        assert!(receive(&kept, Duration::from_secs(2)).is_some());
        assert!(receive(&removed, Duration::from_millis(300)).is_none());

        drop(sender);
        thread
            .join()
            .expect("thread should exit when the sender drops");
    }

    /// The addition is visible to the same running send thread on the next packet: no restart, no
    /// reconfiguration of the capture stream or the control server.
    #[test]
    fn an_added_target_starts_receiving_without_restarting_the_network_thread() {
        let first = bind("127.0.0.1:0");
        let second = bind("127.0.0.2:0");
        let targets = registry(&[first.local_addr().expect("first address")]);
        let (sender, receiver) = sync_channel(4);
        let thread = spawn_network_thread(receiver, Arc::clone(&targets), stats())
            .expect("network thread should spawn");

        sender.send(packet()).expect("packet should queue");
        assert!(receive(&first, Duration::from_secs(2)).is_some());
        assert!(receive(&second, Duration::from_millis(300)).is_none());

        targets
            .add(
                &second.local_addr().expect("second address").to_string(),
                &GroupLayout::default(),
            )
            .expect("fresh address should be accepted");

        sender.send(packet()).expect("packet should queue");
        assert!(receive(&first, Duration::from_secs(2)).is_some());
        assert!(receive(&second, Duration::from_secs(2)).is_some());

        drop(sender);
        thread
            .join()
            .expect("thread should exit when the sender drops");
    }
}
