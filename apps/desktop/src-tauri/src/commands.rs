//! The engine calls the Tauri commands delegate to.
//!
//! They live apart from the `#[tauri::command]` wrappers in the crate root so that an example
//! binary can drive exactly the same code path without a window. The window is only a rendering of
//! what these functions return.

use std::sync::Mutex;

use personal_monitoring::{start, EngineConfig, EngineHandle, GroupDefinition};

use crate::dto::{
    CaptureFormatDto, EngineStatusDto, GroupRequest, InputDeviceDto, StartRequest, StartSummary,
};
use crate::names::MusicianNames;

/// The state the desktop commands share: the running engine, and the desktop-only name store.
///
/// The engine handle is behind a [Mutex] because the engine is already internally concurrent: the
/// commands only need exclusive access to start, stop and take a snapshot. The name store has its
/// own lock, and the two are never held in the opposite order, so they cannot deadlock.
pub struct DesktopState {
    engine: Mutex<Option<EngineHandle>>,
    names: MusicianNames,
}

impl DesktopState {
    pub fn new(names: MusicianNames) -> Self {
        Self {
            engine: Mutex::new(None),
            names,
        }
    }
}

const DEFAULT_CONTROL_PORT: u16 = 50001;

/// Translates the webview's request into the engine's own configuration.
///
/// An empty target list stays empty: the musician list is editable at runtime now, so the window
/// starts with nobody configured and the engineer adds musicians by address. The old implicit
/// `127.0.0.1:50000` target played into the void on the engineer's own machine and made the first
/// step of a real session "remove the placeholder".
fn to_engine_config(request: StartRequest) -> EngineConfig {
    EngineConfig {
        device_filter: request.device_filter,
        targets: request.targets,
        control_port: request.control_port.unwrap_or(DEFAULT_CONTROL_PORT),
        groups: request
            .groups
            .into_iter()
            .map(|group: GroupRequest| GroupDefinition {
                name: group.name,
                channels: group.channels,
            })
            .collect(),
    }
}

/// Starts the engine and returns the device and capture format the window shows.
///
/// Starting while one is already running replaces it rather than stacking a second capture stream.
pub fn start_engine(state: &DesktopState, request: StartRequest) -> Result<StartSummary, String> {
    let config = to_engine_config(request);
    let mut guard = state
        .engine
        .lock()
        .map_err(|_| "engine state lock was poisoned".to_owned())?;

    if let Some(mut running) = guard.take() {
        running
            .stop()
            .map_err(|error| format!("could not stop the previous engine: {error}"))?;
    }

    match start(config).map_err(|error| error.to_string())? {
        Some(handle) => {
            let summary = StartSummary {
                device_name: handle.device_name().to_owned(),
                capture_format: handle.capture_format().into(),
            };
            println!(
                "[mixlink-desktop] start_engine: device=\"{}\" capture_format={}",
                summary.device_name,
                summary.capture_format.describe()
            );
            *guard = Some(handle);
            Ok(summary)
        }
        None => Err(
            "No input device is available. Connect an audio interface and try again.".to_owned(),
        ),
    }
}

/// Returns the engine's current status, with each musician row's displayed name resolved by
/// [apply_names].
pub fn engine_status(state: &DesktopState) -> Result<EngineStatusDto, String> {
    let guard = state
        .engine
        .lock()
        .map_err(|_| "engine state lock was poisoned".to_owned())?;
    let handle = guard
        .as_ref()
        .ok_or_else(|| "the engine is not running".to_owned())?;
    let mut status = EngineStatusDto::from(&handle.status());
    apply_names(&mut status, &state.names);
    Ok(status)
}

/// Fills each musician row's displayed name, joining the local label store on the address string.
///
/// The client's announced name wins: each musician sets their own name in their own app, and a
/// stored label must not shadow it. The engineer's local label is the fallback for a client that
/// never announced a name, such as a fixed rack added by address; `name_is_local` marks that case
/// so the window can tell the two sources apart.
///
/// Kept separate from [EngineStatusDto] so the DTO stays a pure translation of engine state and
/// the desktop-only concern is applied in one place. The client's own name stays in `clientName`,
/// so the window can still show what the phone calls itself. Public so the headless probe can
/// drive exactly the name resolution the window gets, without a webview.
pub fn apply_names(status: &mut EngineStatusDto, names: &MusicianNames) {
    for musician in &mut status.musicians {
        let local = names.get(&musician.address);
        musician.name_is_local = musician.client_name.is_none() && local.is_some();
        musician.name = musician.client_name.clone().or(local);
    }
}

/// Adds a musician by address while the engine runs, returning the resolved address.
///
/// A duplicate IP or an unresolvable address is rejected by the engine with an error naming the
/// value, and nothing changes. On success the next audio packet is sent to the new address; the
/// other musicians, the control server and the discovery beacon are untouched.
pub fn add_target(state: &DesktopState, address: String) -> Result<String, String> {
    let guard = state
        .engine
        .lock()
        .map_err(|_| "engine state lock was poisoned".to_owned())?;
    let handle = guard
        .as_ref()
        .ok_or_else(|| "the engine is not running".to_owned())?;
    let added = handle
        .add_target(&address)
        .map_err(|error| error.to_string())?;
    println!("[mixlink-desktop] add_target: {added}");
    Ok(added.to_string())
}

/// Removes a musician by address while the engine runs, returning the removed address.
///
/// The engine's documented removal semantics apply: their audio stops on the next packet, their
/// control channel stays open and their next `mix` is answered with the existing
/// `no UDP target configured for client IP` error, and no other musician is disturbed.
pub fn remove_target(state: &DesktopState, address: String) -> Result<String, String> {
    let guard = state
        .engine
        .lock()
        .map_err(|_| "engine state lock was poisoned".to_owned())?;
    let handle = guard
        .as_ref()
        .ok_or_else(|| "the engine is not running".to_owned())?;
    let removed = handle
        .remove_target(&address)
        .map_err(|error| error.to_string())?;
    println!("[mixlink-desktop] remove_target: {removed}");
    Ok(removed.to_string())
}

/// Sets or clears a musician's label. The label never reaches the engine or any client, and it is
/// only shown for a client that has not announced a name of its own: a client's own name wins.
pub fn set_musician_name(
    state: &DesktopState,
    address: String,
    name: String,
) -> Result<(), String> {
    state.names.set(&address, &name)?;
    println!("[mixlink-desktop] set_musician_name: {address} -> \"{name}\"");
    Ok(())
}

/// Stops the engine and joins its threads. Stopping when idle is a no-op.
pub fn stop_engine(state: &DesktopState) -> Result<(), String> {
    let mut guard = state
        .engine
        .lock()
        .map_err(|_| "engine state lock was poisoned".to_owned())?;
    match guard.take() {
        Some(mut handle) => handle.stop().map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

/// Lists the input devices the window can switch to, each with the channel count a switch would
/// capture. Independent of the engine, so the window can populate the selector before starting.
pub fn list_input_devices() -> Result<Vec<InputDeviceDto>, String> {
    personal_monitoring::list_input_devices()
        .map(|devices| devices.iter().map(InputDeviceDto::from).collect())
        .map_err(|error| error.to_string())
}

/// Switches a running engine to another input device, keeping the network, control and discovery
/// threads alive. A failed switch returns the error and leaves the previous device running.
pub fn switch_device(state: &DesktopState, device: String) -> Result<StartSummary, String> {
    let mut guard = state
        .engine
        .lock()
        .map_err(|_| "engine state lock was poisoned".to_owned())?;
    let handle = guard
        .as_mut()
        .ok_or_else(|| "the engine is not running".to_owned())?;
    handle
        .switch_device(Some(&device))
        .map_err(|error| error.to_string())?;
    let format = CaptureFormatDto::from(handle.capture_format());
    println!(
        "[mixlink-desktop] switch_device: device=\"{}\" capture_format={}",
        handle.device_name(),
        format.describe()
    );
    Ok(StartSummary {
        device_name: handle.device_name().to_owned(),
        capture_format: format,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::MusicianDto;

    fn status_with_musicians(addresses: &[&str]) -> EngineStatusDto {
        EngineStatusDto {
            device_name: "Test device".to_owned(),
            capture_format: CaptureFormatDto {
                channels: 2,
                sample_rate: 48_000,
                sample_format: "I16".to_owned(),
                buffer_size: "Default".to_owned(),
            },
            control_port: DEFAULT_CONTROL_PORT,
            sample_rate: 48_000,
            source_channels: 2,
            groups: Vec::new(),
            channels: Vec::new(),
            stopped: false,
            samples_received: 0,
            packets_sent: 0,
            packets_discarded: 0,
            musicians: addresses
                .iter()
                .map(|address| MusicianDto {
                    address: (*address).to_owned(),
                    client_name: None,
                    name: None,
                    name_is_local: false,
                    control_connected: false,
                    volume_percent: 100,
                    max_level_percent: 100,
                    muted: false,
                    packets_sent: 0,
                    packets_discarded: 0,
                })
                .collect(),
        }
    }

    fn names_path(test: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir()
            .join("mixlink-command-tests")
            .join(test);
        let _ = std::fs::remove_dir_all(&directory);
        directory.join("musicians.json")
    }

    #[test]
    fn status_rows_take_their_labels_from_the_name_store() {
        let names = MusicianNames::load(names_path("labels"));
        names
            .set("192.168.1.30:50000", "Ana")
            .expect("setting a name should persist");
        let mut status = status_with_musicians(&["192.168.1.30:50000", "192.168.1.31:50000"]);

        apply_names(&mut status, &names);

        assert_eq!(status.musicians[0].name.as_deref(), Some("Ana"));
        assert!(status.musicians[0].name_is_local);
        assert_eq!(status.musicians[1].name, None);
        assert!(!status.musicians[1].name_is_local);
    }

    #[test]
    fn the_clients_own_name_beats_the_local_label() {
        let names = MusicianNames::load(names_path("client-wins"));
        names
            .set("192.168.1.30:50000", "Lead vocal")
            .expect("setting a name should persist");
        let mut status = status_with_musicians(&["192.168.1.30:50000"]);
        status.musicians[0].client_name = Some("Ana".to_owned());

        apply_names(&mut status, &names);

        // The phone is the authority on its own name: the desk's stored label must not shadow it,
        // and the row must not report the label as if it were the name on screen.
        assert_eq!(status.musicians[0].name.as_deref(), Some("Ana"));
        assert_eq!(status.musicians[0].client_name.as_deref(), Some("Ana"));
        assert!(!status.musicians[0].name_is_local);
    }

    #[test]
    fn a_local_label_applies_when_the_client_announced_nothing() {
        let names = MusicianNames::load(names_path("silent-client"));
        names
            .set("192.168.1.30:50000", "Rack A")
            .expect("setting a name should persist");
        let mut status = status_with_musicians(&["192.168.1.30:50000"]);

        apply_names(&mut status, &names);

        assert_eq!(status.musicians[0].name.as_deref(), Some("Rack A"));
        assert_eq!(status.musicians[0].client_name, None);
        assert!(status.musicians[0].name_is_local);
    }

    #[test]
    fn a_row_with_no_client_name_and_no_label_stays_unnamed() {
        let names = MusicianNames::load(names_path("unnamed"));
        let mut status = status_with_musicians(&["192.168.1.30:50000"]);

        apply_names(&mut status, &names);

        assert_eq!(status.musicians[0].name, None);
        assert!(!status.musicians[0].name_is_local);
    }

    #[test]
    fn an_empty_target_list_stays_empty_so_the_window_starts_with_nobody_configured() {
        let config = to_engine_config(StartRequest::default());

        assert!(config.targets.is_empty());
        assert_eq!(config.control_port, DEFAULT_CONTROL_PORT);
    }
}
