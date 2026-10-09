use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};

const DEFAULT_TARGET: &str = "127.0.0.1:50000";
const DEFAULT_CONTROL_PORT: u16 = 50001;

pub(crate) struct Arguments {
    pub(crate) device_filter: Option<String>,
    pub(crate) targets: Vec<String>,
    pub(crate) control_port: u16,
}

pub(crate) fn parse_arguments() -> Result<Arguments, Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let mut device_filter = None;
    let mut targets = Vec::new();
    let mut control_port = DEFAULT_CONTROL_PORT;

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

    Ok(Arguments {
        device_filter,
        targets,
        control_port,
    })
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

    fn parse_arguments_from<const N: usize>(
        values: [&str; N],
    ) -> Result<Arguments, Box<dyn Error>> {
        let mut arguments = values.into_iter();
        let mut device_filter = None;
        let mut targets = Vec::new();
        let mut control_port = DEFAULT_CONTROL_PORT;

        while let Some(argument) = arguments.next() {
            match argument {
                "--device" => {
                    device_filter = Some(arguments.next().ok_or("missing device")?.to_owned())
                }
                "--target" => targets.push(arguments.next().ok_or("missing target")?.to_owned()),
                "--control-port" => {
                    control_port = arguments.next().ok_or("missing control port")?.parse()?
                }
                _ => return Err(format!("unknown argument: {argument}").into()),
            }
        }

        if targets.is_empty() {
            targets.push(DEFAULT_TARGET.to_owned());
        }

        Ok(Arguments {
            device_filter,
            targets,
            control_port,
        })
    }
}
