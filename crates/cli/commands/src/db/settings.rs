//! `reth db settings` command for managing storage settings

use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::{ArgAction, Parser, Subcommand};
use reth_chainspec::EthChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_provider::{
    DBProvider, DatabaseProviderFactory, MetadataProvider, MetadataWriter, StorageSettings,
};

/// `reth db settings` subcommand
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[command(subcommand)]
    command: Subcommands,
}

#[derive(Debug, Clone, Copy, Subcommand)]
enum Subcommands {
    /// Get current storage settings from database
    Get,
    /// Set storage settings in database
    #[clap(subcommand)]
    Set(SetCommand),
}

/// Set storage settings
#[derive(Debug, Clone, Copy, Subcommand)]
pub enum SetCommand {
    /// Store receipts in static files instead of the database
    ReceiptsInStaticFiles {
        #[clap(action(ArgAction::Set))]
        value: bool,
    },
}

impl<C: ChainSpecParser> Command<C>
where
    C::ChainSpec: EthChainSpec,
{
    /// Execute the command
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(self) -> eyre::Result<()> {
        match self.command {
            Subcommands::Get => self.get::<N>().await,
            Subcommands::Set(cmd) => self.set::<N>(cmd).await,
        }
    }

    async fn get<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(&self) -> eyre::Result<()> {
        let data_dir = self.env.datadir.clone().resolve_datadir(self.env.chain.chain());
        let db_path = data_dir.db();

        // Check if database exists
        if !db_path.exists() {
            println!("Database does not exist at: {}", db_path.display());
            println!("No storage settings configured yet.");
            return Ok(());
        }

        // Open database in read-only mode
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO)?;

        // Read storage settings
        let provider = provider_factory.provider()?;
        let StorageSettings { receipts_in_static_files } =
            provider.storage_settings()?.unwrap_or_default();

        // Display settings
        println!("Current storage settings:");
        println!("  receipts_in_static_files = {receipts_in_static_files}");

        Ok(())
    }

    async fn set<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        cmd: SetCommand,
    ) -> eyre::Result<()> {
        let data_dir = self.env.datadir.clone().resolve_datadir(self.env.chain.chain());
        let db_path = data_dir.db();

        // Check if database exists
        if !db_path.exists() {
            eyre::bail!(
                "Database does not exist at: {}. Please run 'reth init' first.",
                db_path.display()
            );
        }

        // Open database in read-write mode
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RW)?;

        // Read existing settings
        let provider_rw = provider_factory.database_provider_rw()?;
        let mut settings = provider_rw.storage_settings()?.unwrap_or_default();

        // Update the setting based on the key
        match cmd {
            SetCommand::ReceiptsInStaticFiles { value } => {
                if settings.receipts_in_static_files == value {
                    println!("receipts_in_static_files is already set to {}", value);
                    return Ok(());
                }
                settings.receipts_in_static_files = value;
                println!("Set receipts_in_static_files = {}", value);
            }
        }

        // Write updated settings
        provider_rw.write_storage_settings(settings)?;
        provider_rw.commit()?;

        println!("Storage settings updated successfully.");

        Ok(())
    }
}
