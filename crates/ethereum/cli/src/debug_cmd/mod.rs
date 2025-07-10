//! `reth debug` command. Collection of various debugging routines.

use alloy_consensus::Header;
use clap::{Parser, Subcommand};
use reth_chainspec::Hardforks;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{CliComponentsBuilder, CliNodeTypes};
use reth_cli_runner::CliContext;
use reth_node_api::NodePrimitives;
use std::sync::Arc;

mod in_memory_merkle;
mod merkle;

/// `reth debug` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(subcommand)]
    command: Subcommands<C>,
}

/// `reth debug` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands<C: ChainSpecParser> {
    /// Debug the clean & incremental state root calculations.
    Merkle(merkle::Command<C>),
    /// Debug in-memory state root calculation.
    InMemoryMerkle(in_memory_merkle::Command<C>),
}

impl<C: ChainSpecParser<ChainSpec: Hardforks>> Command<C> {
    /// Execute `debug` command
    pub async fn execute<N>(
        self,
        ctx: CliContext,
        components: impl CliComponentsBuilder<N>,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec, Primitives: NodePrimitives<BlockHeader = Header>>,
    {
        match self.command {
            Subcommands::Merkle(command) => command.execute::<N>(ctx, components).await,
            Subcommands::InMemoryMerkle(command) => command.execute::<N>(ctx, components).await,
        }
    }
}

impl<C: ChainSpecParser> Command<C> {
    /// Returns the underlying chain being used to run this command
    pub const fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        match &self.command {
            Subcommands::Merkle(command) => command.chain_spec(),
            Subcommands::InMemoryMerkle(command) => command.chain_spec(),
        }
    }
}
