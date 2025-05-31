use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::{Parser, Subcommand};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx};
use reth_provider::{BlockNumReader, StatsReader};
use std::sync::Arc;

/// `reth inspect` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(subcommand)]
    command: Subcommands<C>,
}

/// `reth inspect` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands<C: ChainSpecParser> {
    EmptyAccounts(EmptyAccountsCommand<C>),
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `recover` command
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        self,
        ctx: CliContext,
    ) -> eyre::Result<()> {
        match self.command {
            Subcommands::EmptyAccounts(command) => command.execute::<N>(ctx).await,
        }
    }
}

impl<C: ChainSpecParser> Command<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        match &self.command {
            Subcommands::EmptyAccounts(command) => command.chain_spec(),
        }
    }
}

/// `reth recover storage-tries` command
#[derive(Debug, Parser)]
pub struct EmptyAccountsCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> EmptyAccountsCommand<C> {
    /// Execute `storage-tries` recovery command
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        self,
        _ctx: CliContext,
    ) -> eyre::Result<()> {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO)?;

        let provider = provider_factory.provider()?;
        let head = provider.best_block_number()?;
        let num_accounts = provider.count_entries::<tables::PlainAccountState>()?;
        tracing::info!(head, num_accounts, "Starting to inspect state");

        let tx = provider.tx_ref();
        let mut account_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
        let mut storage_cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;

        let mut total_read = 0;
        let mut total_empty = 0;
        let mut total_empty_with_storage = 0;
        let mut entry = account_cursor.first()?;
        while let Some((address, account)) = entry {
            total_read += 1;
            if account.is_empty() {
                tracing::debug!(%address, "Found empty account");
                total_empty += 1;

                if storage_cursor.seek_exact(address)?.is_some_and(|e| e.0 == address) {
                    total_empty_with_storage += 1;
                    tracing::debug!(%address, "Found empty account with storage");
                }
            }

            if total_read % 100_000 == 0 {
                tracing::debug!(count = total_read, "Read account batch");
            }

            entry = account_cursor.next()?;
        }

        if total_empty > 0 {
            tracing::info!(
                count = total_empty,
                with_storage = total_empty_with_storage,
                "Found empty accounts"
            );
        } else {
            tracing::info!("No empty accounts found");
        }

        Ok(())
    }
}

impl<C: ChainSpecParser> EmptyAccountsCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}
