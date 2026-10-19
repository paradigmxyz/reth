//! `RLPx` (TCP) and `Discovery` (UDP) sockets of a peer.

use std::net::{IpAddr, SocketAddr};

/// Represents a peer's address information.
///
/// # Fields
///
/// - `tcp`: A `SocketAddr` representing the peer's data transfer address.
/// - `udp`: An optional `SocketAddr` representing the peer's discover address. `None` if the peer
///   is directly connecting to us or the port is the same to `tcp`'s
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PeerAddr {
    tcp: SocketAddr,
    udp: Option<SocketAddr>,
}

impl PeerAddr {
    /// Returns the peer's TCP address.
    pub const fn tcp(&self) -> SocketAddr {
        self.tcp
    }

    /// Returns the peer's UDP address.
    pub const fn udp(&self) -> Option<SocketAddr> {
        self.udp
    }

    /// Returns a new `PeerAddr` with the given `tcp` and `udp` addresses.
    pub const fn new(tcp: SocketAddr, udp: Option<SocketAddr>) -> Self {
        Self { tcp, udp }
    }

    /// Returns a new `PeerAddr` with a `tcp` address only.
    pub const fn from_tcp(tcp: SocketAddr) -> Self {
        Self { tcp, udp: None }
    }

    /// Returns a new `PeerAddr` with the given `tcp` and `udp` ports.
    pub fn new_with_ports(ip: IpAddr, tcp_port: u16, udp_port: Option<u16>) -> Self {
        let tcp = SocketAddr::new(ip, tcp_port);
        let udp = udp_port.map(|port| SocketAddr::new(ip, port));
        Self::new(tcp, udp)
    }
}
