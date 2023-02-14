//! clap [Args](clap::Args) for network related arguments.

use crate::dirs::{KnownPeersPath, PlatformPath};
use clap::Args;
use reth_primitives::NodeRecord;

/// Parameters for configuring the network more granularity via CLI
#[derive(Debug, Args)]
#[command(next_help_heading = "Networking")]
pub struct NetworkArgs {
    /// Disable the discovery service.
    #[arg(short, long)]
    pub disable_discovery: bool,

    /// Target trusted peer enodes
    /// --trusted-peers enode://abcd@192.168.0.1:30303
    #[arg(long)]
    pub trusted_peers: Vec<NodeRecord>,

    /// Connect only to trusted peers
    #[arg(long)]
    pub trusted_only: bool,

    /// Nodes to bootstrap network discovery.
    ///
    /// Will fall back to a network-specific default if not specified.
    #[arg(long, value_delimiter = ',')]
    pub bootnodes: Option<Vec<NodeRecord>>,

    /// The path to the known peers file. Connected peers are
    /// dumped to this file on node shutdown, and read on startup.
    /// Cannot be used with --no-persist-peers
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    pub peers_file: PlatformPath<KnownPeersPath>,

    /// Do not persist peers. Cannot be used with --peers-file
    #[arg(long, verbatim_doc_comment, conflicts_with = "peers_file")]
    pub no_persist_peers: bool,
}
