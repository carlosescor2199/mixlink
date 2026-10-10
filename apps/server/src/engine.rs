use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cpal::traits::{HostTrait, StreamTrait};

use crate::capture::{
    build_input_stream, select_device, select_device_by_name, select_input_config,
    TARGET_SAMPLE_RATE,
};
use crate::cli::{resolve_targets, validate_groups, EngineConfig, GroupDefinition};
use crate::control::{spawn_control_thread, ConfigState, ControlPeers};
use crate::discovery::spawn_discovery_thread;
use crate::mix::{Group, GroupLayout, MixValues, MAX_MIX_CHANNELS};
use crate::network::{spawn_network_thread, PacketStats};
use crate::protocol::AudioPacket;
use crate::targets::TargetRegistry;

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

/// One configured group of source channels, as the observation API exposes it.
///
/// Channels are 0-based source indices, the same numbering the protocol's `config` message and the
/// mixer use. Membership is fixed at startup and is never rewritten by a device switch; instead
/// [Self::invalid] reports when the current device no longer has one of the channels the group names,
/// so a UI can warn without the engineer's configuration being destroyed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupStatus {
    pub name: String,
    pub channels: Vec<usize>,
    /// True when the current capture device is missing a channel this group names. An invalid group
    /// is reported and kept, but is excluded from the mixer so it cannot affect audio.
    pub invalid: bool,
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
    /// The configured group layout, in the order the engineer defined it.
    pub groups: Vec<GroupStatus>,
    /// One 0-100 source-channel peak per channel, index-aligned with the source channels.
    ///
    /// The value is measured before mixing, so it describes the input rather than any one
    /// musician's output, and it carries a decay from one read to the next so a poller at a few
    /// hertz sees a level that rises immediately and falls smoothly.
    pub channel_levels: Vec<u8>,
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

/// The fixed decay applied to a held peak each time the level is read.
///
/// A read happens once per UI poll (a few hertz), so an integer multiply by 7/8 per read makes the
/// meter fall smoothly between polls. It is deliberately not a time source: the audio callback
/// must stay free of clock reads, and a poll-rate-relative falloff is enough for a level that only
/// has to look alive.
const LEVEL_DECAY_NUMERATOR: u8 = 7;
const LEVEL_DECAY_DENOMINATOR: u8 = 8;

/// Converts a full-scale-relative sample magnitude into a 0-100 peak.
///
/// Linear in amplitude, not decibels: it costs one integer multiply without a logarithm on the
/// audio thread, and the task only needs a cheap "is there signal here" reading. `i16::MAX`
/// (`32767`) maps to 100 and silence maps to 0.
fn peak_to_level(peak: u16) -> u8 {
    ((u32::from(peak) * 100) / 32767).min(100) as u8
}

/// Per-source-channel peak levels, measured on the audio thread and held for the UI to poll.
///
/// The audio callback raises each channel's held peak with [observe], which is allocation-free and
/// does one max-scan over the already-decimated samples. Reading with [decayed] applies the decay,
/// so the cost of decaying sits on the polling path, never on the audio thread.
pub(crate) struct LevelMeter {
    levels: [AtomicU8; MAX_MIX_CHANNELS],
}

impl LevelMeter {
    pub(crate) fn new() -> Self {
        Self {
            levels: std::array::from_fn(|_| AtomicU8::new(0)),
        }
    }

    /// Raises each channel's held peak to this block's peak. Called from the audio callback.
    ///
    /// Memory order is `Relaxed` on both sides: a meter reading that lags one callback is
    /// irrelevant, and the value carries no other state, so no synchronisation is warranted. The
    /// scan is one pass over `samples` with no allocation and no clock or logarithm.
    pub(crate) fn observe(&self, samples: &[i16], channels: usize) {
        if channels == 0 || channels > MAX_MIX_CHANNELS || samples.len() % channels != 0 {
            return;
        }
        let frames = samples.len() / channels;
        for (channel, level) in self.levels.iter().enumerate().take(channels) {
            let mut peak = 0u16;
            for frame in 0..frames {
                let magnitude = samples[frame * channels + channel].unsigned_abs();
                if magnitude > peak {
                    peak = magnitude;
                }
            }
            level.fetch_max(peak_to_level(peak), Ordering::Relaxed);
        }
    }

    /// Reads the held peaks and decays them for the next read.
    ///
    /// Returns one level per source channel, in channel order. `fetch_update` retries against a
    /// concurrent [observe] instead of clobbering a peak the audio thread raised in between, so a
    /// busy poll never eats a transient. Channels beyond the metered maximum read as zero, matching
    /// the mixer, which also ignores them.
    pub(crate) fn decayed(&self, channels: usize) -> Vec<u8> {
        (0..channels)
            .map(|channel| match self.levels.get(channel) {
                Some(level) => level
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                        // Widened so a full-scale 100 times the numerator cannot overflow a u8.
                        Some(
                            (u16::from(value) * u16::from(LEVEL_DECAY_NUMERATOR)
                                / u16::from(LEVEL_DECAY_DENOMINATOR))
                                as u8,
                        )
                    })
                    .unwrap_or(0),
                None => 0,
            })
            .collect()
    }
}

/// Snapshots the fixed group layout into the observation-friendly [GroupStatus] list.
///
/// Used at startup, where [validate_groups] has already proven every group valid.
fn group_statuses(layout: &GroupLayout) -> Vec<GroupStatus> {
    layout
        .groups()
        .iter()
        .map(|group| GroupStatus {
            name: group.name.clone(),
            channels: group.channels.clone(),
            invalid: false,
        })
        .collect()
}

/// The result of re-checking the engineer's groups against a channel count.
///
/// `statuses` preserves the groups for reporting, with invalidity flagged. `mixer_layout` is the
/// membership the mixer may act on: an invalid group contributes no channels, so it cannot affect
/// audio even though it is neither deleted nor modified.
pub(crate) struct GroupRevalidation {
    pub(crate) statuses: Vec<GroupStatus>,
    pub(crate) mixer_layout: GroupLayout,
}

/// Re-checks the engineer's fixed groups against a channel count without failing or mutating them.
///
/// Unlike [validate_groups], which rejects an out-of-range group at startup, this is the lenient
/// post-switch check: a channel the new device does not have marks the whole group invalid but keeps
/// its definition, so switching back restores it. Definitions are read only.
pub(crate) fn revalidate_groups(
    definitions: &[GroupDefinition],
    channel_count: usize,
) -> GroupRevalidation {
    let mut statuses = Vec::with_capacity(definitions.len());
    let mut mixer_groups = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let invalid = definition
            .channels
            .iter()
            .any(|&channel| channel == 0 || channel > channel_count);
        let channels: Vec<usize> = definition
            .channels
            .iter()
            .map(|&channel| channel.saturating_sub(1))
            .collect();
        statuses.push(GroupStatus {
            name: definition.name.clone(),
            channels: channels.clone(),
            invalid,
        });
        // Invalid groups keep their slot in the list so group indices line up with `group_levels`,
        // but map no channel; the mixer then cannot apply their level to anything.
        mixer_groups.push(Group {
            name: definition.name.clone(),
            channels: if invalid { Vec::new() } else { channels },
        });
    }
    GroupRevalidation {
        statuses,
        mixer_layout: GroupLayout::new(mixer_groups),
    }
}

/// Everything a successful device switch changes, computed before anything is mutated.
pub(crate) struct DeviceSwitchPlan {
    pub(crate) source_channels: u8,
    pub(crate) groups: Vec<GroupStatus>,
    pub(crate) mixer_layout: GroupLayout,
    pub(crate) resend_config: bool,
}

/// Plans the state change for a capture device that now has `new_channels` channels.
///
/// Pure and non-mutating: the previous group definitions are read, the groups are re-validated, and
/// `resend_config` is set only when the channel count actually changed, which is when connected
/// clients must rebuild.
pub(crate) fn plan_device_switch(
    current_channels: u8,
    definitions: &[GroupDefinition],
    new_channels: u8,
) -> DeviceSwitchPlan {
    let revalidation = revalidate_groups(definitions, usize::from(new_channels));
    DeviceSwitchPlan {
        source_channels: new_channels,
        groups: revalidation.statuses,
        mixer_layout: revalidation.mixer_layout,
        resend_config: new_channels != current_channels,
    }
}

/// Turns a fallible capture rebuild into the plan to commit, or leaves the previous state alone.
///
/// The rebuild touches real hardware and can fail; keeping the conversion to a plan in a pure
/// function is what makes the "a failed switch preserves the previous state" rule testable without
/// an audio device.
pub(crate) fn plan_switch_from_attempt(
    current_channels: u8,
    definitions: &[GroupDefinition],
    attempt: Result<u16, Box<dyn Error>>,
) -> Result<DeviceSwitchPlan, Box<dyn Error>> {
    let new_channels = attempt?.min(u16::from(u8::MAX)) as u8;
    Ok(plan_device_switch(
        current_channels,
        definitions,
        new_channels,
    ))
}

/// The live engine. Dropping it does not stop the engine; call [EngineHandle::stop].
pub struct EngineHandle {
    device_name: String,
    capture_format: CaptureFormat,
    control_port: u16,
    source_channels: u8,
    /// The engineer's groups exactly as configured, kept so a device switch can re-validate them
    /// against the new channel count without ever rewriting them.
    group_definitions: Vec<GroupDefinition>,
    groups: Vec<GroupStatus>,
    levels: Arc<LevelMeter>,
    /// The live routing table, shared with the network and control threads so targets can be added
    /// and removed while both keep running.
    targets: Arc<TargetRegistry>,
    /// The group layout the last committed device switch produced, used to give a newly added
    /// target the same channel-to-group membership the existing mix states carry.
    current_layout: GroupLayout,
    stopped: Arc<AtomicBool>,
    samples_seen: Arc<AtomicU64>,
    /// The packet sequence shared with whichever capture stream is currently running, so a switch
    /// continues the numbering instead of restarting it.
    sequence: Arc<AtomicU64>,
    packet_stats: Arc<PacketStats>,
    peers: ControlPeers,
    config_state: Arc<ConfigState>,
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
        let entries = self.targets.read();
        let targets: Vec<SocketAddr> = entries.iter().map(|entry| entry.address).collect();
        let counters: HashMap<IpAddr, MusicianCounters> = entries
            .iter()
            .map(|entry| {
                (
                    entry.address.ip(),
                    MusicianCounters {
                        packets_sent: entry.counters.sent.load(Ordering::Relaxed),
                        packets_discarded: entry.counters.discarded.load(Ordering::Relaxed),
                    },
                )
            })
            .collect();
        let mixes: HashMap<IpAddr, MixValues> = entries
            .iter()
            .map(|entry| (entry.address.ip(), entry.mix_state.snapshot()))
            .collect();
        EngineStatus {
            device_name: self.device_name.clone(),
            capture_format: self.capture_format.clone(),
            control_port: self.control_port,
            sample_rate: TARGET_SAMPLE_RATE,
            source_channels: self.source_channels,
            groups: self.groups.clone(),
            channel_levels: self.levels.decayed(usize::from(self.source_channels)),
            stopped: self.is_stopped(),
            samples_received: self.samples_seen.load(Ordering::Relaxed),
            packets_sent: self.packet_stats.sent.load(Ordering::Relaxed),
            packets_discarded: self.packet_stats.discarded.load(Ordering::Relaxed),
            musicians: join_musicians(&targets, &connected, &counters, &mixes),
        }
    }

    /// Switches the input device while the rest of the engine keeps running.
    ///
    /// Only the capture stream is rebuilt: the UDP network thread, the control WebSocket server and
    /// the discovery beacon are untouched, and the packet sequence continues from where it was. The
    /// new stream is built and started before the old one is dropped, so a device that has vanished
    /// or a format that cannot be opened leaves the previous capture exactly as it was and returns
    /// the error. On success, groups are re-validated (never rewritten) and connected clients are
    /// told to rebuild when the channel count changed.
    pub fn switch_device(&mut self, filter: Option<&str>) -> Result<CaptureFormat, Box<dyn Error>> {
        if self.is_stopped() {
            return Err("the engine is stopping".into());
        }
        let device = {
            let host = cpal::default_host();
            let devices = host
                .input_devices()
                .map_err(|error| format!("could not enumerate input devices: {error}"))?
                .collect::<Vec<_>>();
            match filter {
                Some(name) => select_device_by_name(devices, name)?,
                None => select_device(devices, None)?,
            }
        };
        let supported_config = select_input_config(&device)?;
        let packet_sender = self.packet_sender.clone().ok_or("the engine is stopping")?;

        // Build and start the replacement before touching the current capture, so any failure below
        // is a no-op for the running session.
        let new_stream = build_input_stream(
            &device,
            &supported_config,
            packet_sender,
            Arc::clone(&self.samples_seen),
            Arc::clone(&self.packet_stats),
            Arc::clone(&self.levels),
            Arc::clone(&self.sequence),
        )?;
        new_stream
            .play()
            .map_err(|error| format!("could not start input capture: {error}"))?;

        let plan = plan_switch_from_attempt(
            self.source_channels,
            &self.group_definitions,
            Ok(supported_config.channels()),
        )?;

        // Commit: the new capture is confirmed working.
        self.stream = Some(new_stream);
        self.device_name = device.to_string();
        self.capture_format = CaptureFormat {
            channels: supported_config.channels(),
            sample_rate: supported_config.sample_rate(),
            sample_format: format!("{:?}", supported_config.sample_format()),
            buffer_size: format!("{:?}", supported_config.buffer_size()),
        };
        self.source_channels = plan.source_channels;
        self.groups = plan.groups;
        for entry in self.targets.read().iter() {
            entry.mix_state.set_group_layout(&plan.mixer_layout);
        }
        self.current_layout = plan.mixer_layout;
        if plan.resend_config {
            self.config_state.set_channels(plan.source_channels);
        }
        Ok(self.capture_format.clone())
    }

    /// Adds a UDP target while the engine runs.
    ///
    /// The value is resolved and validated before anything changes; a value that cannot be resolved,
    /// or whose IP is already configured, is rejected with an error naming the value and the table
    /// is left exactly as it was. On success the target joins the live routing table with a neutral
    /// mix carrying the current group layout and zero counters, and the next packet is sent to it.
    /// The capture stream, the UDP network thread, the control server and the discovery beacon all
    /// keep running, exactly as they do across a device switch.
    pub fn add_target(&self, value: &str) -> Result<SocketAddr, Box<dyn Error>> {
        if self.is_stopped() {
            return Err("the engine is stopping".into());
        }
        self.targets
            .add(value, &self.current_layout)
            .map_err(|error| -> Box<dyn Error> { error.into() })
    }

    /// Removes a UDP target while the engine runs.
    ///
    /// ## What happens to the removed musician
    ///
    /// - Their audio stops immediately: the address leaves the routing table, so the next packet is
    ///   sent to the remaining targets only. The bound is one packet interval, tens of milliseconds
    ///   at 48 kHz packaging, not a restart or a timeout.
    /// - Their control channel is **not** forcibly closed. The connection stays open and the next
    ///   `mix` message is answered with the existing `no UDP target configured for client IP`
    ///   error, which the Android client already surfaces. Nothing is silently dropped and no new
    ///   protocol message is introduced: the address is simply no longer in the routing table,
    ///   which is the same answer an unknown IP has always received.
    /// - No other target is disturbed: every other entry keeps its mix state, its counters and its
    ///   stream, and the control server, the discovery beacon and the capture stream keep running.
    pub fn remove_target(&self, value: &str) -> Result<SocketAddr, Box<dyn Error>> {
        if self.is_stopped() {
            return Err("the engine is stopping".into());
        }
        self.targets
            .remove(value)
            .map_err(|error| -> Box<dyn Error> { error.into() })
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
    let target_registry = Arc::new(TargetRegistry::new(&targets, &group_layout)?);
    let peers: ControlPeers = Arc::new(Mutex::new(HashMap::new()));
    let config_state = Arc::new(ConfigState::new(source_channels));
    let events = EventBus::default();

    let control_thread = spawn_control_thread(
        config.control_port,
        Arc::clone(&target_registry),
        Arc::clone(&stopped),
        Arc::clone(&config_state),
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
        Arc::clone(&target_registry),
        Arc::clone(&packet_stats),
    )?;
    let samples_seen = Arc::new(AtomicU64::new(0));
    let levels = Arc::new(LevelMeter::new());
    let sequence = Arc::new(AtomicU64::new(0));
    let stream = build_input_stream(
        &device,
        &supported_config,
        packet_sender.clone(),
        Arc::clone(&samples_seen),
        Arc::clone(&packet_stats),
        Arc::clone(&levels),
        Arc::clone(&sequence),
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
        group_definitions: config.groups,
        groups: group_statuses(&group_layout),
        levels,
        targets: target_registry,
        current_layout: (*group_layout).clone(),
        stopped,
        samples_seen,
        sequence,
        packet_stats,
        peers,
        config_state,
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
    use crate::cli::GroupDefinition;

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

    #[test]
    fn peak_to_level_maps_silence_and_full_scale() {
        assert_eq!(peak_to_level(0), 0);
        assert_eq!(peak_to_level(16_384), 50);
        assert_eq!(peak_to_level(32_767), 100);
        assert_eq!(peak_to_level(i16::MIN.unsigned_abs()), 100);
    }

    #[test]
    fn observe_records_a_peak_per_source_channel() {
        let meter = LevelMeter::new();

        // Two frames, two channels: channel 0 peaks at half scale, channel 1 at full scale.
        meter.observe(&[16_384, 32_767, -16_384, -1], 2);

        assert_eq!(meter.decayed(2), vec![50, 100]);
    }

    #[test]
    fn a_held_peak_decays_between_reads_and_a_new_block_re_raises_it() {
        let meter = LevelMeter::new();

        meter.observe(&[32_767, 32_767], 2);
        assert_eq!(meter.decayed(2), vec![100, 100]);
        // No new audio: the held peak falls on the next read.
        assert_eq!(meter.decayed(2), vec![87, 87]);
        // A fresh block raises it straight back to the block peak.
        meter.observe(&[32_767, 32_767], 2);
        assert_eq!(meter.decayed(2), vec![100, 100]);
    }

    #[test]
    fn observe_ignores_incomplete_frames_and_zero_channels() {
        let meter = LevelMeter::new();

        meter.observe(&[1, 2, 3], 2);
        meter.observe(&[], 0);

        assert_eq!(meter.decayed(2), vec![0, 0]);
    }

    #[test]
    fn group_statuses_carry_names_and_zero_based_channels() {
        use crate::mix::Group;

        let layout = GroupLayout::new(vec![Group {
            name: "Drums".to_owned(),
            channels: vec![0, 1],
        }]);

        assert_eq!(
            group_statuses(&layout),
            vec![GroupStatus {
                name: "Drums".to_owned(),
                channels: vec![0, 1],
                invalid: false,
            }]
        );
    }

    #[test]
    fn revalidating_groups_reports_out_of_range_groups_without_changing_them() {
        let definitions = vec![
            GroupDefinition {
                name: "Drums".to_owned(),
                channels: vec![1, 2],
            },
            GroupDefinition {
                name: "Vocals".to_owned(),
                channels: vec![5],
            },
        ];

        let revalidation = revalidate_groups(&definitions, 2);

        assert_eq!(revalidation.statuses.len(), 2);
        assert_eq!(revalidation.statuses[0].name, "Drums");
        assert_eq!(revalidation.statuses[0].channels, vec![0, 1]);
        assert!(!revalidation.statuses[0].invalid);
        assert_eq!(revalidation.statuses[1].name, "Vocals");
        assert!(revalidation.statuses[1].invalid);
        // The engineer's definitions are read, never rewritten.
        assert_eq!(definitions[1].channels, vec![5]);
    }

    #[test]
    fn an_invalid_group_is_excluded_from_the_mixer_but_kept_for_reporting() {
        let definitions = vec![
            GroupDefinition {
                name: "Drums".to_owned(),
                channels: vec![1, 2],
            },
            GroupDefinition {
                name: "Vocals".to_owned(),
                channels: vec![3, 4],
            },
        ];

        // A two-channel device: "Vocals" names channels 3 and 4, so it is invalid.
        let revalidation = revalidate_groups(&definitions, 2);
        let mapping = revalidation.mixer_layout.channel_group();

        assert_eq!(mapping[0], Some(0));
        assert_eq!(mapping[1], Some(0));
        assert_eq!(mapping[2], None);
        assert_eq!(mapping[3], None);
        assert!(revalidation.statuses[1].invalid);
        assert_eq!(revalidation.statuses[1].channels, vec![2, 3]);
    }

    #[test]
    fn a_smaller_device_invalidates_a_group_and_switching_back_restores_it() {
        let definitions = vec![GroupDefinition {
            name: "Drums".to_owned(),
            channels: vec![1, 2, 3, 4],
        }];

        let smaller = plan_device_switch(4, &definitions, 2);
        assert_eq!(smaller.source_channels, 2);
        assert!(smaller.resend_config);
        assert!(smaller.groups[0].invalid);
        assert_eq!(smaller.mixer_layout.channel_group()[0], None);

        let restored = plan_device_switch(2, &definitions, 4);
        assert_eq!(restored.source_channels, 4);
        assert!(restored.resend_config);
        assert!(!restored.groups[0].invalid);
        assert_eq!(restored.mixer_layout.channel_group()[0], Some(0));
        assert_eq!(restored.mixer_layout.channel_group()[3], Some(0));
    }

    #[test]
    fn a_switch_to_the_same_channel_count_does_not_ask_for_a_config_resend() {
        let definitions = vec![GroupDefinition {
            name: "Drums".to_owned(),
            channels: vec![1, 2],
        }];

        let plan = plan_device_switch(2, &definitions, 2);

        assert!(!plan.resend_config);
    }

    #[test]
    fn a_failed_capture_rebuild_yields_no_plan_and_leaves_the_previous_state() {
        let definitions = vec![GroupDefinition {
            name: "Drums".to_owned(),
            channels: vec![1, 2, 3, 4],
        }];
        let error: Box<dyn Error> = "input device \"Gone\" is no longer available".into();

        let result = plan_switch_from_attempt(4, &definitions, Err(error));

        assert!(result.is_err());
        // Nothing was planned or mutated: the previous configuration stands.
        assert_eq!(definitions[0].channels, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_successful_capture_rebuild_plans_the_new_channels_and_group_validity() {
        let definitions = vec![GroupDefinition {
            name: "Drums".to_owned(),
            channels: vec![1, 2, 3, 4],
        }];

        let plan = plan_switch_from_attempt(4, &definitions, Ok(2)).expect("plan should build");

        assert_eq!(plan.source_channels, 2);
        assert!(plan.resend_config);
        assert!(plan.groups[0].invalid);
    }
}
