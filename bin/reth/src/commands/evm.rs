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
use crate::primitives::BlockHashOrNumber;
use crate::providers::{BlockNumReader, ProviderError};

/// EVM commands
#[derive(Debug, Parser)]
pub struct EvmCommand {
    #[command(flatten)]
    env: EnvironmentArgs,
    /// The block number to fetch the header for
    block_number: BlockHashOrNumber,
}

impl EvmCommand {
    /// Execute the `evm` command
    pub async fn execute(self) -> Result<()> {
        info!(target: "reth::cli", "Executing EVM command...");

        let Environment { provider_factory, config, data_dir } = self.env.init(AccessRights::RW)?;
        let provider = provider_factory.provider()?;

        let last = provider.last_block_number()?;
        let target = match self.block_number {
                BlockHashOrNumber::Hash(hash) => provider
                    .block_number(hash)?
                    .ok_or_else(|| eyre::eyre!("Block hash not found in database: {hash:?}"))?,
                BlockHashOrNumber::Number(num) => num,
            };

        if target > last {
            eyre::bail!("Target block number is higher than the latest block number")
        }

        match provider.header_by_number(target)? {
            Some(header) => {
                drop(provider);
                println!("Block Header: {:?}", header);
                Ok(())
            },
            None => Err(ProviderError::HeaderNotFound(self.block_number).into()),
        }
    }
}
