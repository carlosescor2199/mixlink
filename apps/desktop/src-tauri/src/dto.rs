//! The JSON contract between the webview and the engine.
//!
//! The engine's own types are not serialized directly: this module owns the wire shape the window
//! consumes, so the engine keeps no presentation concerns and can change without breaking the UI.

use serde::{Deserialize, Serialize};

use personal_monitoring::{CaptureFormat, EngineStatus, InputDevice, MusicianStatus};

/// The start-up configuration the webview sends to [crate::commands::start_engine].
///
/// Every field is optional so the window can send a partial object and let the defaults fill in.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    #[serde(default)]
    pub device_filter: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub control_port: Option<u16>,
    #[serde(default)]
    pub groups: Vec<GroupRequest>,
}

/// One group as the window describes it. Channel numbers are 1-based, exactly as the CLI takes
/// them, because the conversion to the protocol's 0-based indices happens in the engine.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRequest {
    pub name: String,
    pub channels: Vec<usize>,
}

/// The capture format the engine selected, in plain values a webview can render.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureFormatDto {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: String,
    pub buffer_size: String,
}

impl From<&CaptureFormat> for CaptureFormatDto {
    fn from(format: &CaptureFormat) -> Self {
        Self {
            channels: format.channels,
            sample_rate: format.sample_rate,
            sample_format: format.sample_format.clone(),
            buffer_size: format.buffer_size.clone(),
        }
    }
}

impl CaptureFormatDto {
    /// A one-line description in the same words the CLI prints at startup.
    pub fn describe(&self) -> String {
        format!(
            "{} ch @ {} Hz, {}, buffer {}",
            self.channels, self.sample_rate, self.sample_format, self.buffer_size
        )
    }
}

/// What [crate::commands::start_engine] returns: the two values T3 asks the window to show.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSummary {
    pub device_name: String,
    pub capture_format: CaptureFormatDto,
}

/// One musician row: the engine's target/control join, flattened for JSON.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicianDto {
    pub address: String,
    /// The name the musician typed on their phone and announced when registering, straight from the
    /// engine. It is client input, never a desktop label, and is never sent back to a client.
    pub client_name: Option<String>,
    /// The name this desk shows: the client's own announced name when it has one, otherwise the
    /// engineer's local label. Filled by the command layer; the engine knows nothing of the label.
    pub name: Option<String>,
    /// True when `name` is the desk's local label rather than the client's announced name, so the
    /// window can mark it as local. It is only ever true when the client announced no name; a
    /// client that announced one always wins, which is why the two cases need telling apart.
    pub name_is_local: bool,
    pub control_connected: bool,
    pub volume_percent: u8,
    pub max_level_percent: u8,
    pub muted: bool,
    pub packets_sent: u64,
    pub packets_discarded: u64,
}

/// One configured group, with the same 0-based source indices the engine uses.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    pub name: String,
    pub channels: Vec<usize>,
    /// True when the current input device is missing a channel this group names. The group is still
    /// shown and kept; the window uses this to mark it and to warn before a further switch.
    pub invalid: bool,
}

/// One selectable input device: its name and the channel count a switch would capture.
///
/// `channels` of `0` means the device has no supported PCM input, so it is listed but not usable.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDeviceDto {
    pub name: String,
    pub channels: u16,
}

impl From<&InputDevice> for InputDeviceDto {
    fn from(device: &InputDevice) -> Self {
        Self {
            name: device.name.clone(),
            channels: device.channels,
        }
    }
}

/// One source channel as the window shows it: a 1-based number, its group when the engineer
/// configured one, and the live pre-mix peak.
///
/// There are no per-channel names in the system yet, so a channel is labelled by number and group
/// rather than by a name like "Kick". Naming is engineer configuration that does not exist, and
/// inventing names here would misrepresent the system.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDto {
    pub number: u8,
    pub group: Option<String>,
    pub level: u8,
}

impl From<&MusicianStatus> for MusicianDto {
    fn from(musician: &MusicianStatus) -> Self {
        Self {
            address: musician.address.to_string(),
            client_name: musician.name.clone(),
            name: None,
            name_is_local: false,
            control_connected: musician.control_connected,
            volume_percent: musician.mix.volume_percent,
            max_level_percent: musician.mix.max_level_percent,
            muted: musician.mix.muted,
            packets_sent: musician.counters.packets_sent,
            packets_discarded: musician.counters.packets_discarded,
        }
    }
}

/// A snapshot of the engine, ready for the webview.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatusDto {
    pub device_name: String,
    pub capture_format: CaptureFormatDto,
    pub control_port: u16,
    pub sample_rate: u32,
    pub source_channels: u8,
    pub groups: Vec<GroupDto>,
    pub channels: Vec<ChannelDto>,
    pub stopped: bool,
    pub samples_received: u64,
    pub packets_sent: u64,
    pub packets_discarded: u64,
    pub musicians: Vec<MusicianDto>,
}

impl From<&EngineStatus> for EngineStatusDto {
    fn from(status: &EngineStatus) -> Self {
        let groups: Vec<GroupDto> = status
            .groups
            .iter()
            .map(|group| GroupDto {
                name: group.name.clone(),
                channels: group.channels.clone(),
                invalid: group.invalid,
            })
            .collect();
        // The engine publishes the group layout once; the per-channel membership the window needs
        // is derived here rather than being stored twice and risking the two views disagreeing.
        let channels = (0..usize::from(status.source_channels))
            .map(|index| ChannelDto {
                number: (index + 1) as u8,
                // An invalid group is shown in the group list but does not own any channel, because
                // the mixer ignores it; labelling a channel with it would overstate what is applied.
                group: status
                    .groups
                    .iter()
                    .find(|group| !group.invalid && group.channels.contains(&index))
                    .map(|group| group.name.clone()),
                level: status.channel_levels.get(index).copied().unwrap_or(0),
            })
            .collect();
        Self {
            device_name: status.device_name.clone(),
            capture_format: (&status.capture_format).into(),
            control_port: status.control_port,
            sample_rate: status.sample_rate,
            source_channels: status.source_channels,
            groups,
            channels,
            stopped: status.stopped,
            samples_received: status.samples_received,
            packets_sent: status.packets_sent,
            packets_discarded: status.packets_discarded,
            musicians: status.musicians.iter().map(MusicianDto::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_monitoring::{CaptureFormat, EngineStatus, GroupStatus, MusicianStatus};

    fn status_with(groups: Vec<GroupStatus>, channel_levels: Vec<u8>) -> EngineStatus {
        EngineStatus {
            device_name: "Test device".to_owned(),
            capture_format: CaptureFormat {
                channels: 2,
                sample_rate: 48_000,
                sample_format: "I16".to_owned(),
                buffer_size: "Default".to_owned(),
            },
            control_port: 50001,
            sample_rate: 48_000,
            source_channels: 2,
            groups,
            channel_levels,
            stopped: false,
            samples_received: 0,
            packets_sent: 0,
            packets_discarded: 0,
            musicians: Vec::new(),
        }
    }

    #[test]
    fn channels_label_by_number_with_their_group_and_level() {
        let status = status_with(
            vec![GroupStatus {
                name: "Drums".to_owned(),
                channels: vec![0],
                invalid: false,
            }],
            vec![50, 100],
        );

        let dto = EngineStatusDto::from(&status);

        assert_eq!(dto.channels.len(), 2);
        assert_eq!(dto.channels[0].number, 1);
        assert_eq!(dto.channels[0].group.as_deref(), Some("Drums"));
        assert_eq!(dto.channels[0].level, 50);
        assert_eq!(dto.channels[1].number, 2);
        assert_eq!(dto.channels[1].group, None);
        assert_eq!(dto.channels[1].level, 100);
        assert_eq!(dto.groups.len(), 1);
        assert_eq!(dto.groups[0].channels, vec![0]);
    }

    #[test]
    fn a_channel_without_a_group_reports_no_group() {
        let status = status_with(Vec::new(), vec![0, 0]);

        let dto = EngineStatusDto::from(&status);

        assert!(dto.channels.iter().all(|channel| channel.group.is_none()));
        assert!(dto.groups.is_empty());
    }

    #[test]
    fn an_invalid_group_is_flagged_for_the_window_and_keeps_its_channels() {
        let status = status_with(
            vec![GroupStatus {
                name: "Vocals".to_owned(),
                channels: vec![2, 3],
                invalid: true,
            }],
            vec![0, 0],
        );

        let dto = EngineStatusDto::from(&status);

        assert!(dto.groups[0].invalid);
        assert_eq!(dto.groups[0].channels, vec![2, 3]);
    }

    #[test]
    fn the_engine_name_becomes_the_client_name_and_the_label_is_left_to_the_command_layer() {
        let musician = MusicianStatus {
            address: "192.168.1.30:50000".parse().expect("test address"),
            name: Some("Ana".to_owned()),
            control_connected: false,
            mix: Default::default(),
            counters: Default::default(),
        };

        let dto = MusicianDto::from(&musician);

        assert_eq!(dto.client_name.as_deref(), Some("Ana"));
        assert_eq!(dto.name, None);
        assert!(!dto.name_is_local);
    }

    #[test]
    fn a_musician_that_never_announced_a_name_reports_no_client_name() {
        let musician = MusicianStatus {
            address: "192.168.1.30:50000".parse().expect("test address"),
            name: None,
            control_connected: false,
            mix: Default::default(),
            counters: Default::default(),
        };

        let dto = MusicianDto::from(&musician);

        assert_eq!(dto.client_name, None);
        assert_eq!(dto.name, None);
        assert!(!dto.name_is_local);
    }

    #[test]
    fn input_device_reports_its_name_and_channel_count() {
        let device = InputDevice {
            name: "Interface".to_owned(),
            channels: 6,
        };

        let dto = InputDeviceDto::from(&device);

        assert_eq!(dto.name, "Interface");
        assert_eq!(dto.channels, 6);
    }

    #[test]
    fn a_channel_of_an_invalid_group_is_not_labelled_with_it() {
        let status = status_with(
            vec![GroupStatus {
                name: "Drums".to_owned(),
                channels: vec![0, 1],
                invalid: true,
            }],
            vec![0, 0],
        );

        let dto = EngineStatusDto::from(&status);

        assert_eq!(dto.channels[0].group, None);
        assert!(dto.groups[0].invalid);
    }
}
