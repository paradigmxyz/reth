//! `reth recover` command.

use crate::common::CliNodeTypes;
use clap::{Parser, Subcommand};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use std::sync::Arc;

mod storage_tries;

/// `reth recover` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(subcommand)]
    command: Subcommands<C>,
}

/// `reth recover` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands<C: ChainSpecParser> {
    /// Recover the node by deleting dangling storage tries.
    StorageTries(storage_tries::Command<C>),
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `recover` command
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        self,
        ctx: CliContext,
    ) -> eyre::Result<()> {
        match self.command {
            Subcommands::StorageTries(command) => command.execute::<N>(ctx).await,
        }
    }
}

impl<C: ChainSpecParser> Command<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        match &self.command {
            Subcommands::StorageTries(command) => command.chain_spec(),
        }
    }
}
