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
use dirs::{KnownPeersPath, PlatformPath};
use std::net::IpAddr;

use reth_rpc_builder::RpcModuleConfig;
pub use reth_staged_sync::utils;

use clap::Args;
use reth_primitives::NodeRecord;

/// Parameters for configuring the network more granularity via CLI
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

    /// The path to the known peers file. Connected peers are
    /// dumped to this file on node shutdown, and read on startup.
    /// Cannot be used with --no-persist-peers
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    peers_file: PlatformPath<KnownPeersPath>,

    /// Do not persist peers. Cannot be used with --peers-file
    #[arg(long, verbatim_doc_comment, conflicts_with = "peers_file")]
    no_persist_peers: bool,
}

/// Parameters for configuring the rpc more granularity via CLI
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn test_rpc_server_opts_parser() {
        let opts =
            CommandParser::<RpcServerOpts>::parse_from(["reth", "--http.api", "eth,admin,debug"])
                .args;

        let apis = opts.http_api.unwrap();
        let expected = RpcModuleConfig::try_from_selection(["eth", "admin", "debug"]).unwrap();

        assert_eq!(apis, expected);
    }
}
