use std::net::{Ipv4Addr, Ipv6Addr};

pub const DEFAULT_DISCOVERY_PORT: u16 = 30305;
pub const DEFAULT_DISCOVERY_ADDR_V4: Ipv4Addr = Ipv4Addr::UNSPECIFIED;
pub const DEFAULT_DISCOVERY_ADDR_V6: Ipv6Addr = Ipv6Addr::UNSPECIFIED;

#[derive(Debug, Clone, clap::Args)]
pub struct Discv5ArgsExt {
    /// IPv4 Network listening address
    #[clap(long = "exex-discv5.addr-v4", value_name = "ADDR_V4", default_value_t = DEFAULT_DISCOVERY_ADDR_V4)]
    pub addr_v4: Ipv4Addr,

    /// IPv6 Network listening address
    #[clap(long = "exex-discv5.addr-v6", value_name = "ADDR_V6", default_value_t = DEFAULT_DISCOVERY_ADDR_V6)]
    pub addr_v6: Ipv6Addr,

    /// Network listening port
    #[clap(long = "exex-discv5.port", value_name = "PORT", default_value_t = DEFAULT_DISCOVERY_PORT)]
    pub port: u16,
}
