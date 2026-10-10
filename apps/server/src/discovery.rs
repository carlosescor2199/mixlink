use std::error::Error;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Discovery beacon datagram format, version 1:
/// magic[4] | version[1] | control_port[2 LE] | sample_rate[4 LE]
///
/// This is a separate, tiny protocol. It is neither PMON nor the control channel, so it cannot
/// break either of them.
pub(crate) const DISCOVERY_MAGIC: [u8; 4] = *b"MLNK";
pub(crate) const DISCOVERY_VERSION: u8 = 1;
const BEACON_LENGTH: usize = 4 + 1 + 2 + 4;

/// Fixed UDP port the beacon is broadcast to and the client listens on. It is intentionally
/// distinct from the audio and control ports.
pub(crate) const DISCOVERY_PORT: u16 = 50002;

/// How often the beacon is sent while the server runs.
pub(crate) const BEACON_INTERVAL: Duration = Duration::from_secs(1);

/// The two facts a client needs from the beacon. The address comes from the packet's source, so it
/// is not carried here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Beacon {
    pub(crate) control_port: u16,
    pub(crate) sample_rate: u32,
}

pub(crate) fn encode_beacon(beacon: Beacon) -> [u8; BEACON_LENGTH] {
    let mut bytes = [0u8; BEACON_LENGTH];
    bytes[0..4].copy_from_slice(&DISCOVERY_MAGIC);
    bytes[4] = DISCOVERY_VERSION;
    bytes[5..7].copy_from_slice(&beacon.control_port.to_le_bytes());
    bytes[7..11].copy_from_slice(&beacon.sample_rate.to_le_bytes());
    bytes
}

/// Parses a beacon datagram. Returns `None` for anything that is not a version-1 MixLink beacon:
/// a short datagram, a wrong magic or a wrong version. It never panics on untrusted input.
///
/// The server only ever sends beacons, so the reader exists to prove the encoding round trip. It is
/// compiled for tests, where it is the counterpart of [encode_beacon].
#[cfg(test)]
pub(crate) fn decode_beacon(bytes: &[u8]) -> Option<Beacon> {
    if bytes.len() < BEACON_LENGTH {
        return None;
    }
    if bytes[0..4] != DISCOVERY_MAGIC {
        return None;
    }
    if bytes[4] != DISCOVERY_VERSION {
        return None;
    }
    let control_port = u16::from_le_bytes([bytes[5], bytes[6]]);
    let sample_rate = u32::from_le_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]);
    Some(Beacon {
        control_port,
        sample_rate,
    })
}

/// Broadcasts the beacon every [BEACON_INTERVAL] until [stopped] turns true, so it starts and stops
/// with the server exactly as the control thread does.
pub(crate) fn spawn_discovery_thread(
    control_port: u16,
    sample_rate: u32,
    stopped: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    let datagram = encode_beacon(Beacon {
        control_port,
        sample_rate,
    });
    let destination = SocketAddr::from(([255, 255, 255, 255], DISCOVERY_PORT));
    Ok(thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            if let Err(error) = socket.send_to(&datagram, destination) {
                eprintln!("discovery beacon send error: {error}");
            }
            // Sleep in short steps so a stop request is honoured promptly instead of waiting out a
            // full interval.
            let mut waited = Duration::ZERO;
            while waited < BEACON_INTERVAL && !stopped.load(Ordering::Relaxed) {
                let step = Duration::from_millis(100).min(BEACON_INTERVAL - waited);
                thread::sleep(step);
                waited += step;
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacon_round_trips_control_port_and_sample_rate() {
        let beacon = Beacon {
            control_port: 50001,
            sample_rate: 48_000,
        };

        assert_eq!(decode_beacon(&encode_beacon(beacon)), Some(beacon));
    }

    #[test]
    fn beacon_encodes_magic_version_and_little_endian_fields() {
        let bytes = encode_beacon(Beacon {
            control_port: 0x1234,
            sample_rate: 0x00AB_CDEF,
        });

        assert_eq!(&bytes[0..4], b"MLNK");
        assert_eq!(bytes[4], DISCOVERY_VERSION);
        assert_eq!(&bytes[5..7], &[0x34, 0x12]);
        assert_eq!(&bytes[7..11], &[0xEF, 0xCD, 0xAB, 0x00]);
    }

    #[test]
    fn a_short_datagram_is_rejected() {
        let bytes = encode_beacon(Beacon {
            control_port: 50001,
            sample_rate: 48_000,
        });

        assert_eq!(decode_beacon(&bytes[..10]), None);
    }

    #[test]
    fn a_wrong_magic_is_rejected() {
        let mut bytes = encode_beacon(Beacon {
            control_port: 50001,
            sample_rate: 48_000,
        });
        bytes[0] = b'X';

        assert_eq!(decode_beacon(&bytes), None);
    }

    #[test]
    fn a_wrong_version_is_rejected() {
        let mut bytes = encode_beacon(Beacon {
            control_port: 50001,
            sample_rate: 48_000,
        });
        bytes[4] = DISCOVERY_VERSION.wrapping_add(1);

        assert_eq!(decode_beacon(&bytes), None);
    }
}
