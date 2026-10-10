//! The JSON contract between the webview and the engine.
//!
//! The engine's own types are not serialized directly: this module owns the wire shape the window
//! consumes, so the engine keeps no presentation concerns and can change without breaking the UI.

use serde::{Deserialize, Serialize};

use personal_monitoring::{CaptureFormat, EngineStatus, MusicianStatus};

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
    pub control_connected: bool,
    pub volume_percent: u8,
    pub max_level_percent: u8,
    pub muted: bool,
    pub packets_sent: u64,
    pub packets_discarded: u64,
}

impl From<&MusicianStatus> for MusicianDto {
    fn from(musician: &MusicianStatus) -> Self {
        Self {
            address: musician.address.to_string(),
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
    pub stopped: bool,
    pub samples_received: u64,
    pub packets_sent: u64,
    pub packets_discarded: u64,
    pub musicians: Vec<MusicianDto>,
}

impl From<&EngineStatus> for EngineStatusDto {
    fn from(status: &EngineStatus) -> Self {
        Self {
            device_name: status.device_name.clone(),
            capture_format: (&status.capture_format).into(),
            control_port: status.control_port,
            sample_rate: status.sample_rate,
            source_channels: status.source_channels,
            stopped: status.stopped,
            samples_received: status.samples_received,
            packets_sent: status.packets_sent,
            packets_discarded: status.packets_discarded,
            musicians: status.musicians.iter().map(MusicianDto::from).collect(),
        }
    }
}
