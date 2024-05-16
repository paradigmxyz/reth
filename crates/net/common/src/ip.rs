//! Utility methods for [`IpAddr`] variants.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Syntactic sugar for working with IP versions.
pub trait IpAddrExt {
    /// Returns the address if this is an [`Ipv4Addr`].
    fn ipv4(&self) -> Option<&Ipv4Addr>;

    /// Returns the address if this is an [`Ipv6Addr`].
    fn ipv6(&self) -> Option<&Ipv6Addr>;
}

impl IpAddrExt for IpAddr {
    fn ipv4(&self) -> Option<&Ipv4Addr> {
        match self {
            IpAddr::V4(ip) => Some(ip),
            IpAddr::V6(_) => None,
        }
    }

    fn ipv6(&self) -> Option<&Ipv6Addr> {
        match self {
            IpAddr::V4(_) => None,
            IpAddr::V6(ip) => Some(ip),
        }
    }
}
