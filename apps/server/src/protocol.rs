use serde::{Deserialize, Serialize};

use crate::mix::{merge_flags, merge_levels, MixValues};

const PACKET_MAGIC: [u8; 4] = *b"PMON";
const PACKET_VERSION: u8 = 1;
const HEADER_SIZE: usize = 4 + 1 + 1 + 4 + 8 + 2;

pub(crate) struct AudioPacket {
    pub(crate) channels: u8,
    pub(crate) sample_rate: u32,
    pub(crate) sequence: u64,
    pub(crate) samples: Vec<i16>,
}

#[derive(Deserialize)]
pub(crate) struct MixCommand {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) volume_percent: i32,
    pub(crate) max_level_percent: i32,
    pub(crate) muted: bool,
    #[serde(default)]
    pub(crate) channels: Option<Vec<i32>>,
    #[serde(default)]
    pub(crate) pans: Option<Vec<i32>>,
    #[serde(default)]
    pub(crate) mutes: Option<Vec<bool>>,
    #[serde(default)]
    pub(crate) solos: Option<Vec<bool>>,
}

/// Parses a `mix` control message and merges it into the client's current values.
///
/// Lives here rather than in `mix` so the mix module stays free of wire-format knowledge: this is
/// the mapping between the two, and the dependency then only runs one way.
pub(crate) fn parse_mix_command(json: &str, current: MixValues) -> Result<MixValues, String> {
    let command: MixCommand = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if command.message_type != "mix" {
        return Err(format!(
            "unsupported control message type: {}",
            command.message_type
        ));
    }

    let channel_gains = merge_levels(
        "channels",
        command.channels.as_ref(),
        &current.channel_gains,
    )?;
    let pans = merge_levels("pans", command.pans.as_ref(), &current.pans)?;
    let channel_muted = merge_flags("mutes", command.mutes.as_ref(), &current.channel_muted)?;
    let channel_solo = merge_flags("solos", command.solos.as_ref(), &current.channel_solo)?;

    Ok(MixValues {
        channel_gains,
        pans,
        channel_muted,
        channel_solo,
        volume_percent: command.volume_percent.clamp(0, 100) as u8,
        max_level_percent: command.max_level_percent.clamp(0, 100) as u8,
        muted: command.muted,
    })
}

#[derive(Serialize)]
struct MixAck {
    #[serde(rename = "type")]
    message_type: &'static str,
    volume_percent: u8,
    max_level_percent: u8,
    muted: bool,
    channels: Vec<u8>,
    pans: Vec<u8>,
    mutes: Vec<bool>,
    solos: Vec<bool>,
}

#[derive(Serialize)]
pub(crate) struct ControlConfig {
    #[serde(rename = "type")]
    pub(crate) message_type: &'static str,
    pub(crate) source_channels: u8,
    pub(crate) sample_rate: u32,
}

#[derive(Serialize)]
struct ControlError {
    #[serde(rename = "type")]
    message_type: &'static str,
    message: String,
}

// Packet layout: magic[4], version[1], channels[1], sample_rate[4 LE],
// sequence[8 LE], sample_count[2 LE], then sample_count PCM16 samples in LE.
pub(crate) fn serialize_packet(packet: &AudioPacket) -> Vec<u8> {
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

pub(crate) fn mix_ack(values: MixValues) -> Result<String, serde_json::Error> {
    serde_json::to_string(&MixAck {
        message_type: "mix_ack",
        volume_percent: values.volume_percent,
        max_level_percent: values.max_level_percent,
        muted: values.muted,
        channels: values.channel_gains.to_vec(),
        pans: values.pans.to_vec(),
        mutes: values.channel_muted.to_vec(),
        solos: values.channel_solo.to_vec(),
    })
}

pub(crate) fn control_error(message: String) -> Result<String, serde_json::Error> {
    serde_json::to_string(&ControlError {
        message_type: "error",
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::MixValues;

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
