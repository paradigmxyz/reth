//! Debug consensus client.
//!
//! This is a worker that sends FCUs and new payloads by fetching recent blocks from an external
//! provider like Etherscan or an RPC endpoint. This allows to quickly test the execution client
//! without running a consensus node.
mod client;
mod providers;

pub use client::{BlockProvider, DebugConsensusClient};
pub use providers::{EtherscanBlockProvider, RpcBlockProvider};
