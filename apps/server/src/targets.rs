use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, RwLock, RwLockReadGuard};

use crate::mix::{GroupLayout, MixState};
use crate::network::TargetCounters;

/// One configured musician destination: the UDP address plus the state that belongs to it.
///
/// The three values travel together on purpose. The network thread needs exactly this triple for
/// every packet, and keeping it in one entry removes the per-packet hash lookups (and the panics
/// that guarded them) the previous target list required.
pub(crate) struct TargetEntry {
    pub(crate) address: SocketAddr,
    pub(crate) mix_state: Arc<MixState>,
    pub(crate) counters: Arc<TargetCounters>,
}

/// The live routing table: which UDP targets exist, each with its mix state and send counters.
///
/// This is the piece that makes the target list editable at runtime. The network thread reads the
/// table once per packet, the control server looks a peer's IP up in it per message, and the engine
/// adds and removes entries under a short write lock. Adding or removing one entry therefore cannot
/// disturb any other entry, and no thread has to be restarted: the same capture stream, control
/// server and discovery beacon keep running, exactly as a device switch leaves them running.
pub(crate) struct TargetRegistry {
    entries: RwLock<Vec<TargetEntry>>,
}

impl TargetRegistry {
    /// Builds the registry from the startup target list, rejecting duplicate IPs.
    ///
    /// [crate::cli::resolve_targets] already enforces the one-target-per-IP rule for the command
    /// line; this repeats it so a UI-built configuration cannot bypass it, because two targets that
    /// share an IP could not be addressed independently by the mix state or the control join.
    pub(crate) fn new(targets: &[SocketAddr], layout: &GroupLayout) -> Result<Self, String> {
        let mut entries = Vec::with_capacity(targets.len());
        let mut seen: HashMap<IpAddr, SocketAddr> = HashMap::with_capacity(targets.len());
        for target in targets {
            if let Some(previous) = seen.insert(target.ip(), *target) {
                return Err(format!(
                    "duplicate UDP target IP {} is not supported (targets {previous} and {target})",
                    target.ip()
                ));
            }
            entries.push(TargetEntry {
                address: *target,
                mix_state: Arc::new(MixState::new(layout)),
                counters: Arc::new(TargetCounters::default()),
            });
        }
        Ok(Self {
            entries: RwLock::new(entries),
        })
    }

    /// A read view of the current table. Callers iterate it; no entry is ever mutated in place.
    pub(crate) fn read(&self) -> RwLockReadGuard<'_, Vec<TargetEntry>> {
        self.entries.read().expect("target registry lock poisoned")
    }

    /// Resolves and validates `value`, then adds it as a new target.
    ///
    /// The new entry starts with a neutral mix carrying the supplied group layout and zero counters.
    /// A value that cannot be resolved, or whose IP is already configured, is rejected with an
    /// error naming the value; the table is left exactly as it was.
    pub(crate) fn add(&self, value: &str, layout: &GroupLayout) -> Result<SocketAddr, String> {
        let address = resolve_target(value)?;
        let mut entries = self.entries.write().expect("target registry lock poisoned");
        if let Some(existing) = entries
            .iter()
            .find(|entry| entry.address.ip() == address.ip())
        {
            return Err(format!(
                "duplicate UDP target IP {} is not supported (target `{value}` is already configured as {})",
                address.ip(),
                existing.address
            ));
        }
        entries.push(TargetEntry {
            address,
            mix_state: Arc::new(MixState::new(layout)),
            counters: Arc::new(TargetCounters::default()),
        });
        Ok(address)
    }

    /// Removes the target whose IP matches `value`.
    ///
    /// `value` is the address string the engine reports, so a UI can hand back what it displayed.
    /// The error names the value when it is not a socket address or not configured. Removal drops
    /// only this entry: the other targets keep their mix states, their counters and their streams.
    pub(crate) fn remove(&self, value: &str) -> Result<SocketAddr, String> {
        let address = value
            .parse::<SocketAddr>()
            .map_err(|error| format!("invalid target `{value}`: {error}"))?;
        let mut entries = self.entries.write().expect("target registry lock poisoned");
        let Some(index) = entries
            .iter()
            .position(|entry| entry.address.ip() == address.ip())
        else {
            return Err(format!("target `{value}` is not configured"));
        };
        Ok(entries.remove(index).address)
    }

    /// The mix state for a control peer's IP, or `None` when that IP is no longer a target.
    ///
    /// This is the join the control server uses: a peer whose target was removed gets `None`, which
    /// is the same answer an IP that was never configured has always received.
    pub(crate) fn mix_state(&self, ip: IpAddr) -> Option<Arc<MixState>> {
        self.read()
            .iter()
            .find(|entry| entry.address.ip() == ip)
            .map(|entry| Arc::clone(&entry.mix_state))
    }
}

/// Resolves one target value to a socket address, naming the value in every error.
///
/// The runtime counterpart of the startup resolution: the UI hands over what the engineer typed,
/// and an unresolvable value must be reported back with the value itself so a typo is easy to spot.
pub(crate) fn resolve_target(value: &str) -> Result<SocketAddr, String> {
    value
        .to_socket_addrs()
        .map_err(|error| format!("invalid target `{value}`: {error}"))?
        .next()
        .ok_or_else(|| format!("invalid target `{value}`: it resolved to no address"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::Group;
    use std::sync::atomic::Ordering;

    fn address(value: &str) -> SocketAddr {
        value.parse().expect("test address should parse")
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("test IP should parse")
    }

    fn addresses(registry: &TargetRegistry) -> Vec<SocketAddr> {
        registry.read().iter().map(|entry| entry.address).collect()
    }

    #[test]
    fn adds_a_resolved_target_and_keeps_the_existing_ones() {
        let registry =
            TargetRegistry::new(&[address("192.168.1.3:50000")], &GroupLayout::default())
                .expect("registry should build");
        let existing = registry
            .mix_state(ip("192.168.1.3"))
            .expect("existing target");

        let added = registry
            .add("192.168.1.9:50000", &GroupLayout::default())
            .expect("a fresh address should be accepted");

        assert_eq!(added, address("192.168.1.9:50000"));
        assert_eq!(
            addresses(&registry),
            vec![address("192.168.1.3:50000"), address("192.168.1.9:50000")]
        );
        // The existing target's state is the same allocation: adding did not rebuild the table.
        assert!(Arc::ptr_eq(
            &existing,
            &registry
                .mix_state(ip("192.168.1.3"))
                .expect("existing target")
        ));
        assert!(registry.mix_state(ip("192.168.1.9")).is_some());
    }

    #[test]
    fn rejects_a_duplicate_ip_naming_the_value() {
        let registry =
            TargetRegistry::new(&[address("192.168.1.3:50000")], &GroupLayout::default())
                .expect("registry should build");

        let error = registry
            .add("192.168.1.3:50001", &GroupLayout::default())
            .expect_err("a duplicate IP must be rejected");

        assert!(
            error.contains("192.168.1.3:50001"),
            "message should name the value: {error}"
        );
        assert!(
            error.contains("192.168.1.3"),
            "message should name the IP: {error}"
        );
        assert_eq!(addresses(&registry).len(), 1);
    }

    #[test]
    fn rejects_an_unresolvable_address_naming_the_value() {
        let registry = TargetRegistry::new(&[], &GroupLayout::default()).expect("empty registry");

        let error = registry
            .add("no-such-host.invalid:50000", &GroupLayout::default())
            .expect_err("an unresolvable host must be rejected");

        assert!(
            error.contains("no-such-host.invalid:50000"),
            "message should name the value: {error}"
        );
        assert!(addresses(&registry).is_empty());
    }

    #[test]
    fn removes_only_the_named_target_and_leaves_the_others_untouched() {
        let registry = TargetRegistry::new(
            &[address("192.168.1.3:50000"), address("192.168.1.4:50000")],
            &GroupLayout::default(),
        )
        .expect("registry should build");
        let kept_mix = registry.mix_state(ip("192.168.1.4")).expect("kept target");
        let kept_counters = {
            let entries = registry.read();
            let kept = entries
                .iter()
                .find(|entry| entry.address.ip() == ip("192.168.1.4"))
                .expect("kept target");
            kept.counters.sent.store(7, Ordering::Relaxed);
            Arc::clone(&kept.counters)
        };

        let removed = registry
            .remove("192.168.1.3:50000")
            .expect("configured target should be removed");

        assert_eq!(removed, address("192.168.1.3:50000"));
        assert_eq!(addresses(&registry), vec![address("192.168.1.4:50000")]);
        assert!(registry.mix_state(ip("192.168.1.3")).is_none());
        assert!(Arc::ptr_eq(
            &kept_mix,
            &registry.mix_state(ip("192.168.1.4")).expect("kept target")
        ));
        assert_eq!(kept_counters.sent.load(Ordering::Relaxed), 7);
        assert!(Arc::ptr_eq(&kept_counters, &{
            let entries = registry.read();
            let kept = entries
                .iter()
                .find(|entry| entry.address.ip() == ip("192.168.1.4"))
                .expect("kept target");
            Arc::clone(&kept.counters)
        }));
    }

    #[test]
    fn removing_an_address_that_is_not_configured_names_the_value() {
        let registry = TargetRegistry::new(&[], &GroupLayout::default()).expect("empty registry");

        let error = registry
            .remove("192.168.1.9:50000")
            .expect_err("an unknown address must be rejected");

        assert!(
            error.contains("192.168.1.9:50000"),
            "message should name the value: {error}"
        );
    }

    #[test]
    fn construction_rejects_a_duplicate_ip_like_startup_validation() {
        let targets = [address("192.168.1.3:50000"), address("192.168.1.3:50001")];

        let error = match TargetRegistry::new(&targets, &GroupLayout::default()) {
            Ok(_) => panic!("duplicate IPs must be rejected"),
            Err(error) => error,
        };

        assert!(
            error.contains("192.168.1.3"),
            "message names the IP: {error}"
        );
    }

    #[test]
    fn an_added_target_carries_the_group_layout_it_was_added_with() {
        let layout = GroupLayout::new(vec![Group {
            name: "Drums".to_owned(),
            channels: vec![0],
        }]);
        let registry = TargetRegistry::new(&[], &GroupLayout::default()).expect("empty registry");

        registry
            .add("192.168.1.9:50000", &layout)
            .expect("a fresh address should be accepted");

        let state = registry.mix_state(ip("192.168.1.9")).expect("added target");
        assert_eq!(state.snapshot().channel_group[0], Some(0));
    }

    #[test]
    fn resolve_target_names_the_value_when_it_cannot_be_resolved() {
        let error = resolve_target("not-an-address").expect_err("a bare value must be rejected");

        assert!(
            error.contains("not-an-address"),
            "message should name the value: {error}"
        );
    }
}
