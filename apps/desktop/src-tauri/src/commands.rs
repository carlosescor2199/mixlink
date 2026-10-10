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

/// The engine handle the commands share, or `None` when the engine is not running.
///
/// A [Mutex] is enough because the engine is already internally concurrent: the commands only need
/// exclusive access to start, stop and take a snapshot.
pub type EngineState = Mutex<Option<EngineHandle>>;

const DEFAULT_TARGET: &str = "127.0.0.1:50000";
const DEFAULT_CONTROL_PORT: u16 = 50001;

/// Translates the webview's request into the engine's own configuration.
fn to_engine_config(request: StartRequest) -> EngineConfig {
    let targets = if request.targets.is_empty() {
        vec![DEFAULT_TARGET.to_owned()]
    } else {
        request.targets
    };
    EngineConfig {
        device_filter: request.device_filter,
        targets,
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
pub fn start_engine(state: &EngineState, request: StartRequest) -> Result<StartSummary, String> {
    let config = to_engine_config(request);
    let mut guard = state
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

/// Returns the engine's current status, or an error when it is not running.
pub fn engine_status(state: &EngineState) -> Result<EngineStatusDto, String> {
    let guard = state
        .lock()
        .map_err(|_| "engine state lock was poisoned".to_owned())?;
    let handle = guard
        .as_ref()
        .ok_or_else(|| "the engine is not running".to_owned())?;
    Ok(EngineStatusDto::from(&handle.status()))
}

/// Stops the engine and joins its threads. Stopping when idle is a no-op.
pub fn stop_engine(state: &EngineState) -> Result<(), String> {
    let mut guard = state
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
pub fn switch_device(state: &EngineState, device: String) -> Result<StartSummary, String> {
    let mut guard = state
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
