use clap::Parser;
use eyre::Result;
use tracing::info;
use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_db::{init_db, DatabaseEnv, tables, transaction::DbTx};
use reth_primitives::{BlockNumber, Header};
use std::sync::Arc;
use std::path::{Path, PathBuf};
use reth_db_api::database::Database;
use reth_provider::HeaderProvider;

/// EVM commands
#[derive(Debug, Parser)]
pub struct EvmCommand {
    #[command(flatten)]
    env: EnvironmentArgs,
    /// The block number to fetch the header for
    block_number: u64,
}

impl EvmCommand {
    /// Execute the `evm` command
    pub async fn execute(self) -> Result<()> {
        info!(target: "reth::cli", "Executing EVM command...");

        let Environment { provider_factory, config, data_dir } = self.env.init(AccessRights::RW)?;
        let provider = provider_factory.provider()?;
        let mut header = None;
        header = provider.header_by_number(self.block_number)?;

        if header.is_none() {
            println!("Block header not found for block number {}", self.block_number);
        } else {
            println!("Block Header: {:?}", header);
        }

        println!("EVM command executed successfully");

        Ok(())
    }
}
