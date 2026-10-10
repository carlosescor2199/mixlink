//! The MixLink desktop shell.
//!
//! This crate is deliberately thin: it owns the window and the JSON contract, and delegates every
//! engine call to [commands]. The engine lives in `personal-monitoring` and knows nothing of Tauri,
//! so the headless binary does not inherit this dependency stack.

pub mod commands;
pub mod dto;
pub mod names;

use commands::DesktopState;
use dto::{EngineStatusDto, InputDeviceDto, StartRequest, StartSummary};
use names::MusicianNames;
use tauri::{Manager, State};

/// Fails to compile rather than at runtime if the shared state ever gains a non-`Send` field,
/// because Tauri shares managed state with its async runtime.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DesktopState>();
};

#[tauri::command]
async fn start_engine(
    state: State<'_, DesktopState>,
    config: StartRequest,
) -> Result<StartSummary, String> {
    commands::start_engine(state.inner(), config)
}

#[tauri::command]
async fn engine_status(state: State<'_, DesktopState>) -> Result<EngineStatusDto, String> {
    commands::engine_status(state.inner())
}

#[tauri::command]
async fn stop_engine(state: State<'_, DesktopState>) -> Result<(), String> {
    commands::stop_engine(state.inner())
}

#[tauri::command]
async fn list_input_devices() -> Result<Vec<InputDeviceDto>, String> {
    commands::list_input_devices()
}

#[tauri::command]
async fn switch_device(
    state: State<'_, DesktopState>,
    device: String,
) -> Result<StartSummary, String> {
    commands::switch_device(state.inner(), device)
}

#[tauri::command]
async fn add_target(state: State<'_, DesktopState>, address: String) -> Result<String, String> {
    commands::add_target(state.inner(), address)
}

#[tauri::command]
async fn remove_target(state: State<'_, DesktopState>, address: String) -> Result<String, String> {
    commands::remove_target(state.inner(), address)
}

#[tauri::command]
async fn set_musician_name(
    state: State<'_, DesktopState>,
    address: String,
    name: String,
) -> Result<(), String> {
    commands::set_musician_name(state.inner(), address, name)
}

/// Builds and runs the desktop application.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // The label store is a desktop-app concern and lives in the app's config directory, so
            // it survives a restart without touching the engine or any client protocol.
            let directory = app.path().app_config_dir()?;
            app.manage(DesktopState::new(MusicianNames::load(
                directory.join(names::NAMES_FILE),
            )));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_engine,
            engine_status,
            stop_engine,
            list_input_devices,
            switch_device,
            add_target,
            remove_target,
            set_musician_name
        ])
        .run(tauri::generate_context!())
        .expect("error while running the MixLink desktop shell");
}
