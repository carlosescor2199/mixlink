use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub(crate) const MAX_MIX_CHANNELS: usize = 32;
pub(crate) const MAX_GROUPS: usize = 16;
pub(crate) const OUTPUT_CHANNELS: u8 = 2;

/// A named group of source channels, with 0-based indices as the protocol and the mixer use them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub(crate) name: String,
    pub(crate) channels: Vec<usize>,
}

/// The groups plus the derived channel-to-group lookup.
///
/// Membership is fixed at startup, so the lookup is built once and copied into every [MixValues]
/// snapshot the network thread reads. A channel belongs to at most one group, which validation
/// enforces; the lookup is therefore a single owner per channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GroupLayout {
    groups: Vec<Group>,
    channel_group: [Option<usize>; MAX_MIX_CHANNELS],
}

impl GroupLayout {
    pub(crate) fn new(groups: Vec<Group>) -> Self {
        let mut channel_group = [None; MAX_MIX_CHANNELS];
        for (index, group) in groups.iter().enumerate() {
            for &channel in &group.channels {
                if channel < MAX_MIX_CHANNELS {
                    channel_group[channel] = Some(index);
                }
            }
        }
        Self {
            groups,
            channel_group,
        }
    }

    pub(crate) fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub(crate) fn channel_group(&self) -> [Option<usize>; MAX_MIX_CHANNELS] {
        self.channel_group
    }
}

pub(crate) struct MixState {
    channel_gains: [AtomicU8; MAX_MIX_CHANNELS],
    pans: [AtomicU8; MAX_MIX_CHANNELS],
    channel_muted: [AtomicBool; MAX_MIX_CHANNELS],
    channel_solo: [AtomicBool; MAX_MIX_CHANNELS],
    group_levels: [AtomicU8; MAX_GROUPS],
    channel_group: [Option<usize>; MAX_MIX_CHANNELS],
    volume_percent: AtomicU8,
    max_level_percent: AtomicU8,
    muted: AtomicBool,
}

/// A plain, copyable snapshot of one client's mix.
///
/// This is the engine's public view of the mix: atomics become plain values, so a UI can hold,
/// compare and render it without touching the live state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixValues {
    pub channel_gains: [u8; MAX_MIX_CHANNELS],
    pub pans: [u8; MAX_MIX_CHANNELS],
    pub channel_muted: [bool; MAX_MIX_CHANNELS],
    pub channel_solo: [bool; MAX_MIX_CHANNELS],
    pub group_levels: [u8; MAX_GROUPS],
    pub channel_group: [Option<usize>; MAX_MIX_CHANNELS],
    pub volume_percent: u8,
    pub max_level_percent: u8,
    pub muted: bool,
}

/// Default pan keeps the captured interleaved layout: even sources hard left, odd sources hard
/// right. With two sources and neutral gains this reproduces the pre-pan output exactly.
fn default_pan(index: usize) -> u8 {
    if index % 2 == 0 {
        0
    } else {
        100
    }
}

impl Default for MixState {
    fn default() -> Self {
        Self {
            channel_gains: std::array::from_fn(|_| AtomicU8::new(100)),
            pans: std::array::from_fn(|index| AtomicU8::new(default_pan(index))),
            channel_muted: std::array::from_fn(|_| AtomicBool::new(false)),
            channel_solo: std::array::from_fn(|_| AtomicBool::new(false)),
            group_levels: std::array::from_fn(|_| AtomicU8::new(100)),
            channel_group: [None; MAX_MIX_CHANNELS],
            volume_percent: AtomicU8::new(100),
            max_level_percent: AtomicU8::new(100),
            muted: AtomicBool::new(false),
        }
    }
}

impl Default for MixValues {
    fn default() -> Self {
        Self {
            channel_gains: [100; MAX_MIX_CHANNELS],
            pans: std::array::from_fn(default_pan),
            channel_muted: [false; MAX_MIX_CHANNELS],
            channel_solo: [false; MAX_MIX_CHANNELS],
            group_levels: [100; MAX_GROUPS],
            channel_group: [None; MAX_MIX_CHANNELS],
            volume_percent: 100,
            max_level_percent: 100,
            muted: false,
        }
    }
}

fn store_all<const N: usize>(slots: &[AtomicU8; N], values: &[u8; N]) {
    for (slot, value) in slots.iter().zip(values.iter().copied()) {
        slot.store(value, Ordering::Relaxed);
    }
}

fn store_all_flags(slots: &[AtomicBool; MAX_MIX_CHANNELS], values: &[bool; MAX_MIX_CHANNELS]) {
    for (slot, value) in slots.iter().zip(values.iter().copied()) {
        slot.store(value, Ordering::Relaxed);
    }
}

impl MixState {
    /// Builds a state that carries the server's fixed group membership. Each client's group levels
    /// start neutral, so a client that never sends `group_levels` hears every channel ungrouped.
    pub(crate) fn new(layout: &GroupLayout) -> Self {
        Self {
            channel_group: layout.channel_group(),
            ..Self::default()
        }
    }

    pub(crate) fn update(&self, values: MixValues) {
        store_all(&self.channel_gains, &values.channel_gains);
        store_all(&self.pans, &values.pans);
        store_all_flags(&self.channel_muted, &values.channel_muted);
        store_all_flags(&self.channel_solo, &values.channel_solo);
        store_all(&self.group_levels, &values.group_levels);
        self.volume_percent
            .store(values.volume_percent, Ordering::Relaxed);
        self.max_level_percent
            .store(values.max_level_percent, Ordering::Relaxed);
        self.muted.store(values.muted, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> MixValues {
        MixValues {
            channel_gains: std::array::from_fn(|index| {
                self.channel_gains[index].load(Ordering::Relaxed)
            }),
            pans: std::array::from_fn(|index| self.pans[index].load(Ordering::Relaxed)),
            channel_muted: std::array::from_fn(|index| {
                self.channel_muted[index].load(Ordering::Relaxed)
            }),
            channel_solo: std::array::from_fn(|index| {
                self.channel_solo[index].load(Ordering::Relaxed)
            }),
            group_levels: std::array::from_fn(|index| {
                self.group_levels[index].load(Ordering::Relaxed)
            }),
            channel_group: self.channel_group,
            volume_percent: self.volume_percent.load(Ordering::Relaxed),
            max_level_percent: self.max_level_percent.load(Ordering::Relaxed),
            muted: self.muted.load(Ordering::Relaxed),
        }
    }
}

/// Merges an optional list of 0-100 values into an existing table.
///
/// An absent list preserves the current values, so a client that does not send the field keeps
/// working. Values are clamped; an oversized list is rejected rather than silently truncated.
pub(crate) fn merge_levels<const N: usize>(
    label: &str,
    incoming: Option<&Vec<i32>>,
    current: &[u8; N],
) -> Result<[u8; N], String> {
    let Some(values) = incoming else {
        return Ok(*current);
    };
    if values.len() > N {
        return Err(format!(
            "{label} accepts at most {N} values, received {}",
            values.len()
        ));
    }

    let mut merged = *current;
    for (slot, value) in merged.iter_mut().zip(values.iter()) {
        *slot = (*value).clamp(0, 100) as u8;
    }
    Ok(merged)
}

/// Merges an optional list of booleans into an existing table.
///
/// Separate from `merge_levels` on purpose: the 0-100 clamp that is right for gains and pans would
/// be meaningless here, and silently reusing it would hide a protocol mistake.
pub(crate) fn merge_flags(
    label: &str,
    incoming: Option<&Vec<bool>>,
    current: &[bool; MAX_MIX_CHANNELS],
) -> Result<[bool; MAX_MIX_CHANNELS], String> {
    let Some(values) = incoming else {
        return Ok(*current);
    };
    if values.len() > MAX_MIX_CHANNELS {
        return Err(format!(
            "{label} accepts at most {MAX_MIX_CHANNELS} flags, received {}",
            values.len()
        ));
    }

    let mut merged = *current;
    for (slot, value) in merged.iter_mut().zip(values.iter()) {
        *slot = *value;
    }
    Ok(merged)
}

/// Equal-power pan law, the console standard: `theta` runs from 0 at hard left to `PI/2` at
/// hard right, so a centred source contributes about `0.707` to each output.
fn pan_gains(pan: u8) -> (f32, f32) {
    let theta = (f32::from(pan) / 100.0) * std::f32::consts::FRAC_PI_2;
    (theta.cos(), theta.sin())
}

fn clamp_sample(value: f32, ceiling: f32) -> i16 {
    value
        .clamp(-ceiling, ceiling)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

/// Mixes one client's interleaved source buffer down to a stereo bus.
///
/// Every source channel contributes to both outputs weighted by its pan, so source channel `k`
/// is no longer bound to output slot `k`. The default pan table keeps the captured layout (even
/// sources hard left, odd sources hard right), which with neutral gains on a two-channel source
/// reproduces the pre-pan output exactly, apart from the full-scale ceiling clamp that also
/// applied before. Master volume, mute and the ceiling are applied after the sum.
///
/// A channel contributes only when it is not muted and either nothing is soloed or it is one of
/// the soloed channels. Solo is applied here rather than by zeroing gains, so coming out of solo
/// restores exactly the levels the musician had set.
pub(crate) fn mix_channels(source: &[i16], channels: usize, values: MixValues) -> Vec<i16> {
    if channels == 0 || source.len() < channels {
        return source.to_vec();
    }

    let frames = source.len() / channels;
    let active = channels.min(MAX_MIX_CHANNELS);
    let any_solo = (0..active).any(|channel| values.channel_solo[channel]);
    let ceiling = (i32::from(i16::MAX) * i32::from(values.max_level_percent) / 100) as f32;
    let master = f32::from(values.volume_percent) / 100.0;
    let mut output = vec![0i16; frames * usize::from(OUTPUT_CHANNELS)];

    for frame in 0..frames {
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for channel in 0..active {
            if values.channel_muted[channel] {
                continue;
            }
            if any_solo && !values.channel_solo[channel] {
                continue;
            }
            // A grouped channel is scaled by its group level; an ungrouped channel uses 1.0, and
            // `x * 1.0 == x` for every f32, so the no-group case stays byte-identical to the
            // pre-group arithmetic.
            let group_gain = match values.channel_group[channel] {
                Some(group) => f32::from(values.group_levels[group]) / 100.0,
                None => 1.0,
            };
            let gain = f32::from(values.channel_gains[channel]) / 100.0 * group_gain * master;
            if gain == 0.0 || values.muted {
                continue;
            }
            let sample = f32::from(source[frame * channels + channel]);
            let (left_weight, right_weight) = pan_gains(values.pans[channel]);
            left += sample * gain * left_weight;
            right += sample * gain * right_weight;
        }
        output[frame * 2] = clamp_sample(left, ceiling);
        output[frame * 2 + 1] = clamp_sample(right, ceiling);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_mix_states;
    use crate::protocol::parse_mix_command;
    use std::net::IpAddr;

    #[test]
    fn independent_mix_states_transform_the_same_input_differently() {
        let targets = vec![
            "192.168.1.3:50000".parse().unwrap(),
            "192.168.1.4:50000".parse().unwrap(),
        ];
        let states = build_mix_states(&targets, &GroupLayout::default()).unwrap();
        states
            .get(&"192.168.1.3".parse().unwrap())
            .unwrap()
            .update(MixValues {
                volume_percent: 50,
                ..MixValues::default()
            });
        states
            .get(&"192.168.1.4".parse().unwrap())
            .unwrap()
            .update(MixValues::default());

        let input = [20_000, -20_000];
        let first_output = mix_channels(
            &input,
            2,
            states
                .get(&"192.168.1.3".parse().unwrap())
                .unwrap()
                .snapshot(),
        );
        let second_output = mix_channels(
            &input,
            2,
            states
                .get(&"192.168.1.4".parse().unwrap())
                .unwrap()
                .snapshot(),
        );

        assert_eq!(first_output, [10_000, -10_000]);
        assert_eq!(second_output, input);
    }

    #[test]
    fn updating_one_mix_state_does_not_change_another() {
        let targets = vec![
            "192.168.1.3:50000".parse().unwrap(),
            "192.168.1.4:50000".parse().unwrap(),
        ];
        let states = build_mix_states(&targets, &GroupLayout::default()).unwrap();
        let first_ip: IpAddr = "192.168.1.3".parse().unwrap();
        let second_ip: IpAddr = "192.168.1.4".parse().unwrap();
        let second_state = states.get(&second_ip).unwrap();
        let initial_second = second_state.snapshot();

        states.get(&first_ip).unwrap().update(MixValues {
            volume_percent: 0,
            max_level_percent: 10,
            muted: true,
            ..MixValues::default()
        });

        assert_eq!(second_state.snapshot(), initial_second);
    }

    #[test]
    fn clamps_mix_command_and_applies_gain_ceiling() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":150,"max_level_percent":25,"muted":false}"#,
            MixValues::default(),
        )
        .unwrap();
        assert_eq!(values.volume_percent, 100);
        assert_eq!(values.max_level_percent, 25);

        let samples = mix_channels(&[i16::MIN, -16_000, 16_000, i16::MAX], 2, values);
        assert_eq!(samples, [-8191, -8191, 8191, 8191]);
    }

    #[test]
    fn mute_zeroes_samples_and_rejects_other_message_types() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":true}"#,
            MixValues::default(),
        )
        .unwrap();
        let samples = mix_channels(&[i16::MIN, 0, i16::MAX, 100], 2, values);
        assert_eq!(samples, [0, 0, 0, 0]);
        assert!(parse_mix_command(
            r#"{"type":"status","volume_percent":80,"max_level_percent":90,"muted":false}"#,
            MixValues::default(),
        )
        .is_err());
    }

    #[test]
    fn neutral_channel_gains_and_default_pans_leave_the_buffer_unchanged() {
        let original = [-16_000, -8_000, 8_000, 16_000];

        let samples = mix_channels(&original, 2, MixValues::default());

        assert_eq!(samples, original);
    }

    #[test]
    fn gain_table_holds_one_entry_per_supported_channel() {
        assert_eq!(MixValues::default().channel_gains.len(), MAX_MIX_CHANNELS);
    }

    #[test]
    fn per_channel_gains_transform_only_their_own_channel() {
        let values = MixValues {
            channel_gains: std::array::from_fn(|index| if index == 1 { 50 } else { 100 }),
            ..MixValues::default()
        };

        let samples = mix_channels(&[10_000, 10_000, -10_000, -10_000], 2, values);

        assert_eq!(samples, [10_000, 5_000, -10_000, -5_000]);
    }

    #[test]
    fn absent_channels_field_preserves_existing_gains() {
        let current = MixValues {
            channel_gains: std::array::from_fn(|index| if index == 0 { 40 } else { 70 }),
            ..MixValues::default()
        };

        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#,
            current,
        )
        .unwrap();

        assert_eq!(values.channel_gains, current.channel_gains);
        assert_eq!(values.volume_percent, 80);
    }

    #[test]
    fn channel_gains_are_clamped_and_oversized_lists_are_rejected() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"channels":[150,-20,100]}"#,
            MixValues::default(),
        )
        .unwrap();

        assert_eq!(&values.channel_gains[..3], &[100, 0, 100]);
        assert_eq!(values.channel_gains[3], 100);

        let oversized = format!(
            r#"{{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"channels":[{}]}}"#,
            vec!["100"; MAX_MIX_CHANNELS + 1].join(",")
        );
        assert!(parse_mix_command(&oversized, MixValues::default()).is_err());
    }

    #[test]
    fn centring_a_source_places_it_in_both_outputs() {
        let values = MixValues {
            pans: std::array::from_fn(|_| 50),
            ..MixValues::default()
        };

        let samples = mix_channels(&[10_000, 0], 2, values);

        // cos(PI/4) == sin(PI/4), so both outputs get the same share.
        assert_eq!(samples, [7_071, 7_071]);
    }

    #[test]
    fn panning_everything_left_empties_the_right_output() {
        let values = MixValues {
            pans: std::array::from_fn(|_| 0),
            ..MixValues::default()
        };

        let samples = mix_channels(&[10_000, -4_000], 2, values);

        assert_eq!(samples, [6_000, 0]);
    }

    #[test]
    fn a_multichannel_source_is_summed_into_a_stereo_output() {
        let samples = mix_channels(&[1_000, 2_000, 3_000, 4_000], 4, MixValues::default());

        assert_eq!(samples.len(), 2);
        assert_eq!(samples, [4_000, 6_000]);
    }

    #[test]
    fn absent_pans_field_preserves_existing_pans_and_oversized_lists_are_rejected() {
        let current = MixValues {
            pans: std::array::from_fn(|index| if index == 0 { 25 } else { 75 }),
            ..MixValues::default()
        };

        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#,
            current,
        )
        .unwrap();
        assert_eq!(values.pans, current.pans);

        let clamped = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"pans":[150,-5,50]}"#,
            current,
        )
        .unwrap();
        assert_eq!(&clamped.pans[..3], &[100, 0, 50]);

        let oversized = format!(
            r#"{{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"pans":[{}]}}"#,
            vec!["50"; MAX_MIX_CHANNELS + 1].join(",")
        );
        assert!(parse_mix_command(&oversized, current).is_err());
    }

    #[test]
    fn default_pan_reproduces_the_captured_interleaved_layout() {
        let values = MixValues::default();

        assert_eq!(values.pans[0], 0);
        assert_eq!(values.pans[1], 100);
        assert_eq!(values.pans[2], 0);
        assert_eq!(values.pans[3], 100);
    }

    #[test]
    fn solo_silences_every_other_channel() {
        let source = [10_000, 20_000];
        let values = MixValues {
            channel_solo: std::array::from_fn(|index| index == 0),
            ..MixValues::default()
        };

        assert_eq!(mix_channels(&source, 2, values), [10_000, 0]);
    }

    #[test]
    fn clearing_the_last_solo_restores_the_previous_levels() {
        let source = [10_000, 20_000];
        let soloed = MixValues {
            channel_solo: std::array::from_fn(|index| index == 0),
            ..MixValues::default()
        };
        assert_eq!(mix_channels(&source, 2, soloed), [10_000, 0]);

        let cleared = MixValues::default();
        assert_eq!(mix_channels(&source, 2, cleared), source);
    }

    #[test]
    fn muting_a_channel_silences_only_that_channel() {
        let source = [10_000, 20_000];
        let values = MixValues {
            channel_muted: std::array::from_fn(|index| index == 1),
            ..MixValues::default()
        };

        assert_eq!(mix_channels(&source, 2, values), [10_000, 0]);
    }

    #[test]
    fn a_muted_soloed_channel_stays_silent() {
        let source = [10_000, 20_000];
        let values = MixValues {
            channel_muted: std::array::from_fn(|index| index == 0),
            channel_solo: std::array::from_fn(|index| index == 0),
            ..MixValues::default()
        };

        assert_eq!(mix_channels(&source, 2, values), [0, 0]);
    }

    #[test]
    fn solo_applies_on_top_of_the_stored_gains_without_changing_them() {
        let source = [10_000, 20_000];
        let values = MixValues {
            channel_gains: std::array::from_fn(|index| if index == 1 { 50 } else { 100 }),
            channel_solo: std::array::from_fn(|index| index == 1),
            ..MixValues::default()
        };

        assert_eq!(mix_channels(&source, 2, values), [0, 10_000]);
    }

    #[test]
    fn absent_mutes_and_solos_preserve_the_stored_flags() {
        let current = MixValues {
            channel_muted: std::array::from_fn(|index| index == 1),
            channel_solo: std::array::from_fn(|index| index == 0),
            ..MixValues::default()
        };

        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#,
            current,
        )
        .unwrap();

        assert_eq!(values.channel_muted, current.channel_muted);
        assert_eq!(values.channel_solo, current.channel_solo);
    }

    #[test]
    fn mutes_and_solos_are_applied_and_oversized_lists_are_rejected() {
        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"mutes":[true,false],"solos":[false,true]}"#,
            MixValues::default(),
        )
        .unwrap();

        assert!(values.channel_muted[0]);
        assert!(!values.channel_muted[1]);
        assert!(!values.channel_solo[0]);
        assert!(values.channel_solo[1]);

        let oversized = format!(
            r#"{{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"mutes":[{}]}}"#,
            vec!["false"; MAX_MIX_CHANNELS + 1].join(",")
        );
        assert!(parse_mix_command(&oversized, MixValues::default()).is_err());
    }

    #[test]
    fn a_group_level_scales_every_member_and_keeps_their_balance() {
        let layout = GroupLayout::new(vec![Group {
            name: "Drums".to_owned(),
            channels: vec![0, 1],
        }]);
        let mut group_levels = [100u8; MAX_GROUPS];
        group_levels[0] = 50;
        let values = MixValues {
            channel_group: layout.channel_group(),
            group_levels,
            ..MixValues::default()
        };

        let samples = mix_channels(&[20_000, 20_000], 2, values);

        // Both members are halved, and because their pans are opposite the sum lands in each
        // output at the same reduced level: the relative balance survives.
        assert_eq!(samples, [10_000, 10_000]);
    }

    #[test]
    fn a_channel_in_no_group_ignores_every_group_level() {
        let layout = GroupLayout::new(vec![Group {
            name: "Drums".to_owned(),
            channels: vec![0],
        }]);
        let mut group_levels = [100u8; MAX_GROUPS];
        group_levels[0] = 0;
        let values = MixValues {
            channel_group: layout.channel_group(),
            group_levels,
            ..MixValues::default()
        };

        let samples = mix_channels(&[10_000, 10_000], 2, values);

        assert_eq!(samples, [0, 10_000]);
    }

    #[test]
    fn no_groups_reproduces_the_current_mix_byte_for_byte() {
        let source = [-16_000, -8_000, 8_000, 16_000];
        let layout = GroupLayout::default();
        let without_groups = MixValues {
            channel_group: layout.channel_group(),
            ..MixValues::default()
        };

        assert_eq!(mix_channels(&source, 2, without_groups), source);
        assert_eq!(
            mix_channels(&source, 2, without_groups),
            mix_channels(&source, 2, MixValues::default())
        );
    }

    #[test]
    fn absent_group_levels_preserve_and_oversized_lists_are_rejected() {
        let mut current = MixValues::default();
        current.group_levels[0] = 40;

        let preserved = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false}"#,
            current,
        )
        .unwrap();
        assert_eq!(preserved.group_levels, current.group_levels);

        let clamped = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"group_levels":[150,-5,50]}"#,
            current,
        )
        .unwrap();
        assert_eq!(&clamped.group_levels[..3], &[100, 0, 50]);

        let oversized = format!(
            r#"{{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"group_levels":[{}]}}"#,
            vec!["50"; MAX_GROUPS + 1].join(",")
        );
        assert!(parse_mix_command(&oversized, current).is_err());
    }

    #[test]
    fn a_mix_update_preserves_the_servers_group_membership() {
        let layout = GroupLayout::new(vec![Group {
            name: "Drums".to_owned(),
            channels: vec![0],
        }]);

        let values = parse_mix_command(
            r#"{"type":"mix","volume_percent":80,"max_level_percent":90,"muted":false,"group_levels":[50]}"#,
            MixValues {
                channel_group: layout.channel_group(),
                ..MixValues::default()
            },
        )
        .unwrap();

        assert_eq!(values.channel_group[0], Some(0));
        assert_eq!(values.group_levels[0], 50);
    }
}
