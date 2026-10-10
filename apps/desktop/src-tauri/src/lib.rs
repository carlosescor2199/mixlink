//! The MixLink desktop shell.
//!
//! This crate is deliberately thin: it owns the window and the JSON contract, and delegates every
//! engine call to [commands]. The engine lives in `personal-monitoring` and knows nothing of Tauri,
//! so the headless binary does not inherit this dependency stack.

pub mod commands;
pub mod dto;

use commands::EngineState;
use dto::{EngineStatusDto, InputDeviceDto, StartRequest, StartSummary};
use tauri::State;

/// Fails to compile rather than at runtime if the engine ever gains a non-`Send` field, because
/// Tauri shares managed state with its async runtime.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EngineState>();
};

#[tauri::command]
async fn start_engine(
    state: State<'_, EngineState>,
    config: StartRequest,
) -> Result<StartSummary, String> {
    commands::start_engine(state.inner(), config)
}

#[tauri::command]
async fn engine_status(state: State<'_, EngineState>) -> Result<EngineStatusDto, String> {
    commands::engine_status(state.inner())
}

#[tauri::command]
async fn stop_engine(state: State<'_, EngineState>) -> Result<(), String> {
    commands::stop_engine(state.inner())
}

#[tauri::command]
async fn list_input_devices() -> Result<Vec<InputDeviceDto>, String> {
    commands::list_input_devices()
}

#[tauri::command]
async fn switch_device(
    state: State<'_, EngineState>,
    device: String,
) -> Result<StartSummary, String> {
    commands::switch_device(state.inner(), device)
}

/// Builds and runs the desktop application.
pub fn run() {
    tauri::Builder::default()
        .manage(EngineState::new(None))
        .invoke_handler(tauri::generate_handler![
            start_engine,
            engine_status,
            stop_engine,
            list_input_devices,
            switch_device
        ])
        .run(tauri::generate_context!())
        .expect("error while running the MixLink desktop shell");
}
