use clap::Parser;
use eyre::Result;
use tracing::info;
use crate::commands::common::EnvironmentArgs;
use reth_db::{init_db, DatabaseEnv, tables, transaction::DbTx};
use reth_primitives::{BlockNumber, Header};
use std::sync::Arc;
use std::path::{Path, PathBuf};
use reth_db_api::database::Database;

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

        let block_header = self.get_block_header(self.block_number).await?;

        if let Some(header) = block_header {
            println!("Block Header: {:?}", header);
        } else {
            println!("Block header not found for block number {}", self.block_number);
        }

        println!("EVM command executed successfully");

        Ok(())
    }

    async fn get_block_header(&self, block_number: BlockNumber) -> Result<Option<Header>> {
        let db = self.get_db_instance().await?;
        let tx = db.tx()?;
        let header: Option<Header> = tx.get::<tables::Headers>(block_number)?;
        Ok(header)
    }

    async fn get_db_instance(&self) -> Result<Arc<DatabaseEnv>> {
        let db_path = PathBuf::from("/Users/macbook/Library/Application Support/reth/mainnet/db");
        let env = Arc::new(init_db(db_path, Default::default())?);
        Ok(env)
    }
}
