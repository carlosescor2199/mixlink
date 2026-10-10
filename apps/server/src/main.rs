use std::error::Error;
use std::sync::atomic::Ordering;
use std::time::Duration;

use personal_monitoring::{parse_arguments, start};

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_arguments()?;
    let Some(mut handle) = start(config)? else {
        return Ok(());
    };

    let stop_signal = handle.stop_signal();
    ctrlc::set_handler(move || stop_signal.store(true, Ordering::SeqCst))
        .map_err(|error| format!("could not install Ctrl+C handler: {error}"))?;

    println!("PCM capture is running. Press Ctrl+C or Ctrl+Break to stop.");
    while !handle.is_stopped() {
        std::thread::sleep(Duration::from_millis(200));
    }

    handle.stop()?;
    let status = handle.status();
    println!(
        "Capture stopped cleanly. Samples received: {}; packets sent: {}; packets discarded: {}.",
        status.samples_received, status.packets_sent, status.packets_discarded,
    );
    Ok(())
}
