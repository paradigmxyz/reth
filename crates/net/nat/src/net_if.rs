//! IP resolution on non-host Docker network.

use std::{io, net::IpAddr};

/// The 'eth0' interface tends to be the default interface that docker containers use to
/// communicate with each other.
pub const DEFAULT_NET_IF_NAME: &str = "eth0";

/// Errors resolving network interface IP.
#[derive(Debug, thiserror::Error)]
pub enum NetInterfaceError {
    /// Error reading OS interfaces.
    #[error("failed to read OS interfaces: {0}")]
    Io(io::Error),
    /// No interface found with given name.
    #[error("interface not found: {0}, found other interfaces: {1:?}")]
    IFNotFound(String, Vec<String>),
}

/// Reads IP of OS interface with given name, if exists.
pub fn resolve_net_if_ip(if_name: &str) -> Result<IpAddr, NetInterfaceError> {
    match if_addrs::get_if_addrs() {
        Ok(ifs) => {
            let ip = ifs.iter().find(|i| i.name == if_name).map(|i| i.ip());
            match ip {
                Some(ip) => Ok(ip),
                None => {
                    let ifs = ifs.into_iter().map(|i| i.name.as_str().into()).collect();
                    Err(NetInterfaceError::IFNotFound(if_name.into(), ifs))
                }
            }
        }
        Err(err) => Err(NetInterfaceError::Io(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn read_docker_if_addr() {
        const LOCALHOST_IF: [&str; 2] = ["lo0", "lo"];

        let ip = resolve_net_if_ip(LOCALHOST_IF[0])
            .unwrap_or_else(|_| resolve_net_if_ip(LOCALHOST_IF[1]).unwrap());

        assert_eq!(ip, Ipv4Addr::LOCALHOST);
    }
}
