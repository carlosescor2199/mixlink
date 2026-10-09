use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Arc;

use cpal::traits::DeviceTrait;
use cpal::{
    Device, InputCallbackInfo, SampleFormat, Stream, SupportedStreamConfig,
    SupportedStreamConfigRange,
};

use crate::network::PacketStats;
use crate::protocol::AudioPacket;

pub(crate) const TARGET_SAMPLE_RATE: u32 = 48_000;
const MAX_SAMPLES_PER_PACKET: usize = u16::MAX as usize;

pub(crate) fn select_device(
    devices: Vec<Device>,
    filter: Option<&str>,
) -> Result<Device, Box<dyn Error>> {
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

pub(crate) fn select_input_config(
    device: &Device,
) -> Result<SupportedStreamConfig, Box<dyn Error>> {
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

pub(crate) fn build_input_stream(
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
}
