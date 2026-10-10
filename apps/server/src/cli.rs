use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::mix::{Group, GroupLayout, MAX_GROUPS};

const DEFAULT_TARGET: &str = "127.0.0.1:50000";
const DEFAULT_CONTROL_PORT: u16 = 50001;

/// The engine's startup configuration, independent of how it was gathered.
///
/// The command line fills this in [parse_arguments], but a UI can build one directly and hand it to
/// [crate::start] without parsing anything.
#[derive(Debug)]
pub struct EngineConfig {
    pub device_filter: Option<String>,
    pub targets: Vec<String>,
    pub control_port: u16,
    pub groups: Vec<GroupDefinition>,
}

/// The crate-internal name the parsing tests use for [EngineConfig].
#[cfg(test)]
pub(crate) type Arguments = EngineConfig;

/// A group as the engineer typed it: a name and 1-based channel numbers.
///
/// The conversion to the protocol's 0-based indices needs the captured channel count, so it happens
/// in [validate_groups] once the input device has been selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupDefinition {
    pub name: String,
    pub channels: Vec<usize>,
}

pub fn parse_arguments() -> Result<EngineConfig, Box<dyn Error>> {
    parse_from(env::args().skip(1))
}

fn parse_from(arguments: impl Iterator<Item = String>) -> Result<EngineConfig, Box<dyn Error>> {
    let mut arguments = arguments;
    let mut device_filter = None;
    let mut targets = Vec::new();
    let mut control_port = DEFAULT_CONTROL_PORT;
    let mut groups = Vec::new();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--device" => {
                device_filter = Some(
                    arguments
                        .next()
                        .ok_or("--device requires a search string")?,
                );
            }
            "--target" => {
                targets.push(
                    arguments
                        .next()
                        .ok_or("--target requires a host:port value")?,
                );
            }
            "--group" => {
                let value = arguments
                    .next()
                    .ok_or("--group requires a Name=1,2,3 value")?;
                groups.push(parse_group(&value)?);
            }
            "--control-port" => {
                let port = arguments
                    .next()
                    .ok_or("--control-port requires a port")?
                    .parse()
                    .map_err(|_| "--control-port must be a valid TCP port")?;
                if port == 0 {
                    return Err("--control-port must be between 1 and 65535".into());
                }
                control_port = port;
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }

    if targets.is_empty() {
        targets.push(DEFAULT_TARGET.to_owned());
    }

    Ok(EngineConfig {
        device_filter,
        targets,
        control_port,
        groups,
    })
}

/// Parses one `Name=1,2,3` group value. Every error names the offending value, so a typo is easy to
/// spot on a command line.
fn parse_group(value: &str) -> Result<GroupDefinition, Box<dyn Error>> {
    let (name, channels) = value
        .split_once('=')
        .ok_or_else(|| format!("--group `{value}` must be written as Name=1,2,3"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("--group `{value}` has an empty name").into());
    }
    if channels.trim().is_empty() {
        return Err(format!("--group `{value}` has an empty channel list").into());
    }

    let mut parsed = Vec::new();
    for channel in channels.split(',') {
        let channel = channel.trim();
        let number = channel
            .parse()
            .map_err(|_| format!("--group `{value}` has a non-numeric channel `{channel}`"))?;
        parsed.push(number);
    }

    Ok(GroupDefinition {
        name: name.to_owned(),
        channels: parsed,
    })
}

/// Validates the engineer's groups against the captured channel count and converts the 1-based
/// command-line channels into the 0-based indices the protocol and mixer use.
///
/// This runs after the input device is selected because the channel count is only known then. A
/// group that cannot be validated is a startup error, not a silent no-op.
pub(crate) fn validate_groups(
    definitions: &[GroupDefinition],
    channel_count: usize,
) -> Result<GroupLayout, Box<dyn Error>> {
    if definitions.len() > MAX_GROUPS {
        return Err(format!(
            "at most {MAX_GROUPS} groups are supported, received {}",
            definitions.len()
        )
        .into());
    }

    let mut owner: Vec<Option<&str>> = vec![None; channel_count];
    let mut groups = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let mut channels = Vec::with_capacity(definition.channels.len());
        for &channel in &definition.channels {
            if channel == 0 || channel > channel_count {
                return Err(format!(
                    "group \"{}\" names channel {channel}, outside the captured range of 1..={channel_count}",
                    definition.name
                )
                .into());
            }
            let index = channel - 1;
            if let Some(previous) = owner[index] {
                return Err(format!(
                    "channel {channel} is listed in both \"{previous}\" and \"{}\"",
                    definition.name
                )
                .into());
            }
            owner[index] = Some(&definition.name);
            channels.push(index);
        }
        groups.push(Group {
            name: definition.name.clone(),
            channels,
        });
    }

    Ok(GroupLayout::new(groups))
}

pub(crate) fn resolve_targets(targets: &[String]) -> Result<Vec<SocketAddr>, Box<dyn Error>> {
    let resolved = targets
        .iter()
        .map(|target| {
            target
                .to_socket_addrs()
                .map_err(|error| format!("invalid --target `{target}`: {error}"))?
                .next()
                .ok_or_else(|| format!("invalid --target `{target}`: it resolved to no address"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| -> Box<dyn Error> { error.into() })?;

    let mut seen_ips = HashMap::new();
    for target in &resolved {
        if let Some(previous) = seen_ips.insert(target.ip(), *target) {
            return Err(format!(
                "duplicate UDP target IP {} is not supported (targets {} and {})",
                target.ip(),
                previous,
                target
            )
            .into());
        }
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_and_target_arguments() {
        let arguments = parse_arguments_from(["--device", "Volt", "--target", "localhost:6000"])
            .expect("arguments should parse");

        assert_eq!(arguments.device_filter.as_deref(), Some("Volt"));
        assert_eq!(arguments.targets, vec!["localhost:6000"]);
        assert_eq!(arguments.control_port, DEFAULT_CONTROL_PORT);
    }

    #[test]
    fn parses_repeated_target_arguments() {
        let arguments = parse_arguments_from([
            "--target",
            "192.168.1.3:50000",
            "--target",
            "192.168.1.4:50000",
        ])
        .expect("arguments should parse");

        assert_eq!(
            arguments.targets,
            vec!["192.168.1.3:50000", "192.168.1.4:50000"]
        );
    }

    #[test]
    fn defaults_to_one_target_when_not_configured() {
        let arguments = parse_arguments_from::<0>([]).expect("arguments should parse");

        assert_eq!(arguments.targets, vec![DEFAULT_TARGET]);
    }

    #[test]
    fn rejects_targets_with_the_same_ip() {
        let targets = vec![
            "192.168.1.3:50000".to_owned(),
            "192.168.1.3:50001".to_owned(),
        ];

        let error = resolve_targets(&targets).unwrap_err();

        assert!(error
            .to_string()
            .contains("duplicate UDP target IP 192.168.1.3"));
    }

    #[test]
    fn parses_repeated_group_arguments_in_order() {
        let arguments = parse_arguments_from(["--group", "Drums=1,2,3", "--group", "Vocals=4,5"])
            .expect("arguments should parse");

        assert_eq!(arguments.groups.len(), 2);
        assert_eq!(arguments.groups[0].name, "Drums");
        assert_eq!(arguments.groups[0].channels, vec![1, 2, 3]);
        assert_eq!(arguments.groups[1].name, "Vocals");
        assert_eq!(arguments.groups[1].channels, vec![4, 5]);
    }

    #[test]
    fn rejects_group_without_an_equals_sign() {
        let error = parse_arguments_from(["--group", "Drums1,2"]).unwrap_err();

        assert!(error.to_string().contains("Drums1,2"));
    }

    #[test]
    fn rejects_group_with_an_empty_name() {
        let error = parse_arguments_from(["--group", "=1,2"]).unwrap_err();

        assert!(error.to_string().contains("empty name"));
    }

    #[test]
    fn rejects_group_with_an_empty_channel_list() {
        let error = parse_arguments_from(["--group", "Drums="]).unwrap_err();

        assert!(error.to_string().contains("empty channel"));
    }

    #[test]
    fn rejects_group_with_a_non_numeric_channel_naming_the_value() {
        let error = parse_arguments_from(["--group", "Drums=1,kick"]).unwrap_err();

        assert!(error.to_string().contains("kick"));
    }

    #[test]
    fn validates_group_channels_against_the_captured_count() {
        let definitions = vec![GroupDefinition {
            name: "Drums".to_owned(),
            channels: vec![1, 7],
        }];

        let error = validate_groups(&definitions, 2).unwrap_err();

        assert!(error.to_string().contains("Drums"));
        assert!(error.to_string().contains('7'));
    }

    #[test]
    fn rejects_a_channel_listed_in_two_groups_naming_both() {
        let definitions = vec![
            GroupDefinition {
                name: "Drums".to_owned(),
                channels: vec![1, 2],
            },
            GroupDefinition {
                name: "Vocals".to_owned(),
                channels: vec![2],
            },
        ];

        let error = validate_groups(&definitions, 2).unwrap_err();
        let message = error.to_string();

        assert!(message.contains("Drums"));
        assert!(message.contains("Vocals"));
        assert!(message.contains('2'));
    }

    #[test]
    fn converts_validated_groups_to_zero_based_protocol_channels() {
        let definitions = vec![GroupDefinition {
            name: "Drums".to_owned(),
            channels: vec![1, 2],
        }];

        let layout = validate_groups(&definitions, 2).expect("groups should validate");

        assert_eq!(layout.groups().len(), 1);
        assert_eq!(layout.groups()[0].name, "Drums");
        assert_eq!(layout.groups()[0].channels, vec![0, 1]);
    }

    #[test]
    fn rejects_more_groups_than_the_supported_maximum() {
        let definitions = (0..MAX_GROUPS + 1)
            .map(|index| GroupDefinition {
                name: format!("Group {index}"),
                channels: vec![1],
            })
            .collect::<Vec<_>>();

        let error = validate_groups(&definitions, 2).unwrap_err();

        assert!(error.to_string().contains(&MAX_GROUPS.to_string()));
    }

    fn parse_arguments_from<const N: usize>(
        values: [&str; N],
    ) -> Result<Arguments, Box<dyn Error>> {
        parse_from(values.into_iter().map(str::to_owned))
    }
}
