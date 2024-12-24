//! Debug consensus client.
//!
//! This is a worker that sends FCUs and new payloads by fetching recent blocks from an external
//! provider like Etherscan or an RPC endpoint. This allows to quickly test the execution client
//! without running a consensus node.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod client;
mod providers;

pub use client::{block_to_execution_payload_v3, BlockProvider, DebugConsensusClient};
pub use providers::{EtherscanBlockProvider, RpcBlockProvider};
