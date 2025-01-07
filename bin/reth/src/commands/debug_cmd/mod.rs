//! `reth debug` command. Collection of various debugging routines.

use clap::{Parser, Subcommand};
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::CliNodeTypes;
use reth_cli_runner::CliContext;
use reth_node_ethereum::EthEngineTypes;
use reth_primitives::EthPrimitives;

mod build_block;
mod execution;
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
    /// Debug the roundtrip execution of blocks as well as the generated data.
    Execution(execution::Command<C>),
    /// Debug the clean & incremental state root calculations.
    Merkle(merkle::Command<C>),
    /// Debug in-memory state root calculation.
    InMemoryMerkle(in_memory_merkle::Command<C>),
    /// Debug block building.
    BuildBlock(build_block::Command<C>),
}

impl<C: ChainSpecParser<ChainSpec = ChainSpec>> Command<C> {
    /// Execute `debug` command
    pub async fn execute<
        N: CliNodeTypes<Engine = EthEngineTypes, Primitives = EthPrimitives, ChainSpec = C::ChainSpec>,
    >(
        self,
        ctx: CliContext,
    ) -> eyre::Result<()> {
        match self.command {
            Subcommands::Execution(command) => command.execute::<N>(ctx).await,
            Subcommands::Merkle(command) => command.execute::<N>(ctx).await,
            Subcommands::InMemoryMerkle(command) => command.execute::<N>(ctx).await,
            Subcommands::BuildBlock(command) => command.execute::<N>(ctx).await,
        }
    }
}
