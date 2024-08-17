//! `reth debug` command. Collection of various debugging routines.

use clap::{Parser, Subcommand};
use reth_cli_runner::CliContext;
use reth_node_api::{primitives::NodePrimitives, NodeTypes};

mod build_block;
mod execution;
mod in_memory_merkle;
mod merkle;
mod replay_engine;

/// `reth debug` command
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Subcommands,
}

/// `reth debug` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// Debug the roundtrip execution of blocks as well as the generated data.
    Execution(execution::Command),
    /// Debug the clean & incremental state root calculations.
    Merkle(merkle::Command),
    /// Debug in-memory state root calculation.
    InMemoryMerkle(in_memory_merkle::Command),
    /// Debug block building.
    BuildBlock(build_block::Command),
    /// Debug engine API by replaying stored messages.
    ReplayEngine(replay_engine::Command),
}

impl Command {
    /// Execute `debug` command
    pub async fn execute<T>(self, ctx: CliContext) -> eyre::Result<()>
    where
        T: NodeTypes,
        T::Primitives: Clone,
    {
        match self.command {
            Subcommands::Execution(command) => command.execute::<T::Primitives>(ctx).await,
            Subcommands::Merkle(command) => command.execute::<T::Primitives>(ctx).await,
            Subcommands::InMemoryMerkle(command) => command.execute::<T::Primitives>(ctx).await,
            Subcommands::BuildBlock(command) => command.execute::<T::Primitives>(ctx).await,
            Subcommands::ReplayEngine(command) => command.execute::<T>(ctx).await,
        }
    }
}
