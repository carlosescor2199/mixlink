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
    #[serde(default)]
    pub(crate) group_levels: Option<Vec<i32>>,
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
    let group_levels = merge_levels(
        "group_levels",
        command.group_levels.as_ref(),
        &current.group_levels,
    )?;

    Ok(MixValues {
        channel_gains,
        pans,
        channel_muted,
        channel_solo,
        group_levels,
        // Membership is server configuration, never client input: preserve whatever the source
        // snapshot carried.
        channel_group: current.channel_group,
        volume_percent: command.volume_percent.clamp(0, 100) as u8,
        max_level_percent: command.max_level_percent.clamp(0, 100) as u8,
        muted: command.muted,
    })
}

/// The longest musician name the server keeps from a registration.
///
/// A longer name is truncated rather than rejected: a musician who types too much must still get
/// audio, and the cap keeps one client from dictating an unbounded string to every UI.
pub(crate) const MAX_MUSICIAN_NAME_LENGTH: usize = 64;

/// A `register` control message: the musician's name and the UDP port the client listens on.
///
/// There is deliberately no address field. The server takes the address from the socket, so a
/// client cannot claim to be someone else's address even if it includes one in the message.
#[derive(Debug, Deserialize)]
pub(crate) struct RegisterCommand {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) name: String,
    pub(crate) udp_port: u16,
}

/// Parses a `register` control message. Errors name what is wrong so a client (or a person
/// debugging with a socket) can tell why the registration was not accepted.
pub(crate) fn parse_register_command(json: &str) -> Result<RegisterCommand, String> {
    let command: RegisterCommand = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if command.message_type != "register" {
        return Err(format!(
            "unsupported control message type: {}",
            command.message_type
        ));
    }
    if command.udp_port == 0 {
        return Err("registration requires a UDP port between 1 and 65535".to_owned());
    }
    Ok(command)
}

/// Normalizes the name a client announced: trimmed, capped, and `None` when nothing is left.
///
/// Truncating instead of rejecting is deliberate: a musician who types a very long name must still
/// get audio, and the cap keeps one client from dictating an unbounded string to every UI.
pub(crate) fn registered_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_MUSICIAN_NAME_LENGTH).collect())
}

/// The envelope every control message shares: just enough to route it to the right parser.
#[derive(Deserialize)]
struct ControlEnvelope {
    #[serde(rename = "type")]
    message_type: String,
}

/// Reads only the `type` of a control message so the connection can route it before committing to
/// a full parse. Invalid JSON is reported the same way the mix parser would report it.
pub(crate) fn control_message_type(json: &str) -> Result<String, String> {
    let envelope: ControlEnvelope =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    Ok(envelope.message_type)
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
    group_levels: Vec<u8>,
}

/// One group as it appears in the `config` message: a name and 0-based source channel indices.
#[derive(Serialize)]
pub(crate) struct GroupConfig {
    pub(crate) name: String,
    pub(crate) channels: Vec<usize>,
}

#[derive(Serialize)]
pub(crate) struct ControlConfig {
    #[serde(rename = "type")]
    pub(crate) message_type: &'static str,
    pub(crate) source_channels: u8,
    pub(crate) sample_rate: u32,
    pub(crate) groups: Vec<GroupConfig>,
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
        group_levels: values.group_levels.to_vec(),
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

    #[test]
    fn mix_ack_echoes_the_group_levels() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"channels":[100,40],"group_levels":[80]}"#,
            MixValues::default(),
        )
        .unwrap();

        let ack = mix_ack(values).unwrap();

        assert!(ack.contains(r#""group_levels":[80,"#));
    }

    #[test]
    fn parses_a_registration_with_the_name_and_udp_port() {
        let command =
            parse_register_command(r#"{"type":"register","name":"Ana","udp_port":50000}"#)
                .expect("a registration should parse");

        assert_eq!(command.name, "Ana");
        assert_eq!(command.udp_port, 50000);
    }

    #[test]
    fn a_registration_without_a_udp_port_is_rejected() {
        let error = parse_register_command(r#"{"type":"register","name":"Ana"}"#)
            .expect_err("a registration without a port must be rejected");

        assert!(
            error.contains("udp_port"),
            "message names the field: {error}"
        );
    }

    #[test]
    fn a_registration_with_port_zero_is_rejected() {
        let error = parse_register_command(r#"{"type":"register","name":"Ana","udp_port":0}"#)
            .expect_err("port zero is not listenable");

        assert!(error.contains("UDP port"), "message explains: {error}");
    }

    #[test]
    fn a_message_of_another_type_is_not_a_registration() {
        let error = parse_register_command(r#"{"type":"mix","name":"Ana","udp_port":50000}"#)
            .expect_err("a mix is not a registration");

        assert!(error.contains("mix"), "message names the type: {error}");
    }

    #[test]
    fn an_address_in_a_registration_is_ignored_because_there_is_no_such_field() {
        let command = parse_register_command(
            r#"{"type":"register","name":"Mallory","udp_port":50000,"address":"10.0.0.99:50000","ip":"10.0.0.99"}"#,
        )
        .expect("unknown fields must not break the parse");

        assert_eq!(command.name, "Mallory");
        assert_eq!(command.udp_port, 50000);
    }

    #[test]
    fn a_musician_name_is_trimmed_and_capped() {
        assert_eq!(registered_name("  Ana  ").as_deref(), Some("Ana"));
        assert_eq!(registered_name("   "), None);
        assert_eq!(registered_name(""), None);

        let long = "a".repeat(MAX_MUSICIAN_NAME_LENGTH + 10);
        let name = registered_name(&long).expect("a long name still registers");

        assert_eq!(name.chars().count(), MAX_MUSICIAN_NAME_LENGTH);
    }

    #[test]
    fn config_carries_groups_with_zero_based_channels() {
        let config = ControlConfig {
            message_type: "config",
            source_channels: 2,
            sample_rate: 48_000,
            groups: vec![GroupConfig {
                name: "Drums".to_owned(),
                channels: vec![0, 1],
            }],
        };

        let json = serde_json::to_string(&config).unwrap();

        assert!(json.contains(r#""groups":[{"name":"Drums","channels":[0,1]}]"#));
    }
}
