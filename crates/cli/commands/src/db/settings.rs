//! `reth db settings` command for managing storage settings

use clap::{ArgAction, Parser, Subcommand};
use reth_db_common::DbTool;
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, MetadataProvider,
    MetadataWriter, StorageSettings,
};

use crate::common::AccessRights;

/// `reth db settings` subcommand
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Subcommands,
}

impl Command {
    /// Returns database access rights required for the command.
    pub fn access_rights(&self) -> AccessRights {
        match self.command {
            Subcommands::Get => AccessRights::RO,
            Subcommands::Set(_) => AccessRights::RW,
        }
    }
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
#[clap(rename_all = "snake_case")]
pub enum SetCommand {
    /// Store receipts in static files instead of the database
    ReceiptsInStaticFiles {
        #[clap(action(ArgAction::Set))]
        value: bool,
    },
}

impl Command {
    /// Execute the command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        match self.command {
            Subcommands::Get => self.get(tool),
            Subcommands::Set(cmd) => self.set(cmd, tool),
        }
    }

    fn get<N: ProviderNodeTypes>(&self, tool: &DbTool<N>) -> eyre::Result<()> {
        // Read storage settings
        let provider = tool.provider_factory.provider()?;
        let StorageSettings { receipts_in_static_files } =
            provider.storage_settings()?.unwrap_or_default();

        // Display settings
        println!("Current storage settings:");
        println!("  receipts_in_static_files = {receipts_in_static_files}");

        Ok(())
    }

    fn set<N: ProviderNodeTypes>(&self, cmd: SetCommand, tool: &DbTool<N>) -> eyre::Result<()> {
        // Read storage settings
        let provider_rw = tool.provider_factory.database_provider_rw()?;
        // Destruct settings struct to not miss adding support for new fields
        let mut settings @ StorageSettings { receipts_in_static_files: _ } =
            provider_rw.storage_settings()?.unwrap_or_default();

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
