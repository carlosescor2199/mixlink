use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::targets::TargetRegistry;

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

/// Resolves the local address this machine uses to reach `target`, without sending a single byte.
///
/// `connect` on a UDP socket performs no handshake; it only makes the OS pick the outgoing
/// interface for the peer's route and fix the socket's source address. Reading `local_addr` then
/// reports the interface IP. This is how the beacon learns which interface to broadcast from,
/// instead of letting Windows guess from the routing table for a limited broadcast.
///
/// A target whose route cannot be resolved returns `None`.
fn local_address_for(target: SocketAddr) -> Option<IpAddr> {
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.connect(target).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// The distinct local addresses that can carry the limited broadcast, in first-seen order.
///
/// This is the testable core. [local_address_for] does the socket work and is injected as
/// `resolve`, so the derivation and deduplication are a pure function of the target list.
///
/// Loopback and unspecified addresses are dropped: a socket bound to them cannot put a
/// 255.255.255.255 datagram on a real network. Dropping them is also what lets a
/// default-target-only server (127.0.0.1) fall back to the unbound broadcast instead of sending
/// nothing.
fn derive_local_addresses(
    targets: &[SocketAddr],
    resolve: impl Fn(SocketAddr) -> Option<IpAddr>,
) -> Vec<IpAddr> {
    let mut addresses = Vec::new();
    for &target in targets {
        let Some(address) = resolve(target) else {
            continue;
        };
        if address.is_loopback() || address.is_unspecified() {
            continue;
        }
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses
}

/// The destination whose route stands in for the default route.
///
/// It is never contacted: `connect` on a UDP socket only asks the OS which interface it would use,
/// so no packet leaves the machine and no DNS lookup happens. A public address is used because
/// every host with a default route has one for it, including a LAN-only host behind a gateway.
fn default_route_probe() -> SocketAddr {
    SocketAddr::from(([8, 8, 8, 8], 53))
}

/// The local address of the default route, resolved without sending a byte.
///
/// This is what keeps the beacon alive when no target can derive an interface: the desktop starts
/// with an empty musician list, and the unbound `255.255.255.255` send would leave through
/// whichever adapter Windows prefers (on a machine with WSL, the WSL adapter) instead of the LAN.
fn default_route_address() -> Option<IpAddr> {
    local_address_for(default_route_probe())
}

/// The interfaces the beacon broadcasts from: the routes to the live targets plus the default
/// route, deduplicated, with loopback and unspecified addresses dropped.
///
/// The default route matters most when no target is configured at all: the desktop starts with an
/// empty musician list, and without it the thread falls back to the unbound broadcast, which on a
/// machine with a WSL adapter leaves through the wrong interface.
pub(crate) fn beacon_interfaces(
    targets: &[SocketAddr],
    resolve: impl Fn(SocketAddr) -> Option<IpAddr>,
    resolve_default: impl Fn() -> Option<IpAddr>,
) -> Vec<IpAddr> {
    let mut addresses = derive_local_addresses(targets, resolve);
    if let Some(address) = resolve_default() {
        if !address.is_loopback() && !address.is_unspecified() && !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses
}

/// Builds one broadcast socket per interface address, each bound to that address so the OS cannot
/// pick another one.
///
/// When no interface address is known (or none can be bound), it falls back to a single unbound
/// `0.0.0.0` socket, so the beacon still advertises on the limited broadcast.
fn build_broadcast_senders(addresses: &[IpAddr]) -> Vec<UdpSocket> {
    let mut senders = Vec::new();
    for &address in addresses {
        if !address.is_ipv4() {
            // The beacon destination is the IPv4 limited broadcast, so an IPv6 local address cannot
            // carry it.
            continue;
        }
        match UdpSocket::bind(SocketAddr::new(address, 0)) {
            Ok(socket) => match socket.set_broadcast(true) {
                Ok(()) => senders.push(socket),
                Err(error) => eprintln!("discovery beacon setup error for {address}: {error}"),
            },
            Err(error) => eprintln!("discovery beacon bind error for {address}: {error}"),
        }
    }
    if senders.is_empty() {
        match UdpSocket::bind("0.0.0.0:0") {
            Ok(socket) => {
                if let Err(error) = socket.set_broadcast(true) {
                    eprintln!("discovery beacon setup error: {error}");
                }
                senders.push(socket);
            }
            Err(error) => eprintln!("discovery beacon bind error: {error}"),
        }
    }
    senders
}

/// Broadcasts the beacon every [BEACON_INTERVAL] until [stopped] turns true, so it starts and stops
/// with the server exactly as the control thread does.
///
/// The interfaces are derived fresh on every tick from the live target table and the default route,
/// never from a list captured at startup: the desktop begins with no musicians at all, and a
/// target added or removed at runtime changes which interfaces can carry the broadcast. The fixed
/// 11-byte datagram, its port and its interval are unchanged.
pub(crate) fn spawn_discovery_thread(
    targets: Arc<TargetRegistry>,
    control_port: u16,
    sample_rate: u32,
    stopped: Arc<AtomicBool>,
) -> JoinHandle<()> {
    let datagram = encode_beacon(Beacon {
        control_port,
        sample_rate,
    });
    let destination = SocketAddr::from(([255, 255, 255, 255], DISCOVERY_PORT));
    thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            let live_targets: Vec<SocketAddr> =
                targets.read().iter().map(|entry| entry.address).collect();
            let addresses =
                beacon_interfaces(&live_targets, local_address_for, default_route_address);
            for socket in build_broadcast_senders(&addresses) {
                if let Err(error) = socket.send_to(&datagram, destination) {
                    eprintln!("discovery beacon send error: {error}");
                }
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

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

    #[test]
    fn derives_distinct_local_addresses_from_the_configured_targets() {
        let targets = [
            "192.168.1.10:50000".parse().unwrap(),
            "192.168.1.20:50000".parse().unwrap(),
            "10.0.0.5:50000".parse().unwrap(),
        ];
        // Two targets share the LAN, one is on another network: three targets, two distinct
        // outgoing interfaces.
        let resolve = |target: SocketAddr| match target.ip() {
            IpAddr::V4(ip) if ip.octets()[0] == 192 => Some("192.168.1.27".parse().unwrap()),
            _ => Some("10.0.0.2".parse().unwrap()),
        };

        let addresses = derive_local_addresses(&targets, resolve);

        assert_eq!(
            addresses,
            vec![
                "192.168.1.27".parse::<IpAddr>().unwrap(),
                "10.0.0.2".parse::<IpAddr>().unwrap(),
            ]
        );
    }

    #[test]
    fn drops_loopback_and_unresolvable_targets_from_the_derived_addresses() {
        let targets = [
            "127.0.0.1:50000".parse().unwrap(),
            "192.168.1.10:50000".parse().unwrap(),
            "203.0.113.9:50000".parse().unwrap(),
        ];
        let resolve = |target: SocketAddr| match target.ip() {
            IpAddr::V4(ip) if ip.is_loopback() => Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            IpAddr::V4(ip) if ip.octets()[0] == 192 => Some("192.168.1.27".parse().unwrap()),
            _ => None,
        };

        let addresses = derive_local_addresses(&targets, resolve);

        assert_eq!(addresses, vec!["192.168.1.27".parse::<IpAddr>().unwrap()]);
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("test IP should parse")
    }

    /// The desktop case: the musician list is empty, so only the default route can keep the beacon
    /// on the live LAN instead of the unbound broadcast Windows routes through the WSL adapter.
    #[test]
    fn the_default_route_carries_the_beacon_when_no_targets_are_configured() {
        let addresses = beacon_interfaces(
            &[],
            |_| unreachable!("there are no targets to resolve"),
            || Some(ip("192.168.1.27")),
        );

        assert_eq!(addresses, vec![ip("192.168.1.27")]);
    }

    #[test]
    fn the_default_route_is_added_after_the_target_interfaces() {
        let targets = ["192.168.1.50:50000".parse().unwrap()];
        let resolve = |_target: SocketAddr| Some(ip("10.0.0.2"));

        let addresses = beacon_interfaces(&targets, resolve, || Some(ip("192.168.1.27")));

        assert_eq!(addresses, vec![ip("10.0.0.2"), ip("192.168.1.27")]);
    }

    #[test]
    fn the_default_route_is_not_duplicated_when_a_target_already_uses_it() {
        let targets = ["192.168.1.50:50000".parse().unwrap()];
        let resolve = |_target: SocketAddr| Some(ip("192.168.1.27"));

        let addresses = beacon_interfaces(&targets, resolve, || Some(ip("192.168.1.27")));

        assert_eq!(addresses, vec![ip("192.168.1.27")]);
    }

    #[test]
    fn a_loopback_or_unspecified_default_route_is_dropped() {
        assert!(beacon_interfaces(&[], |_| None, || Some(ip("127.0.0.1"))).is_empty());
        assert!(beacon_interfaces(&[], |_| None, || Some(ip("0.0.0.0"))).is_empty());
    }

    #[test]
    fn an_unresolvable_default_route_leaves_only_the_target_interfaces() {
        let targets = ["192.168.1.50:50000".parse().unwrap()];
        let resolve = |_target: SocketAddr| Some(ip("10.0.0.2"));

        let addresses = beacon_interfaces(&targets, resolve, || None);

        assert_eq!(addresses, vec![ip("10.0.0.2")]);
    }

    #[test]
    fn one_sender_is_bound_per_interface_address() {
        let senders = build_broadcast_senders(&[ip("127.0.0.1")]);

        assert_eq!(senders.len(), 1);
        assert_eq!(senders[0].local_addr().unwrap().ip(), ip("127.0.0.1"));
    }

    #[test]
    fn the_unbound_fallback_socket_is_used_only_when_no_interface_is_known() {
        let senders = build_broadcast_senders(&[]);

        assert_eq!(senders.len(), 1);
        assert!(senders[0].local_addr().unwrap().ip().is_unspecified());
    }
}
