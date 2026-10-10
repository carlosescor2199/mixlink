//! Drives the command layer's name resolution against the real stored labels, without a webview.
//!
//! It reproduces the exact input of the name-precedence defect: the app's own
//! `%APPDATA%\com.mixlink.desktop\musicians.json`, which maps `192.168.1.10:50000` to `Carlos`.
//! The store file is only read, never written, and the run fails if its bytes change.
//!
//! No host at `192.168.1.10` exists on this machine, so the engine's announcement is supplied at
//! the boundary where the engine hands the command layer its snapshot: the probe builds the same
//! status the engine reports for a musician at that address — once announcing `Carlos Daniel` from
//! the client, once silent — runs `commands::apply_names` on the translated DTO exactly as
//! `commands::engine_status` does, and prints the name and the `nameIsLocal` marker the window
//! receives.

use std::path::PathBuf;

use mixlink_desktop::commands;
use mixlink_desktop::dto::EngineStatusDto;
use mixlink_desktop::names::{MusicianNames, NAMES_FILE};
use personal_monitoring::{CaptureFormat, EngineStatus, MusicianStatus};

/// The address stored in the user's real label file, and the one the bug was reported at.
const ADDRESS: &str = "192.168.1.10:50000";
/// The name the musician's own app announced for that address.
const CLIENT_NAME: &str = "Carlos Daniel";

fn store_path() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").expect("APPDATA should be set on Windows");
    PathBuf::from(appdata)
        .join("com.mixlink.desktop")
        .join(NAMES_FILE)
}

/// The snapshot the engine reports for the single musician at [ADDRESS], with or without an
/// announced client name. All other values are neutral; only the name path is under test.
fn engine_status(client_name: Option<&str>) -> EngineStatus {
    EngineStatus {
        device_name: "Probe device".to_owned(),
        capture_format: CaptureFormat {
            channels: 2,
            sample_rate: 48_000,
            sample_format: "I16".to_owned(),
            buffer_size: "Default".to_owned(),
        },
        control_port: 50001,
        sample_rate: 48_000,
        source_channels: 2,
        groups: Vec::new(),
        channel_levels: vec![0, 0],
        stopped: false,
        samples_received: 0,
        packets_sent: 0,
        packets_discarded: 0,
        musicians: vec![MusicianStatus {
            address: ADDRESS.parse().expect("the probe address should parse"),
            name: client_name.map(str::to_owned),
            // Announcing a name means a control channel was there to carry it.
            control_connected: client_name.is_some(),
            mix: Default::default(),
            counters: Default::default(),
        }],
    }
}

/// Applies the real store's labels the way `engine_status` does and prints the resulting row.
fn report(case: &str, names: &MusicianNames, client_name: Option<&str>) {
    let mut status = EngineStatusDto::from(&engine_status(client_name));
    commands::apply_names(&mut status, names);
    let musician = &status.musicians[0];
    println!("--- {case} ---");
    println!("  clientName  -> {:?}", musician.client_name);
    println!("  name        -> {:?}", musician.name);
    println!("  nameIsLocal -> {}", musician.name_is_local);
    println!(
        "  row json    -> {}",
        serde_json::to_string(musician).expect("the row should serialize")
    );
}

fn main() {
    let path = store_path();
    println!("name store -> {}", path.display());
    let raw = std::fs::read_to_string(&path).expect("the real name store should be readable");
    println!("store bytes -> {}", raw.len());
    println!("store contents -> {raw}");

    let names = MusicianNames::load(path.clone());
    println!("loaded label for {ADDRESS} -> {:?}", names.get(ADDRESS));

    report("client announces a name", &names, Some(CLIENT_NAME));
    report("client announces nothing", &names, None);

    let after =
        std::fs::read_to_string(&path).expect("the real name store should still be readable");
    assert_eq!(
        raw, after,
        "the probe must not modify the user's name store"
    );
    println!("store after -> unchanged ({} bytes)", after.len());
}
