//! Drives the same engine calls the window uses, without a webview.
//!
//! Useful on a headless machine and as proof that the values the window renders are produced by
//! these commands: it prints the JSON the frontend receives.

use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use mixlink_desktop::commands;
use mixlink_desktop::dto::StartRequest;

fn main() {
    let state = Mutex::new(None);
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
    println!("engine_status -> device=\"{}\"", status.device_name);
    println!(
        "engine_status json -> {}",
        serde_json::to_string(&status).expect("status should serialize")
    );

    // Poll the way the window does, so the live source levels the frontend renders are visible
    // without a webview: samples_received must climb and the per-channel levels must react.
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

    commands::stop_engine(&state).expect("the engine should stop");
    println!("stop_engine -> ok");
}
