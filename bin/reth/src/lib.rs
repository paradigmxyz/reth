#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! Rust Ethereum (reth) binary executable.

pub mod cli;
pub mod db;
pub mod dirs;
pub mod node;
pub mod p2p;
pub mod prometheus_exporter;
pub mod stage;
pub mod test_eth_chain;
pub mod test_vectors;
use std::net::IpAddr;

use reth_rpc_builder::RpcModuleConfig;
pub use reth_staged_sync::utils;

use clap::{value_parser, Arg, Args, Command};
use reth_primitives::NodeRecord;

/// Parameters for configuring the network more granularly via CLI
#[derive(Debug, Args)]
#[command(next_help_heading = "Networking")]
struct NetworkOpts {
    /// Disable the discovery service.
    #[arg(short, long)]
    disable_discovery: bool,

    /// Target trusted peer enodes
    /// --trusted-peers enode://abcd@192.168.0.1:30303
    #[arg(long)]
    trusted_peers: Vec<NodeRecord>,

    /// Connect only to trusted peers
    #[arg(long)]
    trusted_only: bool,

    /// Bootnodes to connect to initially.
    ///
    /// Will fall back to a network-specific default if not specified.
    #[arg(long, value_delimiter = ',')]
    bootnodes: Option<Vec<NodeRecord>>,
}

/// Parameters for configuring the network more granularly via CLI
#[derive(Debug, Args, PartialEq, Default)]
#[command(next_help_heading = "Rpc")]
struct RpcServerOpts {
    /// Enable the HTTP-RPC server
    #[arg(long)]
    http: bool,

    /// Http server address to listen on
    #[arg(long = "http.addr")]
    http_addr: Option<IpAddr>,

    /// Http server port to listen on
    #[arg(long = "http.port")]
    http_port: Option<u16>,

    /// Rpc Modules to be configured for http server
    #[arg(long = "http.api")]
    http_api: Option<RpcModuleConfig>,

    /// Enable the WS-RPC server
    #[arg(long)]
    ws: bool,

    /// Ws server address to listen on
    #[arg(long = "ws.addr")]
    ws_addr: Option<IpAddr>,

    /// Http server port to listen on
    #[arg(long = "ws.port")]
    ws_port: Option<u16>,

    /// Rpc Modules to be configured for Ws server
    #[arg(long = "ws.api")]
    ws_api: Option<RpcModuleConfig>,

    /// Disable the IPC-RPC  server
    #[arg(long)]
    ipcdisable: bool,

    /// Filename for IPC socket/pipe within the datadir
    #[arg(long)]
    ipcpath: Option<String>,
}

impl RpcServerOpts {
    fn parse_from(input: Vec<&str>) -> Option<RpcServerOpts> {
        let cmd = Command::new("reth").arg(
            Arg::new("http_api").long("http.api").value_parser(value_parser!(RpcModuleConfig)),
        );

        let mut server_opts = RpcServerOpts::default();

        if let Some(api) = cmd.get_matches_from(input).get_one::<RpcModuleConfig>("http_api") {
            server_opts.http_api = Some(api.clone());
        };

        Some(server_opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_server_opts_parser() {
        let opts =
            RpcServerOpts::parse_from(vec!["reth", "--http.api", "eth,admin,debug"]).unwrap();
        let apis = opts.http_api.unwrap();
        let expected = RpcModuleConfig::try_from_selection(["eth", "admin", "debug"]).unwrap();

        assert_eq!(apis, expected);
    }
}
