use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    mpsc::{sync_channel, Receiver, SyncSender, TrySendError},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, InputCallbackInfo, SampleFormat, Stream, SupportedStreamConfig,
    SupportedStreamConfigRange,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

const DEFAULT_TARGET: &str = "127.0.0.1:50000";
const DEFAULT_CONTROL_PORT: u16 = 50001;
const PACKET_MAGIC: [u8; 4] = *b"PMON";
const PACKET_VERSION: u8 = 1;
const CHANNEL_CAPACITY: usize = 8;
const HEADER_SIZE: usize = 4 + 1 + 1 + 4 + 8 + 2;
const MAX_SAMPLES_PER_PACKET: usize = u16::MAX as usize;
const MAX_MIX_CHANNELS: usize = 32;
const TARGET_SAMPLE_RATE: u32 = 48_000;

struct Arguments {
    device_filter: Option<String>,
    targets: Vec<String>,
    control_port: u16,
}

struct AudioPacket {
    channels: u8,
    sample_rate: u32,
    sequence: u64,
    samples: Vec<i16>,
}

struct PacketStats {
    sent: AtomicU64,
    discarded: AtomicU64,
}

struct MixState {
    channel_gains: [AtomicU8; MAX_MIX_CHANNELS],
    volume_percent: AtomicU8,
    max_level_percent: AtomicU8,
    muted: AtomicBool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MixValues {
    channel_gains: [u8; MAX_MIX_CHANNELS],
    volume_percent: u8,
    max_level_percent: u8,
    muted: bool,
}

#[derive(Deserialize)]
struct MixCommand {
    #[serde(rename = "type")]
    message_type: String,
    volume_percent: i32,
    max_level_percent: i32,
    muted: bool,
    #[serde(default)]
    channels: Option<Vec<i32>>,
}

#[derive(Serialize)]
struct MixAck {
    #[serde(rename = "type")]
    message_type: &'static str,
    volume_percent: u8,
    max_level_percent: u8,
    muted: bool,
    channels: Vec<u8>,
}

#[derive(Serialize)]
struct ControlError {
    #[serde(rename = "type")]
    message_type: &'static str,
    message: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments()?;
    let targets = resolve_targets(&arguments.targets)?;
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
        return Ok(());
    }

    println!("Available input devices:");
    for (index, device) in devices.iter().enumerate() {
        let name = device.to_string();
        println!("  {}. {name}", index + 1);
    }

    let device = select_device(devices, arguments.device_filter.as_deref())?;
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
    let mix_states = build_mix_states(&targets)?;
    let control_thread = spawn_control_thread(
        arguments.control_port,
        Arc::clone(&mix_states),
        Arc::clone(&stopped),
    );
    let (packet_sender, packet_receiver) = sync_channel(CHANNEL_CAPACITY);
    let packet_stats = Arc::new(PacketStats {
        sent: AtomicU64::new(0),
        discarded: AtomicU64::new(0),
    });
    let network_thread = spawn_network_thread(
        packet_receiver,
        targets,
        Arc::clone(&mix_states),
        Arc::clone(&packet_stats),
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

    let stop_signal = Arc::clone(&stopped);
    ctrlc::set_handler(move || stop_signal.store(true, Ordering::SeqCst))
        .map_err(|error| format!("could not install Ctrl+C handler: {error}"))?;

    println!("PCM capture is running. Press Ctrl+C or Ctrl+Break to stop.");
    while !stopped.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }

    drop(stream);
    drop(packet_sender);
    network_thread
        .join()
        .map_err(|_| "UDP network thread panicked")?;
    control_thread
        .join()
        .map_err(|_| "control WebSocket thread panicked")?;
    println!(
        "Capture stopped cleanly. Samples received: {}; packets sent: {}; packets discarded: {}.",
        samples_seen.load(Ordering::Relaxed),
        packet_stats.sent.load(Ordering::Relaxed),
        packet_stats.discarded.load(Ordering::Relaxed),
    );
    Ok(())
}

fn parse_arguments() -> Result<Arguments, Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let mut device_filter = None;
    let mut targets = Vec::new();
    let mut control_port = DEFAULT_CONTROL_PORT;

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--device" => {
                device_filter = Some(
                    arguments
                        .next()
                        .ok_or("--device requires a search string")?,
                );
            }
            "--target" => {
                targets.push(
                    arguments
                        .next()
                        .ok_or("--target requires a host:port value")?,
                );
            }
            "--control-port" => {
                let port = arguments
                    .next()
                    .ok_or("--control-port requires a port")?
                    .parse()
                    .map_err(|_| "--control-port must be a valid TCP port")?;
                if port == 0 {
                    return Err("--control-port must be between 1 and 65535".into());
                }
                control_port = port;
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }

    if targets.is_empty() {
        targets.push(DEFAULT_TARGET.to_owned());
    }

    Ok(Arguments {
        device_filter,
        targets,
        control_port,
    })
}

fn resolve_targets(targets: &[String]) -> Result<Vec<SocketAddr>, Box<dyn Error>> {
    let resolved = targets
        .iter()
        .map(|target| {
            target
                .to_socket_addrs()
                .map_err(|error| format!("invalid --target `{target}`: {error}"))?
                .next()
                .ok_or_else(|| format!("invalid --target `{target}`: it resolved to no address"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| -> Box<dyn Error> { error.into() })?;

    let mut seen_ips = HashMap::new();
    for target in &resolved {
        if let Some(previous) = seen_ips.insert(target.ip(), *target) {
            return Err(format!(
                "duplicate UDP target IP {} is not supported (targets {} and {})",
                target.ip(),
                previous,
                target
            )
            .into());
        }
    }

    Ok(resolved)
}

fn build_mix_states(
    targets: &[SocketAddr],
) -> Result<Arc<HashMap<IpAddr, Arc<MixState>>>, Box<dyn Error>> {
    let mut mix_states = HashMap::with_capacity(targets.len());
    for target in targets {
        if mix_states
            .insert(target.ip(), Arc::new(MixState::default()))
            .is_some()
        {
            return Err(format!(
                "duplicate UDP target IP {} cannot have independent mix state",
                target.ip()
            )
            .into());
        }
    }
    Ok(Arc::new(mix_states))
}

fn select_device(devices: Vec<Device>, filter: Option<&str>) -> Result<Device, Box<dyn Error>> {
    match filter {
        None => Ok(devices
            .into_iter()
            .next()
            .expect("device list was checked before selection")),
        Some(filter) => {
            let filter = filter.to_lowercase();
            devices
                .into_iter()
                .find(|device| device.to_string().to_lowercase().contains(&filter))
                .ok_or_else(|| format!("no input device name contains \"{filter}\".").into())
        }
    }
}

fn select_input_config(device: &Device) -> Result<SupportedStreamConfig, Box<dyn Error>> {
    let mut best_config = None;
    let supported_configs = device
        .supported_input_configs()
        .map_err(|error| format!("could not enumerate input configurations: {error}"))?;

    for range in supported_configs {
        if !is_supported_sample_format(range.sample_format()) {
            continue;
        }

        let sample_rate = if supports_sample_rate(
            range.min_sample_rate(),
            range.max_sample_rate(),
            TARGET_SAMPLE_RATE,
        ) {
            TARGET_SAMPLE_RATE
        } else if let Some(sample_rate) = supported_reduction_sample_rate(&range) {
            sample_rate
        } else {
            continue;
        };

        let config = range.with_sample_rate(sample_rate);
        let rank = (
            u8::from(sample_rate != TARGET_SAMPLE_RATE),
            u8::from(config.channels() != 2),
            sample_format_rank(config.sample_format()),
        );
        if best_config
            .as_ref()
            .map_or(true, |(best_rank, _)| rank < *best_rank)
        {
            best_config = Some((rank, config));
        }
    }

    best_config.map(|(_, config)| config).ok_or_else(|| {
        format!(
            "input device has no supported PCM input at {TARGET_SAMPLE_RATE} Hz or an integer multiple (96 kHz/192 kHz)"
        )
        .into()
    })
}

fn supported_reduction_sample_rate(range: &SupportedStreamConfigRange) -> Option<u32> {
    [TARGET_SAMPLE_RATE * 2, TARGET_SAMPLE_RATE * 4]
        .into_iter()
        .find(|sample_rate| {
            supports_sample_rate(
                range.min_sample_rate(),
                range.max_sample_rate(),
                *sample_rate,
            )
        })
}

fn sample_format_rank(sample_format: SampleFormat) -> u8 {
    match sample_format {
        SampleFormat::I16 => 0,
        SampleFormat::F32 => 1,
        SampleFormat::U16 => 2,
        _ => u8::MAX,
    }
}

fn supports_sample_rate(min_sample_rate: u32, max_sample_rate: u32, sample_rate: u32) -> bool {
    min_sample_rate <= sample_rate && sample_rate <= max_sample_rate
}

fn is_supported_sample_format(sample_format: SampleFormat) -> bool {
    matches!(
        sample_format,
        SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
    )
}

fn build_input_stream(
    device: &Device,
    supported_config: &SupportedStreamConfig,
    packet_sender: SyncSender<AudioPacket>,
    samples_seen: Arc<AtomicU64>,
    packet_stats: Arc<PacketStats>,
) -> Result<Stream, Box<dyn Error>> {
    let config = supported_config.config();
    let channels = config.channels as u8;
    let sample_rate = config.sample_rate;
    let decimator = Decimator::new(sample_rate, channels)?;
    let sequence = Arc::new(AtomicU64::new(0));
    let error_callback = |error| eprintln!("Audio input error: {error}");

    let stream = match supported_config.sample_format() {
        SampleFormat::F32 => device.build_input_stream(
            config,
            {
                let mut decimator = decimator;
                move |data: &[f32], _info: &InputCallbackInfo| {
                    enqueue_packet(
                        &packet_sender,
                        &packet_stats,
                        &sequence,
                        decimator
                            .process(&data.iter().copied().map(f32_to_i16).collect::<Vec<_>>()),
                        channels,
                        &samples_seen,
                    );
                }
            },
            error_callback,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            config,
            {
                let mut decimator = decimator;
                move |data: &[i16], _info: &InputCallbackInfo| {
                    enqueue_packet(
                        &packet_sender,
                        &packet_stats,
                        &sequence,
                        decimator.process(data),
                        channels,
                        &samples_seen,
                    );
                }
            },
            error_callback,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            config,
            {
                let mut decimator = decimator;
                move |data: &[u16], _info: &InputCallbackInfo| {
                    enqueue_packet(
                        &packet_sender,
                        &packet_stats,
                        &sequence,
                        decimator
                            .process(&data.iter().copied().map(u16_to_i16).collect::<Vec<_>>()),
                        channels,
                        &samples_seen,
                    );
                }
            },
            error_callback,
            None,
        ),
        format => {
            return Err(
                format!("the selected input uses unsupported sample format {format:?}").into(),
            )
        }
    };

    stream.map_err(|error| format!("could not open PCM input stream: {error}").into())
}

fn enqueue_packet(
    packet_sender: &SyncSender<AudioPacket>,
    packet_stats: &PacketStats,
    sequence: &AtomicU64,
    samples: Vec<i16>,
    channels: u8,
    samples_seen: &AtomicU64,
) {
    samples_seen.fetch_add(samples.len() as u64, Ordering::Relaxed);
    if samples.len() > MAX_SAMPLES_PER_PACKET {
        packet_stats.discarded.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let packet = AudioPacket {
        channels,
        sample_rate: TARGET_SAMPLE_RATE,
        sequence: sequence.fetch_add(1, Ordering::Relaxed),
        samples,
    };

    match packet_sender.try_send(packet) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
            packet_stats.discarded.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Default for MixState {
    fn default() -> Self {
        Self {
            channel_gains: std::array::from_fn(|_| AtomicU8::new(100)),
            volume_percent: AtomicU8::new(100),
            max_level_percent: AtomicU8::new(100),
            muted: AtomicBool::new(false),
        }
    }
}

impl Default for MixValues {
    fn default() -> Self {
        Self {
            channel_gains: [100; MAX_MIX_CHANNELS],
            volume_percent: 100,
            max_level_percent: 100,
            muted: false,
        }
    }
}

impl MixState {
    fn update(&self, values: MixValues) {
        for (slot, gain) in self
            .channel_gains
            .iter()
            .zip(values.channel_gains.iter().copied())
        {
            slot.store(gain, Ordering::Relaxed);
        }
        self.volume_percent
            .store(values.volume_percent, Ordering::Relaxed);
        self.max_level_percent
            .store(values.max_level_percent, Ordering::Relaxed);
        self.muted.store(values.muted, Ordering::Relaxed);
    }

    fn snapshot(&self) -> MixValues {
        MixValues {
            channel_gains: std::array::from_fn(|index| {
                self.channel_gains[index].load(Ordering::Relaxed)
            }),
            volume_percent: self.volume_percent.load(Ordering::Relaxed),
            max_level_percent: self.max_level_percent.load(Ordering::Relaxed),
            muted: self.muted.load(Ordering::Relaxed),
        }
    }
}

fn parse_mix_command(json: &str, current: MixValues) -> Result<MixValues, String> {
    let command: MixCommand = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if command.message_type != "mix" {
        return Err(format!(
            "unsupported control message type: {}",
            command.message_type
        ));
    }

    let channel_gains = match command.channels.as_ref() {
        None => current.channel_gains,
        Some(channels) => {
            if channels.len() > MAX_MIX_CHANNELS {
                return Err(format!(
                    "channels accepts at most {MAX_MIX_CHANNELS} gains, received {}",
                    channels.len()
                ));
            }
            let mut gains = current.channel_gains;
            for (slot, value) in gains.iter_mut().zip(channels.iter()) {
                *slot = (*value).clamp(0, 100) as u8;
            }
            gains
        }
    };

    Ok(MixValues {
        channel_gains,
        volume_percent: command.volume_percent.clamp(0, 100) as u8,
        max_level_percent: command.max_level_percent.clamp(0, 100) as u8,
        muted: command.muted,
    })
}

/// Applies one client's mix to an interleaved PCM buffer.
///
/// Source channel `k` maps to output slot `k`, so a buffer whose gains are all 100% is passed
/// through unchanged, apart from the full-scale ceiling clamp that also applied before this
/// change (`i16::MIN` becomes `-32767`). Master volume, mute and the ceiling are applied on top
/// of the per-channel gains.
fn apply_mix(samples: &mut [i16], channels: usize, values: MixValues) {
    if channels == 0 {
        return;
    }

    let ceiling = (i32::from(i16::MAX) * i32::from(values.max_level_percent) / 100) as f32;
    let master = f32::from(values.volume_percent) / 100.0;
    for (index, sample) in samples.iter_mut().enumerate() {
        let channel = (index % channels).min(MAX_MIX_CHANNELS - 1);
        let channel_gain = f32::from(values.channel_gains[channel]) / 100.0;
        let scaled = if values.muted {
            0.0
        } else {
            f32::from(*sample) * channel_gain * master
        };
        *sample = scaled
            .clamp(-ceiling, ceiling)
            .round()
            .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
    }
}

fn mix_ack(values: MixValues) -> Result<String, serde_json::Error> {
    serde_json::to_string(&MixAck {
        message_type: "mix_ack",
        volume_percent: values.volume_percent,
        max_level_percent: values.max_level_percent,
        muted: values.muted,
        channels: values.channel_gains.to_vec(),
    })
}

fn control_error(message: String) -> Result<String, serde_json::Error> {
    serde_json::to_string(&ControlError {
        message_type: "error",
        message,
    })
}

fn spawn_control_thread(
    control_port: u16,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    stopped: Arc<AtomicBool>,
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
        runtime.block_on(run_control_server(listener, mix_states, stopped));
    })
}

async fn run_control_server(
    listener: std::net::TcpListener,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    stopped: Arc<AtomicBool>,
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
                    tokio::spawn(async move {
                        if let Err(error) = handle_control_connection(stream, peer, states).await {
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
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let websocket = accept_async(stream).await?;
    let (mut writer, mut reader) = websocket.split();
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

struct Decimator {
    channels: usize,
    ratio: usize,
    next_frame: usize,
}

impl Decimator {
    fn new(source_sample_rate: u32, channels: u8) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            channels: usize::from(channels),
            ratio: sample_rate_reduction_ratio(source_sample_rate)?,
            next_frame: 0,
        })
    }

    fn process(&mut self, samples: &[i16]) -> Vec<i16> {
        decimate_frames(samples, self.channels, self.ratio, &mut self.next_frame)
            .expect("decimator configuration must match the input stream")
    }
}

fn sample_rate_reduction_ratio(source_sample_rate: u32) -> Result<usize, String> {
    if source_sample_rate < TARGET_SAMPLE_RATE || source_sample_rate % TARGET_SAMPLE_RATE != 0 {
        return Err(format!(
            "source sample rate {source_sample_rate} Hz cannot be reduced exactly to {TARGET_SAMPLE_RATE} Hz; only 48000 Hz and integer multiples are supported"
        ));
    }

    Ok((source_sample_rate / TARGET_SAMPLE_RATE) as usize)
}

fn decimate_frames(
    samples: &[i16],
    channels: usize,
    ratio: usize,
    next_frame: &mut usize,
) -> Result<Vec<i16>, String> {
    if channels == 0 {
        return Err("cannot decimate PCM without channels".to_owned());
    }
    if ratio == 0 {
        return Err("decimation ratio must be greater than zero".to_owned());
    }
    if samples.len() % channels != 0 {
        return Err("PCM sample data must contain complete frames".to_owned());
    }

    let frame_count = samples.len() / channels;
    let selected_frames = (frame_count + ratio - 1) / ratio;
    let mut output = Vec::with_capacity(selected_frames * channels);
    for frame in samples.chunks_exact(channels) {
        if *next_frame == 0 {
            output.extend_from_slice(frame);
        }
        *next_frame = (*next_frame + 1) % ratio;
    }
    Ok(output)
}

fn spawn_network_thread(
    packet_receiver: Receiver<AudioPacket>,
    targets: Vec<SocketAddr>,
    mix_states: Arc<HashMap<IpAddr, Arc<MixState>>>,
    packet_stats: Arc<PacketStats>,
) -> Result<JoinHandle<()>, Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    Ok(thread::spawn(move || {
        for packet in packet_receiver {
            for target in &targets {
                let mut samples = packet.samples.clone();
                let mix_state = mix_states
                    .get(&target.ip())
                    .expect("every target must have a mix state");
                apply_mix(
                    &mut samples,
                    usize::from(packet.channels),
                    mix_state.snapshot(),
                );
                let target_packet = AudioPacket {
                    channels: packet.channels,
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

// Packet layout: magic[4], version[1], channels[1], sample_rate[4 LE],
// sequence[8 LE], sample_count[2 LE], then sample_count PCM16 samples in LE.
fn serialize_packet(packet: &AudioPacket) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_SIZE + packet.samples.len() * 2);
    bytes.extend_from_slice(&PACKET_MAGIC);
    bytes.push(PACKET_VERSION);
    bytes.push(packet.channels);
    bytes.extend_from_slice(&packet.sample_rate.to_le_bytes());
    bytes.extend_from_slice(&packet.sequence.to_le_bytes());
    bytes.extend_from_slice(&(packet.samples.len() as u16).to_le_bytes());
    for sample in &packet.samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn u16_to_i16(sample: u16) -> i16 {
    (i32::from(sample) - 32768) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_documented_header_and_pcm_payload() {
        let packet = AudioPacket {
            channels: 2,
            sample_rate: 48_000,
            sequence: 9,
            samples: vec![-1, 0x1234],
        };

        assert_eq!(
            serialize_packet(&packet),
            vec![
                b'P', b'M', b'O', b'N', 1, 2, 0x80, 0xbb, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0xff,
                0xff, 0x34, 0x12,
            ]
        );
    }

    #[test]
    fn converts_supported_sample_formats_to_pcm16() {
        assert_eq!(f32_to_i16(-1.0), -32767);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(0.5), 16384);
        assert_eq!(u16_to_i16(0), -32768);
        assert_eq!(u16_to_i16(32768), 0);
        assert_eq!(u16_to_i16(u16::MAX), 32767);
    }

    #[test]
    fn accepts_target_sample_rate_inside_supported_range() {
        assert!(supports_sample_rate(44_100, 192_000, TARGET_SAMPLE_RATE));
        assert!(supports_sample_rate(48_000, 48_000, TARGET_SAMPLE_RATE));
    }

    #[test]
    fn rejects_target_sample_rate_outside_supported_range() {
        assert!(!supports_sample_rate(8_000, 44_100, TARGET_SAMPLE_RATE));
        assert!(!supports_sample_rate(96_000, 192_000, TARGET_SAMPLE_RATE));
    }

    #[test]
    fn decimates_192_khz_to_48_khz_by_complete_frames() {
        let mut next_frame = 0;
        let samples = [
            10, 11, 20, 21, 30, 31, 40, 41, 50, 51, 60, 61, 70, 71, 80, 81,
        ];

        assert_eq!(
            decimate_frames(
                &samples,
                2,
                sample_rate_reduction_ratio(192_000).unwrap(),
                &mut next_frame,
            )
            .unwrap(),
            vec![10, 11, 50, 51]
        );
    }

    #[test]
    fn leaves_48_khz_samples_unchanged() {
        let mut next_frame = 0;
        let samples = [10, 11, 20, 21];

        assert_eq!(
            decimate_frames(
                &samples,
                2,
                sample_rate_reduction_ratio(48_000).unwrap(),
                &mut next_frame,
            )
            .unwrap(),
            samples
        );
    }

    #[test]
    fn rejects_non_integer_reduction_ratio() {
        let error = sample_rate_reduction_ratio(44_100).unwrap_err();

        assert!(error.contains("cannot be reduced exactly"));
    }

    #[test]
    fn parses_device_and_target_arguments() {
        let arguments = parse_arguments_from(["--device", "Volt", "--target", "localhost:6000"])
            .expect("arguments should parse");

        assert_eq!(arguments.device_filter.as_deref(), Some("Volt"));
        assert_eq!(arguments.targets, vec!["localhost:6000"]);
        assert_eq!(arguments.control_port, DEFAULT_CONTROL_PORT);
    }

    #[test]
    fn parses_repeated_target_arguments() {
        let arguments = parse_arguments_from([
            "--target",
            "192.168.1.3:50000",
            "--target",
            "192.168.1.4:50000",
        ])
        .expect("arguments should parse");

        assert_eq!(
            arguments.targets,
            vec!["192.168.1.3:50000", "192.168.1.4:50000"]
        );
    }

    #[test]
    fn defaults_to_one_target_when_not_configured() {
        let arguments = parse_arguments_from::<0>([]).expect("arguments should parse");

        assert_eq!(arguments.targets, vec![DEFAULT_TARGET]);
    }

    #[test]
    fn rejects_targets_with_the_same_ip() {
        let targets = vec![
            "192.168.1.3:50000".to_owned(),
            "192.168.1.3:50001".to_owned(),
        ];

        let error = resolve_targets(&targets).unwrap_err();

        assert!(error
            .to_string()
            .contains("duplicate UDP target IP 192.168.1.3"));
    }

    #[test]
    fn independent_mix_states_transform_the_same_input_differently() {
        let targets = vec![
            "192.168.1.3:50000".parse().unwrap(),
            "192.168.1.4:50000".parse().unwrap(),
        ];
        let states = build_mix_states(&targets).unwrap();
        states
            .get(&"192.168.1.3".parse().unwrap())
            .unwrap()
            .update(MixValues {
                volume_percent: 50,
                ..MixValues::default()
            });
        states
            .get(&"192.168.1.4".parse().unwrap())
            .unwrap()
            .update(MixValues::default());

        let input = [20_000, -20_000];
        let mut first_output = input;
        let mut second_output = input;
        apply_mix(
            &mut first_output,
            2,
            states
                .get(&"192.168.1.3".parse().unwrap())
                .unwrap()
                .snapshot(),
        );
        apply_mix(
            &mut second_output,
            2,
            states
                .get(&"192.168.1.4".parse().unwrap())
                .unwrap()
                .snapshot(),
        );

        assert_eq!(first_output, [10_000, -10_000]);
        assert_eq!(second_output, input);
    }

    #[test]
    fn updating_one_mix_state_does_not_change_another() {
        let targets = vec![
            "192.168.1.3:50000".parse().unwrap(),
            "192.168.1.4:50000".parse().unwrap(),
        ];
        let states = build_mix_states(&targets).unwrap();
        let first_ip: IpAddr = "192.168.1.3".parse().unwrap();
        let second_ip: IpAddr = "192.168.1.4".parse().unwrap();
        let second_state = states.get(&second_ip).unwrap();
        let initial_second = second_state.snapshot();

        states.get(&first_ip).unwrap().update(MixValues {
            volume_percent: 0,
            max_level_percent: 10,
            muted: true,
            ..MixValues::default()
        });

        assert_eq!(second_state.snapshot(), initial_second);
    }

    fn parse_arguments_from<const N: usize>(
        values: [&str; N],
    ) -> Result<Arguments, Box<dyn Error>> {
        let mut arguments = values.into_iter();
        let mut device_filter = None;
        let mut targets = Vec::new();
        let mut control_port = DEFAULT_CONTROL_PORT;

        while let Some(argument) = arguments.next() {
            match argument {
                "--device" => {
                    device_filter = Some(arguments.next().ok_or("missing device")?.to_owned())
                }
                "--target" => targets.push(arguments.next().ok_or("missing target")?.to_owned()),
                "--control-port" => {
                    control_port = arguments.next().ok_or("missing control port")?.parse()?
                }
                _ => return Err(format!("unknown argument: {argument}").into()),
            }
        }

        if targets.is_empty() {
            targets.push(DEFAULT_TARGET.to_owned());
        }

        Ok(Arguments {
            device_filter,
            targets,
            control_port,
        })
    }

    #[test]
    fn clamps_mix_command_and_applies_gain_ceiling() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":150,"max_level_percent":25,"muted":false}"#,
            MixValues::default(),
        )
        .unwrap();
        assert_eq!(values.volume_percent, 100);
        assert_eq!(values.max_level_percent, 25);

        let mut samples = [i16::MIN, -16_000, 16_000, i16::MAX];
        apply_mix(&mut samples, 2, values);
        assert_eq!(samples, [-8191, -8191, 8191, 8191]);
    }

    #[test]
    fn mute_zeroes_samples_and_rejects_other_message_types() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":true}"#,
            MixValues::default(),
        )
        .unwrap();
        let mut samples = [i16::MIN, 0, i16::MAX];
        apply_mix(&mut samples, 2, values);
        assert_eq!(samples, [0, 0, 0]);
        assert!(parse_mix_command(
            r#"{"type":"status","volume_percent":80,"max_level_percent":90,"muted":false}"#,
            MixValues::default(),
        )
        .is_err());
    }

    #[test]
    fn neutral_channel_gains_leave_the_buffer_unchanged() {
        let mut samples = [-16_000, -8_000, 8_000, 16_000];
        let original = samples;

        apply_mix(&mut samples, 2, MixValues::default());

        assert_eq!(samples, original);
    }

    #[test]
    fn gain_table_holds_one_entry_per_supported_channel() {
        assert_eq!(MixValues::default().channel_gains.len(), MAX_MIX_CHANNELS);
    }

    #[test]
    fn per_channel_gains_transform_only_their_own_channel() {
        let values = MixValues {
            channel_gains: std::array::from_fn(|index| if index == 1 { 50 } else { 100 }),
            ..MixValues::default()
        };

        let mut samples = [10_000, 10_000, -10_000, -10_000];
        apply_mix(&mut samples, 2, values);

        assert_eq!(samples, [10_000, 5_000, -10_000, -5_000]);
    }

    #[test]
    fn absent_channels_field_preserves_existing_gains() {
        let current = MixValues {
            channel_gains: std::array::from_fn(|index| if index == 0 { 40 } else { 70 }),
            ..MixValues::default()
        };

        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#,
            current,
        )
        .unwrap();

        assert_eq!(values.channel_gains, current.channel_gains);
        assert_eq!(values.volume_percent, 80);
    }

    #[test]
    fn channel_gains_are_clamped_and_oversized_lists_are_rejected() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"channels":[150,-20,100]}"#,
            MixValues::default(),
        )
        .unwrap();

        assert_eq!(&values.channel_gains[..3], &[100, 0, 100]);
        assert_eq!(values.channel_gains[3], 100);

        let oversized = format!(
            r#"{{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"channels":[{}]}}"#,
            vec!["100"; MAX_MIX_CHANNELS + 1].join(",")
        );
        assert!(parse_mix_command(&oversized, MixValues::default()).is_err());
    }

    #[test]
    fn mix_ack_reports_the_applied_channel_gains() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"channels":[100,40]}"#,
            MixValues::default(),
        )
        .unwrap();

        let ack = mix_ack(values).unwrap();

        assert!(ack.contains(r#""type":"mix_ack""#));
        assert!(ack.contains(r#""channels":[100,40,100"#));
    }
}
