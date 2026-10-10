mod capture;
mod cli;
mod control;
mod discovery;
mod mix;
mod network;
mod protocol;

use std::collections::HashMap;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc::sync_channel, Arc};
use std::time::Duration;

use cpal::traits::{HostTrait, StreamTrait};

use crate::capture::{build_input_stream, select_device, select_input_config, TARGET_SAMPLE_RATE};
use crate::cli::{parse_arguments, resolve_targets, validate_groups};
use crate::control::spawn_control_thread;
use crate::discovery::spawn_discovery_thread;
use crate::mix::{GroupLayout, MixState};
use crate::network::{spawn_network_thread, PacketStats};

const CHANNEL_CAPACITY: usize = 8;

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
    let source_channels = supported_config.channels().min(u16::from(u8::MAX)) as u8;
    let group_layout = Arc::new(validate_groups(
        &arguments.groups,
        usize::from(source_channels),
    )?);
    let mix_states = build_mix_states(&targets, &group_layout)?;
    let control_thread = spawn_control_thread(
        arguments.control_port,
        Arc::clone(&mix_states),
        Arc::clone(&stopped),
        source_channels,
        Arc::clone(&group_layout),
    );
    let discovery_thread = spawn_discovery_thread(
        arguments.control_port,
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
    discovery_thread
        .join()
        .map_err(|_| "discovery beacon thread panicked")?;
    println!(
        "Capture stopped cleanly. Samples received: {}; packets sent: {}; packets discarded: {}.",
        samples_seen.load(Ordering::Relaxed),
        packet_stats.sent.load(Ordering::Relaxed),
        packet_stats.discarded.load(Ordering::Relaxed),
    );
    Ok(())
}

fn build_mix_states(
    targets: &[SocketAddr],
    group_layout: &GroupLayout,
) -> Result<Arc<HashMap<IpAddr, Arc<MixState>>>, Box<dyn Error>> {
    let mut mix_states = HashMap::with_capacity(targets.len());
    for target in targets {
        if mix_states
            .insert(target.ip(), Arc::new(MixState::new(group_layout)))
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
