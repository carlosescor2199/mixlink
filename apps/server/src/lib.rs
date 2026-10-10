mod capture;
mod cli;
mod control;
mod discovery;
mod engine;
mod mix;
mod network;
mod protocol;

use std::collections::HashMap;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::mix::{GroupLayout, MixState};

pub use cli::{parse_arguments, EngineConfig, GroupDefinition};
pub use engine::{
    start, CaptureFormat, EngineEvent, EngineHandle, EngineStatus, MusicianCounters, MusicianStatus,
};
pub use mix::MixValues;

/// Builds one [MixState] per unique target IP.
///
/// The engine keys mixing by IP because a control peer is identified by its IP; two targets that
/// share an IP could not be addressed independently, which is why [crate::cli::resolve_targets]
/// rejects such a configuration before this runs.
pub(crate) fn build_mix_states(
    targets: &[SocketAddr],
    group_layout: &GroupLayout,
) -> Result<Arc<HashMap<IpAddr, Arc<MixState>>>, Box<dyn Error>> {
    let mut mix_states = HashMap::with_capacity(targets.len());
    for target in targets {
        if mix_states
            .insert(target.ip(), Arc::new(MixState::new(group_layout)))
            .is_some()
        {
            return Err(format!(
                "duplicate UDP target IP {} cannot have independent mix state",
                target.ip()
            )
            .into());
        }
    }
    Ok(Arc::new(mix_states))
}
