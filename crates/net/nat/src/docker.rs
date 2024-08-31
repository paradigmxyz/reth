//! IP resolution on non-host Docker network.

#![cfg(not(target_os = "windows"))]

use std::{io, net::IpAddr};

/// The docker LAN interface.
pub const DOCKER_LAN_IF: &str = "eth0";

/// Errors resolving Docker IP.
#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    /// Error reading OS interfaces.
    #[error("failed to read OS interfaces: {0}")]
    Io(io::Error),
    /// No interface found with given name.
    #[error("interface not found: {0}, found other interfaces: {1:?}")]
    IFNotFound(String, Vec<String>),
}

/// Reads IP of OS interface with given name, if exists.
#[cfg(not(target_os = "windows"))]
pub fn resolve_docker_ip(if_name: &str) -> Result<IpAddr, DockerError> {
    match if_addrs::get_if_addrs() {
        Ok(ifs) => {
            let ip = ifs.iter().find(|i| i.name == if_name).map(|i| i.ip());
            match ip {
                Some(ip) => Ok(ip),
                None => {
                    let ifs = ifs.into_iter().map(|i| i.name.as_str().into()).collect();
                    Err(DockerError::IFNotFound(if_name.into(), ifs))
                }
            }
        }
        Err(err) => Err(DockerError::Io(err)),
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn read_docker_if_addr() {
        const LOCALHOST_IF: &str = "lo0";
        assert_eq!(resolve_docker_ip(LOCALHOST_IF).unwrap(), Ipv4Addr::LOCALHOST);
    }
}
