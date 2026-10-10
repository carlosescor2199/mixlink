use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cpal::traits::{HostTrait, StreamTrait};

use crate::build_mix_states;
use crate::capture::{build_input_stream, select_device, select_input_config, TARGET_SAMPLE_RATE};
use crate::cli::{resolve_targets, validate_groups, EngineConfig};
use crate::control::{spawn_control_thread, ControlPeers};
use crate::discovery::spawn_discovery_thread;
use crate::mix::{MixState, MixValues};
use crate::network::{spawn_network_thread, PacketStats, TargetCounters};
use crate::protocol::AudioPacket;

const CHANNEL_CAPACITY: usize = 8;

/// Something that changed while the engine was running.
///
/// The engine emits these over an [EngineHandle::subscribe] channel. Connect and disconnect are the
/// state a UI cannot cheaply poll for, and a mix change is the confirmation that a client's command
/// was applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    /// A control channel became connected for this peer.
    ClientConnected { address: SocketAddr },
    /// The last control channel for this peer's IP closed.
    ClientDisconnected { address: SocketAddr },
    /// A control command changed this peer's mix.
    MixChanged { address: SocketAddr },
}

/// The capture format the engine selected, as plain strings so a UI can render it without a cpal
/// dependency. [Self::sample_rate] is the device's rate; the packaged rate is always 48 kHz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureFormat {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: String,
    pub buffer_size: String,
}

/// One musician's send counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MusicianCounters {
    pub packets_sent: u64,
    pub packets_discarded: u64,
}

/// One musician: a UDP target joined with its control channel and its mix.
///
/// This is the row the UI shows. The engine performs the join, because the target list and the
/// control peers are two views of the same person and letting the UI infer the relationship from
/// two lists lets them disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MusicianStatus {
    pub address: SocketAddr,
    pub control_connected: bool,
    pub mix: MixValues,
    pub counters: MusicianCounters,
}

/// A snapshot of everything the engine can report without blocking the audio path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineStatus {
    pub device_name: String,
    pub capture_format: CaptureFormat,
    pub control_port: u16,
    /// The rate the engine packages at; the device format above may be a multiple of this.
    pub sample_rate: u32,
    /// Source channel count after capture, which fixes the valid group channel range.
    pub source_channels: u8,
    pub stopped: bool,
    pub samples_received: u64,
    pub packets_sent: u64,
    pub packets_discarded: u64,
    pub musicians: Vec<MusicianStatus>,
}

/// A fan-out of [EngineEvent] to any number of observers.
///
/// A channel was chosen over a listener trait: it decouples the engine threads from the consumer,
/// needs no `Fn + Send + Sync` callback and no reentrancy care, and reuses `std::sync::mpsc` that
/// the crate already depends on. Each [EventBus::subscribe] hands out an independent receiver.
#[derive(Clone, Default)]
pub(crate) struct EventBus {
    subscribers: Arc<Mutex<Vec<Sender<EngineEvent>>>>,
}

impl EventBus {
    pub(crate) fn subscribe(&self) -> Receiver<EngineEvent> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.subscribers
            .lock()
            .expect("event subscribers mutex poisoned")
            .push(sender);
        receiver
    }

    pub(crate) fn emit(&self, event: EngineEvent) {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("event subscribers mutex poisoned");
        subscribers.retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }
}

/// Joins the per-target state into one row per musician.
///
/// Rows are driven by the target list, so a target with no control connection is still reported.
/// Per-target data that is missing falls back to its neutral value rather than dropping the row.
pub(crate) fn join_musicians(
    targets: &[SocketAddr],
    connected: &HashSet<IpAddr>,
    counters: &HashMap<IpAddr, MusicianCounters>,
    mixes: &HashMap<IpAddr, MixValues>,
) -> Vec<MusicianStatus> {
    targets
        .iter()
        .map(|address| {
            let ip = address.ip();
            MusicianStatus {
                address: *address,
                control_connected: connected.contains(&ip),
                mix: mixes.get(&ip).copied().unwrap_or_default(),
                counters: counters.get(&ip).copied().unwrap_or_default(),
            }
        })
        .collect()
}

/// The live engine. Dropping it does not stop the engine; call [EngineHandle::stop].
pub struct EngineHandle {
    device_name: String,
    capture_format: CaptureFormat,
    control_port: u16,
    source_channels: u8,
    targets: Vec<SocketAddr>,
    stopped: Arc<AtomicBool>,
    samples_seen: Arc<AtomicU64>,
    packet_stats: Arc<PacketStats>,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    target_counters: Arc<HashMap<IpAddr, TargetCounters>>,
    peers: ControlPeers,
    events: EventBus,
    stream: Option<cpal::Stream>,
    packet_sender: Option<SyncSender<AudioPacket>>,
    network_thread: Option<JoinHandle<()>>,
    control_thread: Option<JoinHandle<()>>,
    discovery_thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// The flag the caller sets to stop the engine, for use from a signal handler.
    pub fn stop_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stopped)
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Subscribes to the engine's events. Each call returns a fresh receiver.
    pub fn subscribe(&self) -> Receiver<EngineEvent> {
        self.events.subscribe()
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn capture_format(&self) -> &CaptureFormat {
        &self.capture_format
    }

    pub fn control_port(&self) -> u16 {
        self.control_port
    }

    /// A snapshot of the current state, including the joined per-musician view.
    pub fn status(&self) -> EngineStatus {
        let connected: HashSet<IpAddr> = self
            .peers
            .lock()
            .expect("control peer registry poisoned")
            .keys()
            .copied()
            .collect();
        let counters: HashMap<IpAddr, MusicianCounters> = self
            .target_counters
            .iter()
            .map(|(ip, counter)| {
                (
                    *ip,
                    MusicianCounters {
                        packets_sent: counter.sent.load(Ordering::Relaxed),
                        packets_discarded: counter.discarded.load(Ordering::Relaxed),
                    },
                )
            })
            .collect();
        let mixes: HashMap<IpAddr, MixValues> = self
            .mix_states
            .iter()
            .map(|(ip, state)| (*ip, state.snapshot()))
            .collect();
        EngineStatus {
            device_name: self.device_name.clone(),
            capture_format: self.capture_format.clone(),
            control_port: self.control_port,
            sample_rate: TARGET_SAMPLE_RATE,
            source_channels: self.source_channels,
            stopped: self.is_stopped(),
            samples_received: self.samples_seen.load(Ordering::Relaxed),
            packets_sent: self.packet_stats.sent.load(Ordering::Relaxed),
            packets_discarded: self.packet_stats.discarded.load(Ordering::Relaxed),
            musicians: join_musicians(&self.targets, &connected, &counters, &mixes),
        }
    }

    /// Stops capture and joins the engine threads, the same clean shutdown Ctrl+C triggers.
    pub fn stop(&mut self) -> Result<(), Box<dyn Error>> {
        self.stopped.store(true, Ordering::SeqCst);
        // Stop producing before draining: drop the capture stream first, then the sender, so the
        // network thread sees the channel close and exits.
        self.stream = None;
        self.packet_sender = None;
        if let Some(thread) = self.network_thread.take() {
            thread.join().map_err(|_| "UDP network thread panicked")?;
        }
        if let Some(thread) = self.control_thread.take() {
            thread
                .join()
                .map_err(|_| "control WebSocket thread panicked")?;
        }
        if let Some(thread) = self.discovery_thread.take() {
            thread
                .join()
                .map_err(|_| "discovery beacon thread panicked")?;
        }
        Ok(())
    }
}

fn build_target_counters(targets: &[SocketAddr]) -> Arc<HashMap<IpAddr, TargetCounters>> {
    Arc::new(
        targets
            .iter()
            .map(|target| (target.ip(), TargetCounters::default()))
            .collect(),
    )
}

/// Starts the engine and returns its handle, or `None` when no input device is available.
///
/// `None` is not an error: the CLI prints a friendly line and exits cleanly for a machine with no
/// audio interface, and a UI can treat it the same way. Every other startup failure is an error.
///
/// The narrative the CLI prints travels with the engine, so the command-line output is unchanged
/// even though the binary no longer owns the wiring.
pub fn start(config: EngineConfig) -> Result<Option<EngineHandle>, Box<dyn Error>> {
    let targets = resolve_targets(&config.targets)?;
    println!("UDP targets:");
    for target in &targets {
        println!("  {target}");
    }
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|error| format!("could not enumerate input devices: {error}"))?
        .collect::<Vec<_>>();

    if devices.is_empty() {
        println!("No input device is available. Connect an audio interface and try again.");
        return Ok(None);
    }

    println!("Available input devices:");
    for (index, device) in devices.iter().enumerate() {
        let name = device.to_string();
        println!("  {}. {name}", index + 1);
    }

    let device = select_device(devices, config.device_filter.as_deref())?;
    let device_name = device.to_string();
    let supported_config = select_input_config(&device)?;

    println!("Selected input: {device_name}");
    println!(
        "Capture format: channels={}, sample rate={} Hz, buffer={:?}, sample format={:?}",
        supported_config.channels(),
        supported_config.sample_rate(),
        supported_config.buffer_size(),
        supported_config.sample_format()
    );
    if supported_config.sample_rate() != TARGET_SAMPLE_RATE {
        println!(
            "Input is {} Hz; applying integer decimation to {TARGET_SAMPLE_RATE} Hz before packaging.",
            supported_config.sample_rate()
        );
    }

    let stopped = Arc::new(AtomicBool::new(false));
    let source_channels = supported_config.channels().min(u16::from(u8::MAX)) as u8;
    let group_layout = Arc::new(validate_groups(
        &config.groups,
        usize::from(source_channels),
    )?);
    let mix_states = build_mix_states(&targets, &group_layout)?;
    let target_counters = build_target_counters(&targets);
    let peers: ControlPeers = Arc::new(Mutex::new(HashMap::new()));
    let events = EventBus::default();

    let control_thread = spawn_control_thread(
        config.control_port,
        Arc::clone(&mix_states),
        Arc::clone(&stopped),
        source_channels,
        Arc::clone(&group_layout),
        Arc::clone(&peers),
        events.clone(),
    );
    let discovery_thread = spawn_discovery_thread(
        &targets,
        config.control_port,
        TARGET_SAMPLE_RATE,
        Arc::clone(&stopped),
    )?;
    let (packet_sender, packet_receiver) = sync_channel(CHANNEL_CAPACITY);
    let packet_stats = Arc::new(PacketStats {
        sent: AtomicU64::new(0),
        discarded: AtomicU64::new(0),
    });
    let network_thread = spawn_network_thread(
        packet_receiver,
        targets.clone(),
        Arc::clone(&mix_states),
        Arc::clone(&packet_stats),
        Arc::clone(&target_counters),
    )?;
    let samples_seen = Arc::new(AtomicU64::new(0));
    let stream = build_input_stream(
        &device,
        &supported_config,
        packet_sender.clone(),
        Arc::clone(&samples_seen),
        Arc::clone(&packet_stats),
    )?;
    stream
        .play()
        .map_err(|error| format!("could not start input capture: {error}"))?;

    let capture_format = CaptureFormat {
        channels: supported_config.channels(),
        sample_rate: supported_config.sample_rate(),
        sample_format: format!("{:?}", supported_config.sample_format()),
        buffer_size: format!("{:?}", supported_config.buffer_size()),
    };

    Ok(Some(EngineHandle {
        device_name,
        capture_format,
        control_port: config.control_port,
        source_channels,
        targets,
        stopped,
        samples_seen,
        packet_stats,
        mix_states,
        target_counters,
        peers,
        events,
        stream: Some(stream),
        packet_sender: Some(packet_sender),
        network_thread: Some(network_thread),
        control_thread: Some(control_thread),
        discovery_thread: Some(discovery_thread),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("test IP should parse")
    }

    #[test]
    fn joins_targets_and_connected_peer_ips_into_one_row_per_musician() {
        let targets = vec![
            "192.168.1.3:50000".parse().unwrap(),
            "192.168.1.4:50000".parse().unwrap(),
            "192.168.1.5:50000".parse().unwrap(),
        ];
        let connected: HashSet<IpAddr> =
            [ip("192.168.1.3"), ip("192.168.1.5")].into_iter().collect();

        let mut counters = HashMap::new();
        counters.insert(
            ip("192.168.1.3"),
            MusicianCounters {
                packets_sent: 10,
                packets_discarded: 1,
            },
        );
        counters.insert(
            ip("192.168.1.4"),
            MusicianCounters {
                packets_sent: 20,
                packets_discarded: 2,
            },
        );

        let mut mixes = HashMap::new();
        mixes.insert(
            ip("192.168.1.3"),
            MixValues {
                volume_percent: 42,
                ..MixValues::default()
            },
        );

        let musicians = join_musicians(&targets, &connected, &counters, &mixes);

        assert_eq!(musicians.len(), 3);

        let connected_with_mix = &musicians[0];
        assert_eq!(connected_with_mix.address, targets[0]);
        assert!(connected_with_mix.control_connected);
        assert_eq!(connected_with_mix.mix.volume_percent, 42);
        assert_eq!(
            connected_with_mix.counters,
            MusicianCounters {
                packets_sent: 10,
                packets_discarded: 1,
            }
        );

        let disconnected = &musicians[1];
        assert_eq!(disconnected.address, targets[1]);
        assert!(!disconnected.control_connected);
        assert_eq!(disconnected.mix, MixValues::default());
        assert_eq!(
            disconnected.counters,
            MusicianCounters {
                packets_sent: 20,
                packets_discarded: 2,
            }
        );

        let connected_without_data = &musicians[2];
        assert_eq!(connected_without_data.address, targets[2]);
        assert!(connected_without_data.control_connected);
        assert_eq!(connected_without_data.counters, MusicianCounters::default());
        assert_eq!(connected_without_data.mix, MixValues::default());
    }
}
