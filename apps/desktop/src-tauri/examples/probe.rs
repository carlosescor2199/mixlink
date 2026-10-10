//! Drives the same engine calls the window uses, without a webview.
//!
//! Useful on a headless machine and as proof that the values the window renders are produced by
//! these commands: it prints the JSON the frontend receives. It also exercises the editable
//! musician list end to end — start with nobody, add by address, label the new musician, read the
//! labelled status back, and remove the musician — so the exact command path the window invokes is
//! proven without a window.

use std::thread::sleep;
use std::time::Duration;

use mixlink_desktop::commands::{self, DesktopState};
use mixlink_desktop::dto::StartRequest;
use mixlink_desktop::names::MusicianNames;

fn main() {
    // A scratch config directory: the probe must not disturb the real app's labels.
    let names_path = std::env::temp_dir()
        .join("mixlink-desktop-probe")
        .join("musicians.json");
    let state = DesktopState::new(MusicianNames::load(names_path));

    let summary =
        commands::start_engine(&state, StartRequest::default()).expect("the engine should start");
    println!(
        "start_engine -> device=\"{}\" capture_format={}",
        summary.device_name,
        summary.capture_format.describe()
    );
    println!(
        "start_engine json -> {}",
        serde_json::to_string(&summary).expect("summary should serialize")
    );

    let status = commands::engine_status(&state).expect("status should be available");
    println!(
        "engine_status -> device=\"{}\" musicians={}",
        status.device_name,
        status.musicians.len()
    );

    // The window's add form calls exactly this.
    let added = commands::add_target(&state, "127.0.0.1:50000".to_owned())
        .expect("the target should be added");
    println!("add_target -> {added}");
    commands::set_musician_name(&state, added.clone(), "Probe musician".to_owned())
        .expect("the name should persist");

    let status = commands::engine_status(&state).expect("status should be available");
    let musician = &status.musicians[0];
    println!(
        "engine_status after add -> name={:?} address={} control_connected={}",
        musician.name, musician.address, musician.control_connected
    );
    println!(
        "engine_status json -> {}",
        serde_json::to_string(&status).expect("status should serialize")
    );

    // Poll the way the window does, so the live counters the frontend renders are visible without
    // a webview: samples_received must climb and the per-channel levels must react.
    for tick in 1..=4 {
        sleep(Duration::from_millis(300));
        let status = commands::engine_status(&state).expect("status should be available");
        println!(
            "poll {tick} -> samples_received={} packets_sent={} channel_levels={:?}",
            status.samples_received,
            status.packets_sent,
            status
                .channels
                .iter()
                .map(|channel| channel.level)
                .collect::<Vec<_>>()
        );
    }

    let removed = commands::remove_target(&state, added).expect("the target should be removed");
    println!("remove_target -> {removed}");
    let status = commands::engine_status(&state).expect("status should be available");
    println!(
        "engine_status after remove -> musicians={}",
        status.musicians.len()
    );

    commands::stop_engine(&state).expect("the engine should stop");
    println!("stop_engine -> ok");
}
