//! `reth stage` command

use std::sync::Arc;

use crate::common::CliNodeTypes;
use clap::{Parser, Subcommand};
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_eth_wire::NetPrimitivesFor;
use reth_evm::execute::BlockExecutorProvider;

pub mod drop;
pub mod dump;
pub mod run;
pub mod unwind;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(subcommand)]
    command: Subcommands<C>,
}

/// `reth stage` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands<C: ChainSpecParser> {
    /// Run a single stage.
    ///
    /// Note that this won't use the Pipeline and as a result runs stages
    /// assuming that all the data can be held in memory. It is not recommended
    /// to run a stage for really large block ranges if your computer does not have
    /// a lot of memory to store all the data.
    Run(run::Command<C>),
    /// Drop a stage's tables from the database.
    Drop(drop::Command<C>),
    /// Dumps a stage from a range into a new database.
    Dump(dump::Command<C>),
    /// Unwinds a certain block range, deleting it from the database.
    Unwind(unwind::Command<C>),
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + Hardforks + EthereumHardforks>> Command<C> {
    /// Execute `stage` command
    pub async fn execute<N, E, F, P>(self, ctx: CliContext, executor: F) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
        E: BlockExecutorProvider<Primitives = N::Primitives>,
        F: FnOnce(Arc<C::ChainSpec>) -> E,
        P: NetPrimitivesFor<N::Primitives>,
    {
        match self.command {
            Subcommands::Run(command) => command.execute::<N, _, _, P>(ctx, executor).await,
            Subcommands::Drop(command) => command.execute::<N>().await,
            Subcommands::Dump(command) => command.execute::<N, _, _>(executor).await,
            Subcommands::Unwind(command) => command.execute::<N>().await,
        }
    }
}
