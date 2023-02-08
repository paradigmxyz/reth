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
use reth_rpc_builder::RethRpcModule;
pub use reth_staged_sync::utils;

use clap::Args;
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
#[derive(Debug, Args)]
#[command(next_help_heading = "Rpc")]
struct RpcServerOpts {
    #[arg(long, default_value = "true")]
    http: bool,

    #[arg(long = "http.addr")]
    http_addr: Option<String>,

    #[arg(long = "http.port")]
    http_port: Option<u128>,

    #[arg(long = "http.api", value_delimiter = ',')]
    http_api: Option<Vec<RethRpcModule>>,

    #[arg(long, default_value = "true")]
    ws: bool,

    #[arg(long = "ws.addr")]
    ws_addr: Option<String>,

    #[arg(long = "ws.port")]
    ws_port: Option<u128>,

    #[arg(long = "ws.api", value_delimiter = ',')]
    ws_api: Option<Vec<RethRpcModule>>,

    #[arg(long, default_value = "true")]
    ipcdisable: bool,

    #[arg(long)]
    ipcpath: Option<String>,
}
