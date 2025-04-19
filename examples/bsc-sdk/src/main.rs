//! Example for how to hook into the bsc p2p network
//!
//! Run with
//!
//! ```sh
//! cargo run -p bsc-sdk
//! ```
//!
//! This launches a regular reth node overriding the engine api payload builder with our custom.
//!
//! Credits to: <https://merkle.io/blog/modifying-reth-to-build-the-fastest-transaction-network-on-bsc-and-polygon>

use crate::{
    chainspec::Cli, consensus::ParliaConsensus,
    node::network::block_import::service::ImportService as BlockImportService,
};
use chainspec::parser::BscChainSpecParser;
use clap::{Args, Parser};
use node::BscNode;
use reth::builder::NodeHandle;
use std::sync::Arc;
use tracing::error;

pub mod chainspec;
mod consensus;
mod evm;
mod hardforks;
mod node;
mod system_contracts;

/// No Additional arguments
#[derive(Debug, Clone, Copy, Default, Args)]
#[non_exhaustive]
pub struct NoArgs;

fn main() -> eyre::Result<()> {
    Cli::<BscChainSpecParser, NoArgs>::parse().run(|builder, _| async move {
        let NodeHandle { node, node_exit_future: exit_future } =
            builder.node(BscNode::default()).launch().await?;
        let provider = node.provider.clone();
        let consensus = Arc::new(ParliaConsensus { provider });
        let (service, _) = BlockImportService::new(consensus, node.beacon_engine_handle.clone());

        node.task_executor.spawn(async move {
            if let Err(e) = service.await {
                error!("Import service error: {}", e);
            }
        });

        exit_future.await
    })?;
    Ok(())
}
