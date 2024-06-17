use clap::Args;

pub const DEFAULT_DISCOVERY_PORT: u16 = 30304;
pub const DEFAULT_RLPX_PORT: u16 = 30303;

#[derive(Debug, Clone, Args)]
pub(crate) struct Discv5ArgsExt {
    /// TCP port used by RLPx
    #[clap(long = "exex-discv5.tcp-port", default_value_t = DEFAULT_RLPX_PORT)]
    pub tcp_port: u16,

    /// UDP port used for discovery
    #[clap(long = "exex-discv5.udp-port", default_value_t = DEFAULT_DISCOVERY_PORT)]
    pub udp_port: u16,
}
